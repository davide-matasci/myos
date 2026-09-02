//! ECAM config on QEMU `virt` with `highmem-ecam=off`.
//!
//! Low ECAM is 0x3f000000 (16 MiB, 16 buses). PCI MMIO is 0x10000000.
//! Both sit in the identity-mapped 0x00000000–0x3fffffff device window.

use super::paging;

/// 16 MiB ECAM / 1 MiB per bus.
pub const MAX_BUS: u8 = 15;
/// 32-bit PCI MMIO (`highmem-mmio=off`).
pub const MMIO_ASSIGN: u64 = 0x1000_0000;
const ECAM_BASE: usize = 0x3F00_0000;

fn ecam(bus: u8, slot: u8, func: u8, offset: u8) -> usize {
    ECAM_BASE
        + ((bus as usize) << 20)
        + ((slot as usize) << 15)
        + ((func as usize) << 12)
        + (offset as usize & !3)
}

pub fn cfg_read32(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    unsafe { core::ptr::read_volatile(ecam(bus, slot, func, offset) as *const u32) }
}

pub fn cfg_write32(bus: u8, slot: u8, func: u8, offset: u8, value: u32) {
    unsafe { core::ptr::write_volatile(ecam(bus, slot, func, offset) as *mut u32, value) }
}

pub fn map_mmio(phys: u64, size: u64) -> Option<usize> {
    paging::map_mmio(phys, size)
}
