//! virtio-net: modern virtio 1.0 PCI `/dev/netN/`.
//!
//! Speaks only through [`myos_abi::KernelApi`]. Ethernet frames only; no IP.
//! `data` reads never block (a `0` means "no frame"), but the RX queue
//! raises an interrupt (MSI-X on x86_64, the INTx line elsewhere) that wakes
//! the pollers, so `netd` sleeps in `poll` on `data` until a frame instead
//! of polling the ring. `ctl` names the device's MAC and whether that
//! interrupt works (`mac 52:54:00:12:34:56`, `irq on|off`).

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use core::sync::atomic::{AtomicUsize, Ordering, compiler_fence};

use myos_abi::{
    status_ok, ABI_VERSION, ApiCell, KernelApi, Lock, MYOS_IRQ_INTX, MYOS_POLLIN, MYOS_POLLOUT,
    ModuleChrOps,
};

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
const VIRTIO_NET_F_MAC: u32 = 1 << 5;
const MAX_NET: usize = 4;
/// Fallback when the device does not advertise MAC (matches QEMU default).
const DEFAULT_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
const VIRTIO_MSI_NO_VECTOR: u16 = 0xFFFF;

const DESC_F_WRITE: u16 = 2;
const AVAIL_F_NO_INTERRUPT: u16 = 1;
const DESC_SIZE: usize = 16;

/// Entries per queue (the device's maximum if smaller): the frames the
/// device may hold for netd before it drops them. A 64 KiB TCP window is
/// 45 full frames; 16 lost most of a burst, and every drop a retransmit.
const QSIZE: u16 = 128;
const PAGE: usize = 4096;
const BUF_SIZE: usize = 2048;
/// RX buffers come in runs of this many pages (`RX_CHUNK_BUFS` buffers
/// each): the contiguous allocation a module gets is a run of frames the
/// allocator happens to hand out in order, which 64 pages at once are not
/// sure to be.
const RX_CHUNK_PAGES: usize = 8;
const RX_CHUNK_BUFS: usize = RX_CHUNK_PAGES * PAGE / BUF_SIZE;
const RX_CHUNKS: usize = (QSIZE as usize).div_ceil(RX_CHUNK_BUFS);
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
    /// The RX buffers, [`RX_CHUNK_BUFS`] per run (`rx_buf`).
    rx_chunks: [(*mut u8, u64); RX_CHUNKS],
    tx_buf_va: *mut u8,
    tx_buf_phys: u64,
    mac: [u8; 6],
    /// The RX queue raises interrupts (`pci_irq_enable` succeeded).
    irq_ok: bool,
}

// SAFETY: the pointers are the device's own rings and buffers, used only
// by whoever holds its slot in `NETS`.
unsafe impl Send for Net {}

/// The devices, a lock per slot, held for a `data` read or write (one RX
/// and one TX buffer per device) or a look at `ctl`. The interrupt handler
/// never takes it: what it needs is in [`ISR`].
static NETS: [Lock<Option<Net>>; MAX_NET] = [const { Lock::new(None) }; MAX_NET];
/// Per slot, the device's ISR status byte (virtio-pci ISR capability), read
/// by the interrupt handler to deassert a legacy INTx line; 0 before the
/// device is up or when the cap is absent.
static ISR: [AtomicUsize; MAX_NET] = [const { AtomicUsize::new(0) }; MAX_NET];
static API: ApiCell = ApiCell::new();

