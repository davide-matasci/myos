//! x86_64's unit, whichever the firmware describes: Intel's VT-d (the DMAR
//! table, `vtd`) or AMD's (the IVRS table, `amdvi`). The one found at init
//! answers for the arch. The two remap interrupts differently, so the
//! message formats live here, above both.

use alloc::string::String;
use core::sync::atomic::{AtomicU8, Ordering};

use super::{amdvi, vtd};

/// Who an interrupt comes from: a PCI function, or the I/O APIC.
#[derive(Clone, Copy)]
pub enum Source {
    Pci(u8, u8, u8),
    IoApic,
}

const NONE: u8 = 0;
const VTD: u8 = 1;
const AMDVI: u8 = 2;
static KIND: AtomicU8 = AtomicU8::new(NONE);

fn kind() -> u8 {
    KIND.load(Ordering::Relaxed)
}

pub fn init() {
    vtd::init();
    if vtd::active() {
        KIND.store(VTD, Ordering::Relaxed);
        return;
    }
    amdvi::init();
    if amdvi::active() {
        KIND.store(AMDVI, Ordering::Relaxed);
    }
}

pub fn attach(bus: u8, slot: u8, func: u8) -> Option<u32> {
    match kind() {
        VTD => vtd::attach(bus, slot, func),
        AMDVI => amdvi::attach(bus, slot, func),
        _ => None,
    }
}

pub fn map(domain: u32, phys: u64, len: usize) -> bool {
    match kind() {
        VTD => vtd::map(domain, phys, len),
        AMDVI => amdvi::map(domain, phys, len),
        _ => false,
    }
}

pub fn detach(bus: u8, slot: u8, func: u8) {
    match kind() {
        VTD => vtd::detach(bus, slot, func),
        AMDVI => amdvi::detach(bus, slot, func),
        _ => {}
    }
}

pub fn text() -> String {
    match kind() {
        VTD => vtd::text(),
        AMDVI => amdvi::text(),
        _ => String::from("none\n"),
    }
}

/// The MSI message (address, data) with which PCI function `bus:slot.func`
/// raises `vector` on the BSP through the unit's interrupt remapping. VT-d
/// names the entry in the address (the remappable format: bit 4, the
/// handle in bits 19:5 and 2, no data); AMD-Vi in the data (bits 10:0 the
/// index into the device's table, the address the LAPIC's). `None` where
/// messages are not remapped: the caller sends the plain one.
pub fn remap_msi(bus: u8, slot: u8, func: u8, vector: u8) -> Option<(u32, u32)> {
    let source = Source::Pci(bus, slot, func);
    match kind() {
        VTD => vtd::remap(source, vector, false).map(|h| {
            let h = u32::from(h);
            (0xFEE0_0000 | (h & 0x7fff) << 5 | (h >> 15) << 2 | 1 << 4, 0)
        }),
        AMDVI => amdvi::remap(source, vector).map(|i| (0xFEE0_0000, u32::from(i))),
        _ => None,
    }
}

/// The same for an I/O APIC pin (`level`: its trigger mode): its
/// redirection entry's high dword and the bits of the low one beside the
/// mask, polarity and trigger bits. VT-d: the remappable format (bit 48,
/// the handle in bits 63:49 and 11, the vector); AMD-Vi: the index in the
/// vector field, where the pin's message carries it (the destination
/// field unused: the entry names it).
pub fn remap_ioapic(vector: u8, level: bool) -> Option<(u32, u32)> {
    match kind() {
        VTD => vtd::remap(Source::IoApic, vector, level).map(|h| {
            let h = u32::from(h);
            ((h & 0x7fff) << 17 | 1 << 16, u32::from(vector) | (h >> 15) << 11)
        }),
        AMDVI => amdvi::remap(Source::IoApic, vector).map(|i| ((crate::smp::cpu_hw_id(0) as u32) << 24, u32::from(i))),
        _ => None,
    }
}

/// Free the remapping entry of `source` that raises `vector` (made by
/// [`remap_msi`] or [`remap_ioapic`]), for the next interrupt to reuse.
pub fn unremap(source: Source, vector: u8) {
    match kind() {
        VTD => vtd::unremap(source, vector),
        AMDVI => amdvi::unremap(source, vector),
        _ => {}
    }
}
