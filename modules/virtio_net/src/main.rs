//! virtio-net: modern virtio 1.0 PCI, poll-mode `/dev/net0`.
//!
//! Speaks only through [`myos_abi::KernelApi`]. Ethernet frames only; no IP.

#![no_std]
#![no_main]

use core::sync::atomic::{Ordering, compiler_fence};

use myos_abi::{ABI_VERSION, KernelApi, ModuleChrOps};

const VENDOR: u16 = 0x1AF4;
const DEV_NET_MODERN: u16 = 0x1041;
const DEV_NET_TRANS: u16 = 0x1000;

const PCI_CAP_VNDR: u8 = 9;
const PCI_STATUS_CAP_LIST: u16 = 0x10;
const VIRTIO_PCI_CAP_COMMON: u8 = 1;
const VIRTIO_PCI_CAP_NOTIFY: u8 = 2;
const VIRTIO_PCI_CAP_ISR: u8 = 3;
const VIRTIO_PCI_CAP_DEVICE: u8 = 4;

const ACKNOWLEDGE: u8 = 1;
const DRIVER: u8 = 2;
const DRIVER_OK: u8 = 4;
const FEATURES_OK: u8 = 8;

const VIRTIO_F_VERSION_1: u32 = 1; // bit 32, features dword 1
const VIRTIO_MSI_NO_VECTOR: u16 = 0xFFFF;

const DESC_F_WRITE: u16 = 2;
const AVAIL_F_NO_INTERRUPT: u16 = 1;
const DESC_SIZE: usize = 16;

const QSIZE: u16 = 16;
const PAGE: usize = 4096;
const BUF_SIZE: usize = 2048;
/// virtio 1.0 + VERSION_1 includes `num_buffers` (12 bytes). Userspace sees
/// the Ethernet frame only.
const HDR_SIZE: usize = 12;
const ETH_MAX: usize = BUF_SIZE - HDR_SIZE;

const C_DEVICE_FEATURE_SELECT: usize = 0;
const C_DEVICE_FEATURE: usize = 4;
const C_DRIVER_FEATURE_SELECT: usize = 8;
const C_DRIVER_FEATURE: usize = 12;
const C_MSIX_CONFIG: usize = 16;
const C_DEVICE_STATUS: usize = 20;
const C_QUEUE_SELECT: usize = 22;
const C_QUEUE_SIZE: usize = 24;
const C_QUEUE_MSIX_VECTOR: usize = 26;
const C_QUEUE_ENABLE: usize = 28;
const C_QUEUE_NOTIFY_OFF: usize = 30;
const C_QUEUE_DESC: usize = 32;
const C_QUEUE_DRIVER: usize = 40;
const C_QUEUE_DEVICE: usize = 48;

struct Queue {
    num: u16,
    notify_off: u16,
    desc: *mut u8,
    avail: *mut u8,
    used: *mut u8,
    last_used: u16,
}

struct Net {
    notify: usize,
    notify_mult: u32,
    rx: Queue,
    tx: Queue,
    rx_buf_va: *mut u8,
    rx_buf_phys: u64,
    tx_buf_va: *mut u8,
    tx_buf_phys: u64,
}

static mut NET: Option<Net> = None;

static OPS: ModuleChrOps = ModuleChrOps {
    read: net_read,
    write: net_write,
};

fn r8(p: usize) -> u8 {
    unsafe { core::ptr::read_volatile(p as *const u8) }
}
fn w8(p: usize, v: u8) {
    unsafe { core::ptr::write_volatile(p as *mut u8, v) }
}
fn r16(p: usize) -> u16 {
    unsafe { core::ptr::read_volatile(p as *const u16) }
}
fn w16(p: usize, v: u16) {
    unsafe { core::ptr::write_volatile(p as *mut u16, v) }
}
fn r32(p: usize) -> u32 {
    unsafe { core::ptr::read_volatile(p as *const u32) }
}
fn w32(p: usize, v: u32) {
    unsafe { core::ptr::write_volatile(p as *mut u32, v) }
}
fn w64(p: usize, v: u64) {
    w32(p, v as u32);
    w32(p + 4, (v >> 32) as u32);
}