unsafe extern "C" fn net0_read(buf: *mut u8, buf_len: usize) -> i32 {
    net_read_n(0, buf, buf_len)
}
unsafe extern "C" fn net0_write(buf: *const u8, buf_len: usize) -> i32 {
    net_write_n(0, buf, buf_len)
}
unsafe extern "C" fn net0_poll() -> u32 {
    net_poll_n(0)
}
unsafe extern "C" fn net0_ctl(buf: *mut u8, cap: usize) -> i32 {
    net_ctl_n(0, buf, cap)
}
unsafe extern "C" fn net1_read(buf: *mut u8, buf_len: usize) -> i32 {
    net_read_n(1, buf, buf_len)
}
unsafe extern "C" fn net1_write(buf: *const u8, buf_len: usize) -> i32 {
    net_write_n(1, buf, buf_len)
}
unsafe extern "C" fn net1_poll() -> u32 {
    net_poll_n(1)
}
unsafe extern "C" fn net1_ctl(buf: *mut u8, cap: usize) -> i32 {
    net_ctl_n(1, buf, cap)
}
unsafe extern "C" fn net2_read(buf: *mut u8, buf_len: usize) -> i32 {
    net_read_n(2, buf, buf_len)
}
unsafe extern "C" fn net2_write(buf: *const u8, buf_len: usize) -> i32 {
    net_write_n(2, buf, buf_len)
}
unsafe extern "C" fn net2_poll() -> u32 {
    net_poll_n(2)
}
unsafe extern "C" fn net2_ctl(buf: *mut u8, cap: usize) -> i32 {
    net_ctl_n(2, buf, cap)
}
unsafe extern "C" fn net3_read(buf: *mut u8, buf_len: usize) -> i32 {
    net_read_n(3, buf, buf_len)
}
unsafe extern "C" fn net3_write(buf: *const u8, buf_len: usize) -> i32 {
    net_write_n(3, buf, buf_len)
}
unsafe extern "C" fn net3_poll() -> u32 {
    net_poll_n(3)
}
unsafe extern "C" fn net3_ctl(buf: *mut u8, cap: usize) -> i32 {
    net_ctl_n(3, buf, cap)
}

static OPS: [ModuleChrOps; MAX_NET] = [
    ModuleChrOps {
        read: net0_read,
        write: net0_write,
        poll: Some(net0_poll),
        ctl_read: Some(net0_ctl),
        ctl_write: None,
    },
    ModuleChrOps {
        read: net1_read,
        write: net1_write,
        poll: Some(net1_poll),
        ctl_read: Some(net1_ctl),
        ctl_write: None,
    },
    ModuleChrOps {
        read: net2_read,
        write: net2_write,
        poll: Some(net2_poll),
        ctl_read: Some(net2_ctl),
        ctl_write: None,
    },
    ModuleChrOps {
        read: net3_read,
        write: net3_write,
        poll: Some(net3_poll),
        ctl_read: Some(net3_ctl),
        ctl_write: None,
    },
];

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

fn dma_alloc(api: &KernelApi, n_pages: usize) -> Option<(*mut u8, u64)> {
    let mut phys = 0u64;
    let va = api.dma_alloc(n_pages, &mut phys);
    if va.is_null() || phys == 0 {
        None
    } else {
        Some((va, phys))
    }
}

fn pci_find_at(api: &KernelApi, vendor: u16, device: u16, index: u32) -> Option<(u8, u8, u8)> {
    let mut bus = 0u8;
    let mut slot = 0u8;
    let mut func = 0u8;
    let rc = api.pci_find(vendor, device, index, &mut bus, &mut slot, &mut func);
    if rc == 0 {
        Some((bus, slot, func))
    } else {
        None
    }
}

/// Enumerate modern virtio-net first, then transitional, by logical index.
fn pci_find_net(api: &KernelApi, index: u32) -> Option<(u8, u8, u8)> {
    let mut seen = 0u32;
    for i in 0..8u32 {
        match pci_find_at(api, VENDOR, DEV_NET_MODERN, i) {
            Some(bdf) => {
                if seen == index {
                    return Some(bdf);
                }
                seen += 1;
            }
            None => break,
        }
    }
    for i in 0..8u32 {
        match pci_find_at(api, VENDOR, DEV_NET_TRANS, i) {
            Some(bdf) => {
                if seen == index {
                    return Some(bdf);
                }
                seen += 1;
            }
            None => break,
        }
    }
    None
}

struct Caps {
    common: usize,
    notify: usize,
    notify_mult: u32,
    device: usize,
    isr: usize,
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
    let rc = api.pci_bar_map(bus, slot, func, bar, &mut va, &mut size);
    if rc != 0 || va == 0 || size == 0 {
        return None;
    }
    *slot_cache = Some((va, size));
    Some((va, size))
}

