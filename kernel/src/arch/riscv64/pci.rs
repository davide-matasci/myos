//! ECAM config on QEMU `virt`.
//!
//! ECAM is 0x30000000 (256 MiB). 32-bit PCI MMIO is 0x40000000; 64-bit BARs
//! land in the high window. `map_devices` covers ECAM; BAR pages are mapped
//! when the scan finds them.

use super::paging;

pub const MAX_BUS: u8 = 255;
/// 64-bit PCI MMIO window (avoids user `0x4000_0000`).
pub const MMIO_ASSIGN: u64 = 0x4_0000_0000;
const ECAM_BASE: usize = 0x3000_0000;

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
