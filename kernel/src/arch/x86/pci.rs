//! PCI config via `0xCF8` / `0xCFC`. Enough to find virtio-blk and map BAR0.

use x86_64::instructions::port::Port;

const CONFIG_ADDR: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

const VENDOR_VIRTIO: u16 = 0x1AF4;
const DEV_BLK_LEGACY: u16 = 0x1001;
const DEV_BLK_MODERN: u16 = 0x1042;

#[derive(Clone, Copy)]
pub struct Bdf {
    pub bus: u8,
    pub slot: u8,
    pub func: u8,
}

#[derive(Clone, Copy)]
pub struct Bar {
    pub io: bool,
    pub addr: u64,
}

fn addr_word(bdf: Bdf, offset: u8) -> u32 {
    0x8000_0000
        | (u32::from(bdf.bus) << 16)
        | (u32::from(bdf.slot) << 11)
        | (u32::from(bdf.func) << 8)
        | (u32::from(offset) & 0xFC)
}

pub fn config_read32(bdf: Bdf, offset: u8) -> u32 {
    unsafe {
        let mut addr = Port::<u32>::new(CONFIG_ADDR);
        let mut data = Port::<u32>::new(CONFIG_DATA);
        addr.write(addr_word(bdf, offset));
        data.read()
    }
}

pub fn config_write32(bdf: Bdf, offset: u8, value: u32) {
    unsafe {
        let mut addr = Port::<u32>::new(CONFIG_ADDR);
        let mut data = Port::<u32>::new(CONFIG_DATA);
        addr.write(addr_word(bdf, offset));
        data.write(value);
    }
}

pub fn config_read16(bdf: Bdf, offset: u8) -> u16 {
    let v = config_read32(bdf, offset & !3);
    (v >> ((offset & 3) * 8)) as u16
}

pub fn config_write16(bdf: Bdf, offset: u8, value: u16) {
    let aligned = offset & !3;
    let shift = (offset & 3) * 8;
    let mut v = config_read32(bdf, aligned);
    v &= !(0xFFFF << shift);
    v |= u32::from(value) << shift;
    config_write32(bdf, aligned, v);
}

pub fn vendor_device(bdf: Bdf) -> (u16, u16) {
    let w = config_read32(bdf, 0);
    (w as u16, (w >> 16) as u16)
}

/// Enable I/O space, memory space, and bus mastering.
pub fn enable_bus_master(bdf: Bdf) {
    let cmd = config_read16(bdf, 4);
    config_write16(bdf, 4, cmd | 0x0007);
}

pub fn bar0(bdf: Bdf) -> Option<Bar> {
    let bar = config_read32(bdf, 0x10);
    if bar == 0 || bar == 0xFFFF_FFFF {
        return None;
    }
    if bar & 1 != 0 {
        Some(Bar {
            io: true,
            addr: u64::from(bar & 0xFFFC),
        })
    } else {
        let addr = u64::from(bar & 0xFFFF_FFF0);
        Some(Bar { io: false, addr })
    }
}

/// Scan for virtio-blk: vendor `0x1AF4`, device `0x1001` (legacy/transitional)
/// or `0x1042` (modern 1.0). Prefers `0x1001` if both exist.
pub fn find_virtio_blk() -> Option<(Bdf, u16)> {
    let mut modern = None;
    for bus in 0u8..=255 {
        for slot in 0u8..32 {
            let bdf0 = Bdf { bus, slot, func: 0 };
            let (vendor, _) = vendor_device(bdf0);
            if vendor == 0xFFFF {
                continue;
            }
            let header = (config_read32(bdf0, 0x0C) >> 16) as u8;
            let funcs = if header & 0x80 != 0 { 8 } else { 1 };
            for func in 0..funcs {
                let bdf = Bdf { bus, slot, func };
                let (v, d) = vendor_device(bdf);
                if v != VENDOR_VIRTIO {
                    continue;
                }
                if d == DEV_BLK_LEGACY {
                    return Some((bdf, d));
                }
                if d == DEV_BLK_MODERN && modern.is_none() {
                    modern = Some((bdf, d));
                }
            }
        }
    }
    modern
}
