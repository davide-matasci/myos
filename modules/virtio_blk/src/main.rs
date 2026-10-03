//! virtio-blk: `/dev/vd*` through [`myos_abi::KernelApi::blk_register`].
//!
//! Speaks only through the module ABI. Two transports, chosen by the target:
//! the transitional PCI device (`0x1001`) with a legacy I/O BAR on x86_64
//! (QEMU is started with `disable-modern=on`), and virtio-mmio v2 on the
//! QEMU `virt` boards (aarch64, riscv64). Polling only; no virtio IRQ.

#![no_std]
#![no_main]

use myos_abi::{ABI_VERSION, KernelApi, ModuleBlkOps, status_ok};
use virtq::{Ring, SECTOR};

const MAX_DISKS: usize = 8;
const PAGE: usize = 4096;

struct Dev {
    ring: Ring,
    capacity: u64,
    /// x86: the I/O port base; mmio: the register window.
    base: usize,
}

static mut DEVS: [Option<Dev>; MAX_DISKS] = [const { None }; MAX_DISKS];
static mut API: Option<&'static KernelApi> = None;

static OPS: ModuleBlkOps = ModuleBlkOps {
    read: blk_read,
    write: blk_write,
    capacity_sectors: blk_capacity,
};

fn api() -> &'static KernelApi {
    unsafe { (*core::ptr::addr_of!(API)).expect("virtio_blk: API") }
}

fn dma_page() -> Option<(u64, *mut u8)> {
    dma_pages(1)
}

fn dma_pages(n: usize) -> Option<(u64, *mut u8)> {
    let mut phys = 0u64;
    let va = unsafe { (api().dma_alloc)(n, &mut phys) };
    if va.is_null() { None } else { Some((phys, va)) }
}

fn dev(i: usize) -> Option<&'static mut Dev> {
    unsafe { (*core::ptr::addr_of_mut!(DEVS)).get_mut(i)?.as_mut() }
}

unsafe extern "C" fn blk_read(ctx: usize, lba: u64, buf: *mut u8, len: usize) -> i32 {
    if buf.is_null() || len % SECTOR != 0 {
        return -1;
    }
    let Some(d) = dev(ctx) else {
        return -1;
    };
    let base = d.base;
    let buf = unsafe { core::slice::from_raw_parts_mut(buf, len) };
    match unsafe { d.ring.read_buf(lba, buf, || transport::notify(base)) } {
        Ok(()) => 0,
        Err(()) => -1,
    }
}

unsafe extern "C" fn blk_write(ctx: usize, lba: u64, buf: *const u8, len: usize) -> i32 {
    if buf.is_null() || len % SECTOR != 0 {
        return -1;
    }
    let Some(d) = dev(ctx) else {
        return -1;
    };
    let base = d.base;
    let buf = unsafe { core::slice::from_raw_parts(buf, len) };
    match unsafe { d.ring.write_buf(lba, buf, || transport::notify(base)) } {
        Ok(()) => 0,
        Err(()) => -1,
    }
}