fn dma_wmb() {
    compiler_fence(Ordering::Release);
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

fn dma_rmb() {
    compiler_fence(Ordering::SeqCst);
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

fn dcache_civac(va: *mut u8, len: usize) {
    let _ = (va, len);
    #[cfg(target_arch = "aarch64")]
    unsafe {
        if len == 0 {
            return;
        }
        let mut addr = va as usize & !63;
        let end = va as usize + len;
        while addr < end {
            core::arch::asm!("dc civac, {x}", x = in(reg) addr, options(nostack));
            addr += 64;
        }
        core::arch::asm!("dsb sy", options(nostack));
    }
}

fn write_str(api: &KernelApi, msg: &[u8]) {
    unsafe { (api.write_str)(msg.as_ptr(), msg.len()) }
}

fn dma_alloc(api: &KernelApi, n_pages: usize) -> Option<(*mut u8, u64)> {
    let mut phys = 0u64;
    let va = unsafe { (api.dma_alloc)(n_pages, &mut phys) };
    if va.is_null() || phys == 0 {
        None
    } else {
        Some((va, phys))
    }
}

fn pci_find(
    api: &KernelApi,
    vendor: u16,
    device: u16,
) -> Option<(u8, u8, u8)> {
    let mut bus = 0u8;
    let mut slot = 0u8;
    let mut func = 0u8;
    let rc = unsafe { (api.pci_find)(vendor, device, 0, &mut bus, &mut slot, &mut func) };
    if rc == 0 {
        Some((bus, slot, func))
    } else {
        None
    }
}

struct Caps {
    common: usize,
    notify: usize,
    notify_mult: u32,
}

fn map_bar(
    api: &KernelApi,
    bus: u8,
    slot: u8,
    func: u8,
    bar: u8,
    cache: &mut [Option<(usize, u64)>; 6],
) -> Option<(usize, u64)> {
    let idx = bar as usize;
    let slot_cache = match cache.get_mut(idx) {
        Some(s) => s,
        None => return None,
    };
    if let Some(x) = *slot_cache {
        return Some(x);
    }
    let mut va = 0usize;
    let mut size = 0u64;
    let rc = unsafe { (api.pci_bar_map)(bus, slot, func, bar, &mut va, &mut size) };
    if rc != 0 || va == 0 || size == 0 {
        return None;
    }
    *slot_cache = Some((va, size));
    Some((va, size))
}

fn walk_caps(api: &KernelApi, bus: u8, slot: u8, func: u8) -> Option<Caps> {
    let cmdsts = unsafe { (api.pci_cfg_read32)(bus, slot, func, 4) };
    let sts = (cmdsts >> 16) as u16;
    if sts & PCI_STATUS_CAP_LIST == 0 {
        return None;
    }
    let mut cap = (unsafe { (api.pci_cfg_read32)(bus, slot, func, 0x34) } & 0xFC) as u8;
    let mut cache: [Option<(usize, u64)>; 6] = [None; 6];
    let mut common = 0usize;
    let mut notify = 0usize;
    let mut notify_mult = 0u32;
    let mut hops = 0u8;
    while cap != 0 && hops < 64 {
        hops += 1;
        let w0 = unsafe { (api.pci_cfg_read32)(bus, slot, func, cap) };
        let id = (w0 & 0xFF) as u8;
        let next = ((w0 >> 8) & 0xFF) as u8;
        if id == PCI_CAP_VNDR {
            let cfg_type = ((w0 >> 24) & 0xFF) as u8;
            let w1 = unsafe { (api.pci_cfg_read32)(bus, slot, func, cap.wrapping_add(4)) };
            let bar = (w1 & 0xFF) as u8;
            let off = unsafe { (api.pci_cfg_read32)(bus, slot, func, cap.wrapping_add(8)) };
            let len = unsafe { (api.pci_cfg_read32)(bus, slot, func, cap.wrapping_add(12)) };
            if let Some((va, size)) = map_bar(api, bus, slot, func, bar, &mut cache) {
                let start = off as u64;
                let end = start.saturating_add(u64::from(len));
                if end <= size {
                    let mmio = va.wrapping_add(off as usize);
                    match cfg_type {
                        VIRTIO_PCI_CAP_COMMON => common = mmio,
                        VIRTIO_PCI_CAP_NOTIFY => {
                            notify = mmio;
                            notify_mult =
                                unsafe { (api.pci_cfg_read32)(bus, slot, func, cap.wrapping_add(16)) };
                        }
                        VIRTIO_PCI_CAP_ISR | VIRTIO_PCI_CAP_DEVICE => {}
                        _ => {}
                    }
                }
            }
        }
        cap = next & 0xFC;
        if next != 0 && (next & 0xFC) == 0 {
            break;
        }
    }
    if common == 0 || notify == 0 {
        return None;
    }
    Some(Caps {
        common,
        notify,
        notify_mult,
    })
}

unsafe fn write_desc(desc: *mut u8, i: u16, addr: u64, len: u32, flags: u16) {
    let p = unsafe { desc.add(i as usize * DESC_SIZE) };
    unsafe {
        core::ptr::write_volatile(p as *mut u64, addr);
        core::ptr::write_volatile(p.add(8) as *mut u32, len);
        core::ptr::write_volatile(p.add(12) as *mut u16, flags);
        core::ptr::write_volatile(p.add(14) as *mut u16, 0);
    }
}

fn setup_queue(api: &KernelApi, common: usize, qsel: u16) -> Option<Queue> {
    w16(common + C_QUEUE_SELECT, qsel);
    let max = r16(common + C_QUEUE_SIZE);
    let num = if max >= QSIZE {
        QSIZE
    } else if max >= 8 {
        8
    } else if max >= 4 {
        4
    } else if max >= 2 {
        2
    } else {
        return None;
    };
    w16(common + C_QUEUE_SIZE, num);
    w16(common + C_QUEUE_MSIX_VECTOR, VIRTIO_MSI_NO_VECTOR);

    let (desc, desc_phys) = dma_alloc(api, 1)?;
    let (avail, avail_phys) = dma_alloc(api, 1)?;
    let (used, used_phys) = dma_alloc(api, 1)?;

    w64(common + C_QUEUE_DESC, desc_phys);
    w64(common + C_QUEUE_DRIVER, avail_phys);
    w64(common + C_QUEUE_DEVICE, used_phys);
    dma_wmb();

    let notify_off = r16(common + C_QUEUE_NOTIFY_OFF);
    w16(common + C_QUEUE_ENABLE, 1);
    dma_wmb();
    if r16(common + C_QUEUE_ENABLE) == 0 {
        return None;
    }

    unsafe {
        core::ptr::write_volatile(avail as *mut u16, AVAIL_F_NO_INTERRUPT);
    }

    Some(Queue {
        num,
        notify_off,
        desc,
        avail,
        used,
        last_used: 0,
    })
}

fn notify(net: &Net, q: &Queue, qindex: u16) {
    let off = (q.notify_off as u32).wrapping_mul(net.notify_mult) as usize;
    w16(net.notify.wrapping_add(off), qindex);
}

fn push(q: &Queue, head: u16) {
    let avail = q.avail as usize;
    let idx = r16(avail + 2);
    let slot = (idx as usize) % (q.num as usize);
    w16(avail + 4 + slot * 2, head);
    dma_wmb();
    dcache_civac(q.avail, PAGE);
    w16(avail + 2, idx.wrapping_add(1));
    dma_wmb();
    dcache_civac(q.avail, PAGE);
}

fn used_idx(q: &Queue) -> u16 {
    dcache_civac(q.used, PAGE);
    dma_rmb();
    r16(q.used as usize + 2)
}

fn used_elem(q: &Queue, idx: u16) -> (u16, u32) {
    let slot = (idx as usize) % (q.num as usize);
    let p = q.used as usize + 4 + slot * 8;
    dcache_civac(q.used, PAGE);
    let id = r32(p);
    let len = r32(p + 4);
    (id as u16, len)
}

fn post_rx(net: &Net, i: u16) {
    let addr = net.rx_buf_phys + u64::from(i) * BUF_SIZE as u64;
    unsafe {
        write_desc(net.rx.desc, i, addr, BUF_SIZE as u32, DESC_F_WRITE);
    }
    dcache_civac(net.rx.desc, PAGE);
    push(&net.rx, i);
}

fn probe(api: &KernelApi) -> Option<Net> {
    let (bus, slot, func) = pci_find(api, VENDOR, DEV_NET_MODERN)
        .or_else(|| pci_find(api, VENDOR, DEV_NET_TRANS))?;

    unsafe { (api.pci_enable)(bus, slot, func) };

    let caps = walk_caps(api, bus, slot, func)?;
    let common = caps.common;

    w8(common + C_DEVICE_STATUS, 0);
    dma_wmb();
    w8(common + C_DEVICE_STATUS, ACKNOWLEDGE);
    w8(common + C_DEVICE_STATUS, ACKNOWLEDGE | DRIVER);
    w16(common + C_MSIX_CONFIG, VIRTIO_MSI_NO_VECTOR);

    w32(common + C_DEVICE_FEATURE_SELECT, 1);
    let f1 = r32(common + C_DEVICE_FEATURE);
    if f1 & VIRTIO_F_VERSION_1 == 0 {
        return None;
    }
    w32(common + C_DRIVER_FEATURE_SELECT, 0);
    w32(common + C_DRIVER_FEATURE, 0);
    w32(common + C_DRIVER_FEATURE_SELECT, 1);
    w32(common + C_DRIVER_FEATURE, VIRTIO_F_VERSION_1);

    w8(
        common + C_DEVICE_STATUS,
        ACKNOWLEDGE | DRIVER | FEATURES_OK,
    );
    dma_wmb();
    if r8(common + C_DEVICE_STATUS) & FEATURES_OK == 0 {
        return None;
    }

    let rx = setup_queue(api, common, 0)?;
    let tx = setup_queue(api, common, 1)?;

    let rx_pages = ((rx.num as usize) * BUF_SIZE + PAGE - 1) / PAGE;
    let (rx_buf_va, rx_buf_phys) = dma_alloc(api, rx_pages)?;
    let (tx_buf_va, tx_buf_phys) = dma_alloc(api, 1)?;

    let net = Net {
        notify: caps.notify,
        notify_mult: caps.notify_mult,
        rx,
        tx,
        rx_buf_va,
        rx_buf_phys,
        tx_buf_va,
        tx_buf_phys,
    };

    let n = net.rx.num;
    let mut i = 0u16;
    while i < n {
        post_rx(&net, i);
        i += 1;
    }

    w8(
        common + C_DEVICE_STATUS,
        ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK,
    );
    dma_wmb();
    notify(&net, &net.rx, 0);
    Some(net)
}

unsafe extern "C" fn net_read(buf: *mut u8, buf_len: usize) -> i32 {
    if buf.is_null() {
        return -1;
    }
    let net = unsafe {
        match NET.as_mut() {
            Some(n) => n,
            None => return -1,
        }
    };
    let used = used_idx(&net.rx);
    if used == net.rx.last_used {
        return 0;
    }
    let (id, len) = used_elem(&net.rx, net.rx.last_used);
    net.rx.last_used = net.rx.last_used.wrapping_add(1);
    if (id as usize) >= net.rx.num as usize {
        return 0;
    }
    let pkt_len = if len as usize > HDR_SIZE {
        (len as usize) - HDR_SIZE
    } else {
        0
    };
    let copy = if pkt_len < buf_len { pkt_len } else { buf_len };
    let src = unsafe { net.rx_buf_va.add(id as usize * BUF_SIZE + HDR_SIZE) };
    dcache_civac(unsafe { net.rx_buf_va.add(id as usize * BUF_SIZE) }, BUF_SIZE);
    if copy != 0 {
        unsafe { core::ptr::copy_nonoverlapping(src, buf, copy) };
    }
    post_rx(net, id);
    notify(net, &net.rx, 0);
    copy as i32
}

unsafe extern "C" fn net_write(buf: *const u8, buf_len: usize) -> i32 {
    if buf_len == 0 {
        return 0;
    }
    if buf.is_null() {
        return -1;
    }
    let net = unsafe {
        match NET.as_mut() {
            Some(n) => n,
            None => return -1,
        }
    };
    let frame = if buf_len > ETH_MAX { ETH_MAX } else { buf_len };
    unsafe {
        core::ptr::write_bytes(net.tx_buf_va, 0, HDR_SIZE);
        core::ptr::copy_nonoverlapping(buf, net.tx_buf_va.add(HDR_SIZE), frame);
    }
    dcache_civac(net.tx_buf_va, HDR_SIZE + frame);
    let total = (HDR_SIZE + frame) as u32;
    unsafe {
        write_desc(net.tx.desc, 0, net.tx_buf_phys, total, 0);
    }
    dcache_civac(net.tx.desc, PAGE);
    push(&net.tx, 0);
    notify(net, &net.tx, 1);

    let want = net.tx.last_used.wrapping_add(1);
    let mut spins = 0u32;
    while spins < 50_000_000 {
        if used_idx(&net.tx) == want {
            net.tx.last_used = want;
            return frame as i32;
        }
        core::hint::spin_loop();
        spins += 1;
    }
    -1
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api: *const KernelApi) -> i32 {
    unsafe {
        if api.is_null() {
            return -1;
        }
        let api = &*api;
        if api.abi_version != ABI_VERSION {
            return -2;
        }
        match probe(api) {
            Some(net) => {
                NET = Some(net);
                let rc = (api.dev_register)(b"net0".as_ptr(), 4, &OPS);
                if rc != 0 {
                    NET = None;
                    write_str(api, b"virtio-net skip\n");
                    return 0;
                }
                write_str(api, b"virtio-net mod ok\n");
                0
            }
            None => {
                write_str(api, b"virtio-net skip\n");
                0
            }
        }
    }
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
