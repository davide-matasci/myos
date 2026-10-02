//! PCI configuration access for the module ABI (`pci_find`, `pci_find_class`,
//! BAR mapping, bus-master enable). Config access and MMIO mapping are
//! arch-specific; device MMIO is mapped by the arch, it is not HHDM. The
//! drivers themselves (virtio-blk, NVMe, virtio-net) are modules.

use crate::arch::pci as arch_pci;

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
        addr = (arch_pci::mmio_assign() + size - 1) & !(size - 1);
        write32(bdf, off, (addr as u32) | (lo & 0xF));
        if is64 {
            write32(bdf, off + 4, (addr >> 32) as u32);
        }
    }
    Some((addr, size))
}

pub fn cfg_read32(bus: u8, slot: u8, func: u8, off: u8) -> u32 {
    read32(Bdf { bus, slot, func }, off)
}

pub fn cfg_write32(bus: u8, slot: u8, func: u8, off: u8, val: u32) {
    write32(Bdf { bus, slot, func }, off, val)
}

pub fn enable(bus: u8, slot: u8, func: u8) {
    enable_mem_master(Bdf { bus, slot, func })
}

/// Map BAR `bar` as MMIO. Returns (VA, size).
pub fn bar_map(bus: u8, slot: u8, func: u8, bar: u8) -> Option<(usize, u64)> {
    let bdf = Bdf { bus, slot, func };
    let (phys, size) = bar_mmio(bdf, bar)?;
    let va = arch_pci::map_mmio(phys, size)?;
    Some((va, size))
}

/// Nth PCI function matching `vend`/`dev` (0-based), walking like NVMe scan.
pub fn find(vend: u16, dev: u16, nth: u32) -> Option<Bdf> {
    let mut seen = 0u32;
    for bus in 0u8..=arch_pci::max_bus() {
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
                let id = read32(bdf, 0);
                let v = id as u16;
                let d = (id >> 16) as u16;
                if v != vend || d != dev {
                    continue;
                }
                if seen == nth {
                    return Some(bdf);
                }
                seen += 1;
            }
        }
    }
    None
}

/// Nth PCI function (0-based) of `class` / `subclass` (NVMe: 0x01 / 0x08).
pub fn find_class(class: u8, subclass: u8, nth: u32) -> Option<Bdf> {
    let mut seen = 0u32;
    for bus in 0u8..=arch_pci::max_bus() {
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
                if class_subclass(bdf) != (class, subclass) {
                    continue;
                }
                if seen == nth {
                    return Some(bdf);
                }
                seen += 1;
            }
        }
    }
    None
}