unsafe extern "C" fn blk_capacity(ctx: usize) -> u64 {
    dev(ctx).map_or(0, |d| d.capacity)
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api_ptr: *const KernelApi) -> i32 {
    if api_ptr.is_null() {
        return -1;
    }
    let api: &'static KernelApi = unsafe { &*api_ptr };
    if api.abi_version != ABI_VERSION {
        return -2;
    }
    unsafe {
        *core::ptr::addr_of_mut!(API) = Some(api);
    }
    if probe(api) > 0 {
        status_ok(api, "virtio block");
    }
    0
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

/// A `rescan` of `/proc/pci`: disks that appeared since init.
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_rescan() {
    probe(api());
}

/// Whether a device with this register base (an I/O port base on x86, an
/// MMIO window elsewhere) is already up: a rescan must not reset it.
fn known(base: usize) -> bool {
    unsafe { (*core::ptr::addr_of!(DEVS)).iter().flatten().any(|d| d.base == base) }
}

/// Every virtio-blk device of the transport: the new ones come up and
/// register as `/dev/vd<slot>`, the known ones are left alone. The number
/// of disks brought up by this call.
fn probe(api: &KernelApi) -> usize {
    let mut new = 0usize;
    transport::probe(known, |d| {
        let devs = unsafe { &mut *core::ptr::addr_of_mut!(DEVS) };
        let Some(n) = devs.iter().position(|s| s.is_none()) else {
            return;
        };
        let name = [b'v', b'd', b'a' + n as u8];
        devs[n] = Some(d);
        let rc = unsafe { (api.blk_register)(name.as_ptr(), name.len(), &OPS, n) };
        if rc < 0 {
            devs[n] = None;
            return;
        }
        new += 1;
    });
    new
}

/// Transitional virtio-blk over the legacy I/O BAR (x86_64).
#[cfg(target_arch = "x86_64")]
mod transport {
    use super::*;

    const VENDOR: u16 = 0x1AF4;
    const DEV_BLK_LEGACY: u16 = 0x1001;

    const REG_DEV_FEAT: u16 = 0;
    const REG_DRV_FEAT: u16 = 4;
    const REG_QUEUE_PFN: u16 = 8;
    const REG_QUEUE_NUM: u16 = 12;
    const REG_QUEUE_SEL: u16 = 14;
    const REG_QUEUE_NOTIFY: u16 = 16;
    const REG_STATUS: u16 = 18;
    const REG_ISR: u16 = 19;
    const REG_CONFIG: u16 = 20;

    const ACKNOWLEDGE: u8 = 1;
    const DRIVER: u8 = 2;
    const DRIVER_OK: u8 = 4;

    fn inb(port: u16) -> u8 {
        let v: u8;
        unsafe { core::arch::asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack, preserves_flags)) };
        v
    }
    fn inw(port: u16) -> u16 {
        let v: u16;
        unsafe { core::arch::asm!("in ax, dx", in("dx") port, out("ax") v, options(nomem, nostack, preserves_flags)) };
        v
    }
    fn inl(port: u16) -> u32 {
        let v: u32;
        unsafe { core::arch::asm!("in eax, dx", in("dx") port, out("eax") v, options(nomem, nostack, preserves_flags)) };
        v
    }
    fn outb(port: u16, v: u8) {
        unsafe { core::arch::asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack, preserves_flags)) };
    }
    fn outw(port: u16, v: u16) {
        unsafe { core::arch::asm!("out dx, ax", in("dx") port, in("ax") v, options(nomem, nostack, preserves_flags)) };
    }
    fn outl(port: u16, v: u32) {
        unsafe { core::arch::asm!("out dx, eax", in("dx") port, in("eax") v, options(nomem, nostack, preserves_flags)) };
    }

    pub fn notify(iobase: usize) {
        outw(iobase as u16 + REG_QUEUE_NOTIFY, 0);
        let _ = inb(iobase as u16 + REG_ISR);
    }

    /// Every transitional virtio-blk whose BAR0 is an I/O port range
    /// (modern `0x1042` devices are skipped: no MMIO driver), except the
    /// ones `known` by their port base.
    pub fn probe(known: impl Fn(usize) -> bool, mut found: impl FnMut(Dev)) {
        let api = api();
        for i in 0..MAX_DISKS as u32 {
            let (mut bus, mut slot, mut func) = (0u8, 0u8, 0u8);
            if unsafe { (api.pci_find)(VENDOR, DEV_BLK_LEGACY, i, &mut bus, &mut slot, &mut func) } != 0 {
                break;
            }
            let bar = unsafe { (api.pci_cfg_read32)(bus, slot, func, 0x10) };
            if bar == 0 || bar == 0xFFFF_FFFF || bar & 1 == 0 || known((bar & 0xFFFC) as usize) {
                continue;
            }
            // I/O space + memory space + bus master.
            let cmd = unsafe { (api.pci_cfg_read32)(bus, slot, func, 4) };
            unsafe { (api.pci_cfg_write32)(bus, slot, func, 4, cmd | 0x0007) };
            if let Some(d) = setup((bar & 0xFFFC) as u16) {
                found(d);
            }
        }
    }

    fn setup(iobase: u16) -> Option<Dev> {
        outb(iobase + REG_STATUS, 0);
        outb(iobase + REG_STATUS, ACKNOWLEDGE);
        outb(iobase + REG_STATUS, ACKNOWLEDGE | DRIVER);
        // Accept no optional features; legacy does not use FEATURES_OK.
        let _host = inl(iobase + REG_DEV_FEAT);
        outl(iobase + REG_DRV_FEAT, 0);

        outw(iobase + REG_QUEUE_SEL, 0);
        // QEMU 10+ legacy virtio-blk advertises QueueNum=1024 (older QEMU:
        // 256). Accept up to 1024; negotiating down via QueueNum writes looked
        // accepted but hung on first I/O.
        let num = inw(iobase + REG_QUEUE_NUM);
        if num == 0 || num > 1024 {
            return None;
        }
        let bytes = virtq::vring_size(num as usize, PAGE);
        let (vq_phys, vq_va) = dma_pages(bytes.div_ceil(PAGE))?;
        let (dma_phys, dma_va) = dma_page()?;
        let used_off = virtq::used_offset(num as usize, PAGE);
        let avail = unsafe { vq_va.add(num as usize * virtq::DESC_SIZE) };
        unsafe { virtq::set_avail_no_interrupt(avail) };

        virtq::dsb();
        outl(iobase + REG_QUEUE_PFN, (vq_phys / PAGE as u64) as u32);
        outb(iobase + REG_STATUS, ACKNOWLEDGE | DRIVER | DRIVER_OK);

        let lo = inl(iobase + REG_CONFIG);
        let hi = inl(iobase + REG_CONFIG + 4);
        let capacity = (u64::from(hi) << 32) | u64::from(lo);

        Some(Dev {
            ring: Ring {
                num,
                desc: vq_va,
                avail,
                used: unsafe { vq_va.add(used_off) },
                last_used: 0,
                dma_phys,
                dma_va,
            },
            capacity,
            base: iobase as usize,
        })
    }
}