fn walk_caps(api: &KernelApi, bus: u8, slot: u8, func: u8) -> Option<Caps> {
    let cmdsts = api.pci_cfg_read32(bus, slot, func, 4);
    let sts = (cmdsts >> 16) as u16;
    if sts & PCI_STATUS_CAP_LIST == 0 {
        return None;
    }
    let mut cap = (api.pci_cfg_read32(bus, slot, func, 0x34) & 0xFC) as u8;
    let mut cache: [Option<(usize, u64)>; 6] = [None; 6];
    let mut common = 0usize;
    let mut notify = 0usize;
    let mut notify_mult = 0u32;
    let mut device = 0usize;
    let mut isr = 0usize;
    let mut hops = 0u8;
    while cap != 0 && hops < 64 {
        hops += 1;
        let w0 = api.pci_cfg_read32(bus, slot, func, cap);
        let id = (w0 & 0xFF) as u8;
        let next = ((w0 >> 8) & 0xFF) as u8;
        if id == PCI_CAP_VNDR {
            let cfg_type = ((w0 >> 24) & 0xFF) as u8;
            let w1 = api.pci_cfg_read32(bus, slot, func, cap.wrapping_add(4));
            let bar = (w1 & 0xFF) as u8;
            let off = api.pci_cfg_read32(bus, slot, func, cap.wrapping_add(8));
            let len = api.pci_cfg_read32(bus, slot, func, cap.wrapping_add(12));
            if let Some((va, size)) = map_bar(api, bus, slot, func, bar, &mut cache) {
                let start = off as u64;
                let end = start.saturating_add(u64::from(len));
                if end <= size {
                    let mmio = va.wrapping_add(off as usize);
                    match cfg_type {
                        VIRTIO_PCI_CAP_COMMON => common = mmio,
                        VIRTIO_PCI_CAP_NOTIFY => {
                            notify = mmio;
                            notify_mult = api.pci_cfg_read32(bus, slot, func, cap.wrapping_add(16));
                        }
                        VIRTIO_PCI_CAP_DEVICE => device = mmio,
                        VIRTIO_PCI_CAP_ISR => isr = mmio,
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
        device,
        isr,
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

/// RX buffer `i`: its virtual and physical address.
fn rx_buf(net: &Net, i: u16) -> (*mut u8, u64) {
    let (va, phys) = net.rx_chunks[i as usize / RX_CHUNK_BUFS];
    let off = (i as usize % RX_CHUNK_BUFS) * BUF_SIZE;
    (unsafe { va.add(off) }, phys + off as u64)
}

fn post_rx(net: &Net, i: u16) {
    let (_, addr) = rx_buf(net, i);
    unsafe {
        write_desc(net.rx.desc, i, addr, BUF_SIZE as u32, DESC_F_WRITE);
    }
    dcache_civac(net.rx.desc, PAGE);
    push(&net.rx, i);
}

fn probe(api: &KernelApi, pci_index: u32, slot_index: usize) -> Option<Net> {
    let (bus, slot, func) = pci_find_net(api, pci_index)?;

    api.pci_enable(bus, slot, func);

    let caps = walk_caps(api, bus, slot, func)?;
    let common = caps.common;

    w8(common + C_DEVICE_STATUS, 0);
    dma_wmb();
    w8(common + C_DEVICE_STATUS, ACKNOWLEDGE);
    w8(common + C_DEVICE_STATUS, ACKNOWLEDGE | DRIVER);
    w16(common + C_MSIX_CONFIG, VIRTIO_MSI_NO_VECTOR);

    w32(common + C_DEVICE_FEATURE_SELECT, 0);
    let f0 = r32(common + C_DEVICE_FEATURE);
    w32(common + C_DEVICE_FEATURE_SELECT, 1);
    let f1 = r32(common + C_DEVICE_FEATURE);
    if f1 & VIRTIO_F_VERSION_1 == 0 {
        return None;
    }
    let mut driver_f0 = 0u32;
    if f0 & VIRTIO_NET_F_MAC != 0 {
        driver_f0 |= VIRTIO_NET_F_MAC;
    }
    w32(common + C_DRIVER_FEATURE_SELECT, 0);
    w32(common + C_DRIVER_FEATURE, driver_f0);
    w32(common + C_DRIVER_FEATURE_SELECT, 1);
    w32(common + C_DRIVER_FEATURE, VIRTIO_F_VERSION_1);

    w8(common + C_DEVICE_STATUS, ACKNOWLEDGE | DRIVER | FEATURES_OK);
    dma_wmb();
    if r8(common + C_DEVICE_STATUS) & FEATURES_OK == 0 {
        return None;
    }

    let mut mac = DEFAULT_MAC;
    if caps.device != 0 && (f0 & VIRTIO_NET_F_MAC) != 0 {
        let mut i = 0usize;
        while i < 6 {
            mac[i] = r8(caps.device + i);
            i += 1;
        }
    }

    let rx = setup_queue(api, common, 0)?;
    let tx = setup_queue(api, common, 1)?;

    let mut rx_chunks = [(core::ptr::null_mut(), 0u64); RX_CHUNKS];
    for chunk in rx_chunks.iter_mut().take((rx.num as usize).div_ceil(RX_CHUNK_BUFS)) {
        *chunk = dma_alloc(api, RX_CHUNK_PAGES)?;
    }
    let (tx_buf_va, tx_buf_phys) = dma_alloc(api, 1)?;

    let mut net = Net {
        notify: caps.notify,
        notify_mult: caps.notify_mult,
        rx,
        tx,
        rx_chunks,
        tx_buf_va,
        tx_buf_phys,
        mac,
        irq_ok: false,
    };

    // RX interrupts: route the function's interrupt to `net_irq`, point the
    // RX queue at the MSI-X entry the kernel chose (or leave INTx), and let
    // the device interrupt for RX completions (the TX queue keeps
    // AVAIL_F_NO_INTERRUPT). Failure leaves the device in poll mode; `ctl`
    // says `irq off` then, so netd keeps its timed polling.
    let mut msix_entry: u16 = MYOS_IRQ_INTX;
    ISR[slot_index].store(caps.isr, Ordering::Release);
    let rc = api.pci_irq_enable(
        bus,
        slot,
        func,
        "virtio-net",
        net_irq,
        slot_index as *mut core::ffi::c_void,
        &mut msix_entry,
    );
    if rc == 0 {
        let mut ok = true;
        if msix_entry != MYOS_IRQ_INTX {
            w16(common + C_QUEUE_SELECT, 0);
            w16(common + C_QUEUE_MSIX_VECTOR, msix_entry);
            dma_wmb();
            ok = r16(common + C_QUEUE_MSIX_VECTOR) == msix_entry;
        }
        if ok {
            unsafe {
                core::ptr::write_volatile(net.rx.avail as *mut u16, 0);
            }
            dcache_civac(net.rx.avail, PAGE);
            dma_wmb();
            net.irq_ok = true;
        }
    }

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

/// Run `f` on the device in slot `idx` with its lock held; `None` for an
/// empty slot.
fn with_net<R>(idx: usize, f: impl FnOnce(&mut Net) -> R) -> Option<R> {
    NETS.get(idx)?.lock().as_mut().map(f)
}

/// Interrupt handler: ack the device (ISR read deasserts a legacy INTx line;
/// harmless under MSI-X) and wake the pollers: `netd` sleeps in `poll` on
/// `data`. Atomics only: a handler must not wait for the slot's lock.
unsafe extern "C" fn net_irq(ctx: *mut core::ffi::c_void) {
    let idx = ctx as usize;
    let isr = ISR.get(idx).map_or(0, |a| a.load(Ordering::Acquire));
    if isr != 0 {
        let _ = r8(isr);
    }
    if let Some(api) = API.try_get() {
        api.wake_any();
    }
}

fn rx_available(net: &Net) -> bool {
    used_idx(&net.rx) != net.rx.last_used
}

/// `poll` bits of `data`: readable when the RX ring holds a frame, always
/// writable.
fn net_poll_n(idx: usize) -> u32 {
    let readable = with_net(idx, |net| rx_available(net)).unwrap_or(false);
    if readable { MYOS_POLLIN | MYOS_POLLOUT } else { MYOS_POLLOUT }
}

/// The text of `ctl`: `mac 52:54:00:12:34:56` and `irq on|off` (whether the
/// RX queue interrupts, so a reader knows if `poll` on `data` wakes by itself).
fn net_ctl_n(idx: usize, buf: *mut u8, cap: usize) -> i32 {
    with_net(idx, |net| ctl_text(net, buf, cap)).unwrap_or(-1)
}

fn ctl_text(net: &Net, buf: *mut u8, cap: usize) -> i32 {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = [0u8; 32];
    let mut n = 0;
    for b in b"mac " {
        text[n] = *b;
        n += 1;
    }
    for (i, b) in net.mac.iter().enumerate() {
        if i != 0 {
            text[n] = b':';
            n += 1;
        }
        text[n] = HEX[(b >> 4) as usize];
        text[n + 1] = HEX[(b & 0xf) as usize];
        n += 2;
    }
    let irq: &[u8] = if net.irq_ok { b"\nirq on\n" } else { b"\nirq off\n" };
    for b in irq {
        text[n] = *b;
        n += 1;
    }
    if !buf.is_null() {
        let copy = n.min(cap);
        unsafe { core::ptr::copy_nonoverlapping(text.as_ptr(), buf, copy) };
    }
    n as i32
}

fn net_read_n(idx: usize, buf: *mut u8, buf_len: usize) -> i32 {
    if buf.is_null() {
        return -1;
    }
    with_net(idx, |net| read_frame(net, buf, buf_len)).unwrap_or(-1)
}

/// The next received frame into `buf`; 0 when none waits.
fn read_frame(net: &mut Net, buf: *mut u8, buf_len: usize) -> i32 {
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
    let (buf_va, _) = rx_buf(net, id);
    let src = unsafe { buf_va.add(HDR_SIZE) };
    dcache_civac(buf_va, BUF_SIZE);
    if copy != 0 {
        unsafe { core::ptr::copy_nonoverlapping(src, buf, copy) };
    }
    post_rx(net, id);
    notify(net, &net.rx, 0);
    copy as i32
}

fn net_write_n(idx: usize, buf: *const u8, buf_len: usize) -> i32 {
    if buf_len == 0 {
        return 0;
    }
    if buf.is_null() {
        return -1;
    }
    with_net(idx, |net| write_frame(net, buf, buf_len)).unwrap_or(-1)
}

/// Send `buf` as one frame (cut at [`ETH_MAX`]) and wait for the device to
/// take it; the bytes sent.
/// The TX ring is idle: the device used every frame pushed so far (its used
/// index caught up with the avail index this driver writes), so the one TX
/// buffer is free.
fn tx_idle(net: &Net) -> bool {
    used_idx(&net.tx) == r16(net.tx.avail as usize + 2)
}

/// Wait for [`tx_idle`] at most `TX_SPIN` polls. Syscalls run with
/// interrupts masked (SIE clear on RISC-V): the old 50e6-spin wait could
/// freeze the guest for tens of seconds when the used ring lagged, so CI
/// hung after `tcc std ok` with no ping timeout printed. The cap is well
/// above normal QEMU completion (usually <<1k spins) but far below a
/// multi-second IRQ-off stall.
fn wait_tx_idle(net: &Net) -> bool {
    const TX_SPIN: u32 = 50_000;
    for _ in 0..TX_SPIN {
        if tx_idle(net) {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// Send one frame. A frame the device has not used within the wait is
/// reported as not sent but stays in flight: the next send waits for it
/// before it reuses the buffer. Comparing the two ring indices, rather
/// than a count of the frames seen used, keeps a late completion from
/// putting the driver behind the device for good (every later send then
/// spun its whole wait and failed: netd's transfers stalled).
fn write_frame(net: &mut Net, buf: *const u8, buf_len: usize) -> i32 {
    if !wait_tx_idle(net) {
        return -1;
    }
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
    if wait_tx_idle(net) { frame as i32 } else { -1 }
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api: *const KernelApi) -> i32 {
    if api.is_null() {
        return -1;
    }
    let api: &'static KernelApi = unsafe { &*api };
    if api.abi_version != ABI_VERSION {
        return -2;
    }
    unsafe { API.set(api) };
    const NAMES: [&str; MAX_NET] = ["net0", "net1", "net2", "net3"];
    let mut registered = 0usize;
    let mut pci_index = 0u32;
    while registered < MAX_NET {
        match probe(api, pci_index, registered) {
            Some(net) => {
                let slot = registered;
                *NETS[slot].lock() = Some(net);
                let rc = api.dev_register(NAMES[slot], &OPS[slot]);
                if rc != 0 {
                    *NETS[slot].lock() = None;
                    // Slot full or name clash — stop trying further NICs.
                    break;
                }
                registered += 1;
                pci_index += 1;
            }
            None => break,
        }
    }
    if registered == 0 {
        api.write_str("virtio-net skip\n");
    } else {
        status_ok(api, "virtio-net");
    }
    0
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
