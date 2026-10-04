//! PCI configuration space: through the ECAM window the ACPI MCFG names
//! when the firmware has one (QEMU `q35`, every PCIe PC), through ports
//! `0xCF8` / `0xCFC` otherwise (QEMU `pc`), and for every access made
//! before the page tables can grow (the heap's frames map the window, one
//! bus at a time on first use).

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;
use x86_64::instructions::port::Port;

use crate::limine_boot;

const CONFIG_ADDR: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

pub const MAX_BUS: u8 = 255;
/// Fallback MMIO window if firmware left BAR0 at 0.
pub const MMIO_ASSIGN: u64 = 0xF000_0000;

/// The ECAM window (`set_ecam`): physical base of bus `ECAM_BUS_START`, 0
/// without one.
static ECAM_PHYS: AtomicU64 = AtomicU64::new(0);
static ECAM_BUS_START: AtomicU8 = AtomicU8::new(0);
static ECAM_BUS_END: AtomicU8 = AtomicU8::new(0);
/// Page tables can be allocated: the window can be mapped.
static ECAM_READY: AtomicBool = AtomicBool::new(false);
/// Which buses' 1 MiB of the window are mapped (`MAPPED[bus / 64]`).
static MAPPED: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
static MAP_LOCK: Mutex<()> = Mutex::new(());

/// The PCIe host bridge's ECAM window from the platform description
/// (before the heap: recorded, used once `ecam_ready` runs).
pub fn set_ecam(phys: u64, bus_start: u8, bus_end: u8) {
    if phys == 0 || bus_end < bus_start {
        return;
    }
    ECAM_PHYS.store(phys, Ordering::SeqCst);
    ECAM_BUS_START.store(bus_start, Ordering::SeqCst);
    ECAM_BUS_END.store(bus_end, Ordering::SeqCst);
}

/// The frame allocator works: configuration space goes through the ECAM
/// window from here on.
pub fn ecam_ready() {
    ECAM_READY.store(true, Ordering::SeqCst);
}

/// Where configuration space comes from, for the boot log.
pub fn config_source() -> &'static str {
    if ECAM_PHYS.load(Ordering::Relaxed) != 0 {
        "ecam"
    } else {
        "port 0xcf8"
    }
}

/// The ECAM address of `bus:slot.func`'s configuration space, mapping the
/// bus's megabyte on first use. `None` keeps the access on the ports.
fn ecam(bus: u8, slot: u8, func: u8) -> Option<usize> {
    let phys = ECAM_PHYS.load(Ordering::Relaxed);
    if phys == 0 || !ECAM_READY.load(Ordering::Relaxed) {
        return None;
    }
    let start = ECAM_BUS_START.load(Ordering::Relaxed);
    if bus < start || bus > ECAM_BUS_END.load(Ordering::Relaxed) {
        return None;
    }
    let bus_phys = phys + (u64::from(bus - start) << 20);
    let (word, bit) = (&MAPPED[usize::from(bus) / 64], 1u64 << (bus % 64));
    if word.load(Ordering::Acquire) & bit == 0 {
        let _held = MAP_LOCK.lock();
        if word.load(Ordering::Acquire) & bit == 0 {
            super::paging::map_mmio(bus_phys, 1 << 20)?;
            word.fetch_or(bit, Ordering::Release);
        }
    }
    let va = (limine_boot::hhdm_offset() + bus_phys) as usize;
    Some(va + ((slot as usize) << 15) + ((func as usize) << 12))
}

/// Last bus the configuration mechanism covers.
pub fn max_bus() -> u8 {
    if ECAM_PHYS.load(Ordering::Relaxed) != 0 && ECAM_READY.load(Ordering::Relaxed) {
        ECAM_BUS_END.load(Ordering::Relaxed)
    } else {
        MAX_BUS
    }
}

/// Where BARs are assigned.
pub fn mmio_assign() -> u64 {
    MMIO_ASSIGN
}

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
    if let Some(base) = ecam(bdf.bus, bdf.slot, bdf.func) {
        return unsafe { core::ptr::read_volatile((base + (offset as usize & !3)) as *const u32) };
    }
    unsafe {
        let mut addr = Port::<u32>::new(CONFIG_ADDR);
        let mut data = Port::<u32>::new(CONFIG_DATA);
        addr.write(addr_word(bdf, offset));
        data.read()
    }
}

pub fn config_write32(bdf: Bdf, offset: u8, value: u32) {
    if let Some(base) = ecam(bdf.bus, bdf.slot, bdf.func) {
        unsafe { core::ptr::write_volatile((base + (offset as usize & !3)) as *mut u32, value) };
        return;
    }
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