/// Modern virtio-mmio v2 (aarch64, riscv64): every `virtio,mmio` node of
/// the device tree is probed for a block device (device id 2).
#[cfg(not(target_arch = "x86_64"))]
mod transport {
    use super::*;

    const MAGIC: u32 = 0x7472_6976; // "virt"
    const VERSION_2: u32 = 2;
    const DEV_BLK: u32 = 2;

    const REG_MAGIC: u32 = 0x000;
    const REG_VERSION: u32 = 0x004;
    const REG_DEVICE_ID: u32 = 0x008;
    const REG_DEV_FEAT: u32 = 0x010;
    const REG_DEV_FEAT_SEL: u32 = 0x014;
    const REG_DRV_FEAT: u32 = 0x020;
    const REG_DRV_FEAT_SEL: u32 = 0x024;
    const REG_QUEUE_SEL: u32 = 0x030;
    const REG_QUEUE_NUM_MAX: u32 = 0x034;
    const REG_QUEUE_NUM: u32 = 0x038;
    const REG_QUEUE_READY: u32 = 0x044;
    const REG_QUEUE_NOTIFY: u32 = 0x050;
    const REG_ISR: u32 = 0x060;
    const REG_ISR_ACK: u32 = 0x064;
    const REG_STATUS: u32 = 0x070;
    const REG_DESC_LO: u32 = 0x080;
    const REG_DESC_HI: u32 = 0x084;
    const REG_AVAIL_LO: u32 = 0x090;
    const REG_AVAIL_HI: u32 = 0x094;
    const REG_USED_LO: u32 = 0x0A0;
    const REG_USED_HI: u32 = 0x0A4;
    const REG_CONFIG: u32 = 0x100;

    const ACKNOWLEDGE: u32 = 1;
    const DRIVER: u32 = 2;
    const DRIVER_OK: u32 = 4;
    const FEATURES_OK: u32 = 8;
    const VIRTIO_F_VERSION_1: u32 = 1; // bit 32, in features dword 1

    fn r32(base: usize, off: u32) -> u32 {
        unsafe { core::ptr::read_volatile((base + off as usize) as *const u32) }
    }
    fn w32(base: usize, off: u32, v: u32) {
        unsafe { core::ptr::write_volatile((base + off as usize) as *mut u32, v) }
    }
    fn write_phys(base: usize, lo: u32, hi: u32, phys: u64) {
        w32(base, lo, phys as u32);
        w32(base, hi, (phys >> 32) as u32);
    }

