//! ECAM config access and BAR mapping, with the host bridge's ECAM window,
//! bus range and 32-bit MMIO window from the device tree (`set_host`; QEMU
//! `virt` with `highmem-ecam=off`: ECAM 0x3f000000 for 16 buses, MMIO at
//! 0x10000000, both in the identity-mapped low 1 GiB).

use core::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};

use super::paging;

static ECAM_BASE: AtomicUsize = AtomicUsize::new(0);
static BUS_END: AtomicU8 = AtomicU8::new(0);
static MMIO_BASE: AtomicU64 = AtomicU64::new(0);
static MMIO_SIZE: AtomicU64 = AtomicU64::new(0);

/// The PCIe host bridge from the device tree: ECAM window (mapped here),
/// last bus, and the MMIO window BARs are assigned from.
pub fn set_host(ecam_phys: u64, ecam_size: u64, bus_end: u8, mmio_base: u64, mmio_size: u64) {
    let va = map_mmio(ecam_phys, ecam_size.max(1)).unwrap_or(ecam_phys as usize);
    ECAM_BASE.store(va, Ordering::SeqCst);
    // One bus takes 1 MiB of ECAM: never read past the window.
    let by_size = (ecam_size >> 20).saturating_sub(1).min(255) as u8;
    BUS_END.store(bus_end.min(by_size), Ordering::SeqCst);
    MMIO_BASE.store(mmio_base, Ordering::SeqCst);
    MMIO_SIZE.store(mmio_size, Ordering::SeqCst);
}

/// Last bus the ECAM window covers.
pub fn max_bus() -> u8 {
    BUS_END.load(Ordering::Relaxed)
}

/// Where BARs are assigned (the 32-bit MMIO window).
pub fn mmio_assign() -> u64 {
    MMIO_BASE.load(Ordering::Relaxed)
}

fn ecam(bus: u8, slot: u8, func: u8, offset: u8) -> usize {
    ECAM_BASE.load(Ordering::Relaxed)
        + ((bus as usize) << 20)
        + ((slot as usize) << 15)
        + ((func as usize) << 12)
        + (offset as usize & !3)
}

pub fn cfg_read32(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    if bus > max_bus() || ECAM_BASE.load(Ordering::Relaxed) == 0 {
        return 0xFFFF_FFFF;
    }
    unsafe { core::ptr::read_volatile(ecam(bus, slot, func, offset) as *const u32) }
}

pub fn cfg_write32(bus: u8, slot: u8, func: u8, offset: u8, value: u32) {
    if bus > max_bus() || ECAM_BASE.load(Ordering::Relaxed) == 0 {
        return;
    }
    unsafe { core::ptr::write_volatile(ecam(bus, slot, func, offset) as *mut u32, value) }
}

pub fn map_mmio(phys: u64, size: u64) -> Option<usize> {
    paging::map_mmio(phys, size)
}
