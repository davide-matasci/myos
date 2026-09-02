//! Shared PCI scan for NVMe. Config access and BAR mapping are arch-specific.
//!
//! NVMe is class `0x01` subclass `0x08`. BAR0 is always memory (often 64-bit).
//! Device MMIO is mapped by the arch; it is not HHDM.

use crate::arch::pci::{self as arch_pci, MAX_BUS};
use crate::nvme;

const CLASS_MASS: u8 = 0x01;
const SUBCLASS_NVME: u8 = 0x08;
const MAX_CTRL: usize = nvme::MAX_CTRL;

#[derive(Clone, Copy)]
pub struct Bdf {
    pub bus: u8,
    pub slot: u8,
    pub func: u8,
}

fn read32(bdf: Bdf, offset: u8) -> u32 {
    arch_pci::cfg_read32(bdf.bus, bdf.slot, bdf.func, offset)
}

fn write32(bdf: Bdf, offset: u8, value: u32) {
    arch_pci::cfg_write32(bdf.bus, bdf.slot, bdf.func, offset, value)
}

fn vendor(bdf: Bdf) -> u16 {
    read32(bdf, 0) as u16
}

fn header_type(bdf: Bdf) -> u8 {
    (read32(bdf, 0x0C) >> 16) as u8
}

fn class_subclass(bdf: Bdf) -> (u8, u8) {
    let w = read32(bdf, 0x08);
    (((w >> 24) as u8), ((w >> 16) as u8))
}

fn enable_mem_master(bdf: Bdf) {
    let mut v = read32(bdf, 4);
    v |= 0x0006; // memory space + bus master
    write32(bdf, 4, v);
}

/// Decode BAR `index` as MMIO. Handles 64-bit BARs. Returns (phys, size).
pub fn bar_mmio(bdf: Bdf, index: u8) -> Option<(u64, u64)> {
    if index > 5 {
        return None;
    }
    let off = 0x10 + index * 4;
    let lo = read32(bdf, off);
    if lo == 0 || lo == 0xFFFF_FFFF {
        return None;
    }
    if lo & 1 != 0 {
        return None;
    }
    let is64 = (lo & 0x6) == 0x4;
    if is64 && index >= 5 {
        return None;
    }
    let orig_hi = if is64 { read32(bdf, off + 4) } else { 0 };

    write32(bdf, off, 0xFFFF_FFFF);
    if is64 {
        write32(bdf, off + 4, 0xFFFF_FFFF);
    }
    let mask_lo = read32(bdf, off);
    let mask_hi = if is64 { read32(bdf, off + 4) } else { 0 };
    write32(bdf, off, lo);
    if is64 {
        write32(bdf, off + 4, orig_hi);
    }

    let mut addr = if is64 {
        (u64::from(orig_hi) << 32) | u64::from(lo & 0xFFFF_FFF0)
    } else {
        u64::from(lo & 0xFFFF_FFF0)
    };
    let size = if is64 {
        let mask = (u64::from(mask_hi) << 32) | u64::from(mask_lo & 0xFFFF_FFF0);
        (!mask).wrapping_add(1)
    } else {
        u64::from((!(mask_lo & 0xFFFF_FFF0)).wrapping_add(1))
    };
    if size == 0 || !size.is_power_of_two() {
        return None;
    }
    if addr == 0 {
        addr = (arch_pci::MMIO_ASSIGN + size - 1) & !(size - 1);
        write32(bdf, off, (addr as u32) | (lo & 0xF));
        if is64 {
            write32(bdf, off + 4, (addr >> 32) as u32);
        }
    }
    Some((addr, size))
}

/// Walk PCI, map each NVMe BAR, and attach the shared driver.
pub fn scan_nvme() {
    let mut found = 0usize;
    for bus in 0u8..=MAX_BUS {
        for slot in 0u8..32 {
            let bdf0 = Bdf { bus, slot, func: 0 };
            if vendor(bdf0) == 0xFFFF {
                continue;
            }
            let funcs = if header_type(bdf0) & 0x80 != 0 { 8 } else { 1 };
            for func in 0..funcs {
                let bdf = Bdf { bus, slot, func };
                if vendor(bdf) == 0xFFFF {
                    continue;
                }
                let (class, sub) = class_subclass(bdf);
                if class != CLASS_MASS || sub != SUBCLASS_NVME {
                    continue;
                }
                enable_mem_master(bdf);
                let Some((phys, size)) = bar_mmio(bdf, 0) else {
                    continue;
                };
                let Some(va) = arch_pci::map_mmio(phys, size) else {
                    continue;
                };
                if nvme::attach(va) {
                    found += 1;
                    if found == MAX_CTRL {
                        return;
                    }
                }
            }
        }
    }
}