    pub fn notify(base: usize) {
        virtq::dsb();
        w32(base, REG_QUEUE_NOTIFY, 0);
        let isr = r32(base, REG_ISR);
        if isr != 0 {
            w32(base, REG_ISR_ACK, isr);
        }
    }

    /// Every `virtio,mmio` node of the device tree that is a block device,
    /// except the ones `known` by their window base.
    pub fn probe(known: impl Fn(usize) -> bool, mut found: impl FnMut(Dev)) {
        let compat = myos_abi::StrRef {
            ptr: b"virtio,mmio".as_ptr(),
            len: b"virtio,mmio".len(),
        };
        let mut i = 0;
        loop {
            let mut node = myos_abi::MmioDevice::default();
            if unsafe { (api().dt_mmio_find)(compat, i, &mut node) } != 0 {
                break;
            }
            i += 1;
            let base = node.base;
            if known(base) || r32(base, REG_MAGIC) != MAGIC || r32(base, REG_DEVICE_ID) != DEV_BLK {
                continue;
            }
            if let Some(d) = setup(base) {
                found(d);
            }
        }
    }

    fn setup(base: usize) -> Option<Dev> {
        if r32(base, REG_VERSION) != VERSION_2 {
            return None;
        }
        w32(base, REG_STATUS, 0);
        virtq::dsb();
        w32(base, REG_STATUS, ACKNOWLEDGE);
        w32(base, REG_STATUS, ACKNOWLEDGE | DRIVER);

        w32(base, REG_DEV_FEAT_SEL, 1);
        let f1 = r32(base, REG_DEV_FEAT);
        w32(base, REG_DRV_FEAT_SEL, 0);
        w32(base, REG_DRV_FEAT, 0);
        w32(base, REG_DRV_FEAT_SEL, 1);
        w32(base, REG_DRV_FEAT, f1 & VIRTIO_F_VERSION_1);

        w32(base, REG_STATUS, ACKNOWLEDGE | DRIVER | FEATURES_OK);
        virtq::dsb();
        if r32(base, REG_STATUS) & FEATURES_OK == 0 {
            return None;
        }

        w32(base, REG_QUEUE_SEL, 0);
        let max = r32(base, REG_QUEUE_NUM_MAX);
        if max == 0 {
            return None;
        }
        let num = (max.min(128) as u16).max(1);
        w32(base, REG_QUEUE_NUM, u32::from(num));

        let (desc_phys, desc_va) = dma_page()?;
        let (avail_phys, avail_va) = dma_page()?;
        let (used_phys, used_va) = dma_page()?;
        let (dma_phys, dma_va) = dma_page()?;
        unsafe { virtq::set_avail_no_interrupt(avail_va) };

        w32(base, REG_QUEUE_READY, 0);
        write_phys(base, REG_DESC_LO, REG_DESC_HI, desc_phys);
        write_phys(base, REG_AVAIL_LO, REG_AVAIL_HI, avail_phys);
        write_phys(base, REG_USED_LO, REG_USED_HI, used_phys);
        virtq::dcache_civac(desc_va, PAGE);
        virtq::dcache_civac(avail_va, PAGE);
        virtq::dcache_civac(used_va, PAGE);
        virtq::dcache_civac(dma_va, PAGE);
        virtq::dsb();
        w32(base, REG_QUEUE_READY, 1);
        if r32(base, REG_QUEUE_READY) != 1 {
            return None;
        }
        w32(base, REG_STATUS, ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK);
        virtq::dsb();

        let lo = r32(base, REG_CONFIG);
        let hi = r32(base, REG_CONFIG + 4);
        let capacity = (u64::from(hi) << 32) | u64::from(lo);

        Some(Dev {
            ring: Ring {
                num,
                desc: desc_va,
                avail: avail_va,
                used: used_va,
                last_used: 0,
                dma_phys,
                dma_va,
            },
            capacity,
            base,
        })
    }
}

// So `cargo build --bin virtio_blk` links. The kernel never jumps here.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
