//! PCI config via `0xCF8` / `0xCFC`.

use x86_64::instructions::port::Port;

const CONFIG_ADDR: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

pub const MAX_BUS: u8 = 255;
/// Fallback MMIO window if firmware left BAR0 at 0.
pub const MMIO_ASSIGN: u64 = 0xF000_0000;

#[derive(Clone, Copy)]
pub struct Bdf {
    pub bus: u8,
    pub slot: u8,
    pub func: u8,
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

pub fn cfg_read32(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    config_read32(Bdf { bus, slot, func }, offset)
}

pub fn cfg_write32(bus: u8, slot: u8, func: u8, offset: u8, value: u32) {
    config_write32(Bdf { bus, slot, func }, offset, value)
}

pub fn map_mmio(phys: u64, size: u64) -> Option<usize> {
    super::paging::map_mmio(phys, size)
}
