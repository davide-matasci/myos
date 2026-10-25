//! The I/O APIC: legacy ISA interrupts (the COM1 UART's IRQ 4, the PS/2
//! keyboard's IRQ 1) to LAPIC vectors on the BSP, and the PCI INTx lines
//! of the functions with neither MSI-X nor MSI ([`route_gsi`], the GSIs
//! the firmware's `_PRT` gives: `intx`). Only the first I/O APIC of the
//! MADT is driven: it serves GSIs 0..23 on every PC, the ISA ones among
//! them. Without an MADT I/O APIC entry nothing is routed and the callers
//! poll their device.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// The registers' virtual address (0: no I/O APIC).
static BASE: AtomicUsize = AtomicUsize::new(0);
/// The first GSI of its redirection table.
static GSI_BASE: AtomicU32 = AtomicU32::new(0);
/// Its number of redirection entries.
static ENTRIES: AtomicU32 = AtomicU32::new(0);
/// The vector each ISA IRQ was given (0: not routed).
static ISA_VECTOR: [AtomicU8; 16] = [const { AtomicU8::new(0) }; 16];
/// Each ISA IRQ's remapped redirection entry (the high dword in bits
/// 62:32, the low one's bits in 31:0, bit 63 set; 0: none yet), and
/// whether its pin is unmasked (for [`refresh`]).
static ISA_REMAP: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];
static ISA_UNMASKED: [AtomicBool; 16] = [const { AtomicBool::new(false) }; 16];
/// The pins a PCI GSI was routed to ([`route_gsi`]), by pin: their vector
/// (0: none), the entry's trigger and polarity bits, the remapped entry
/// (as `ISA_REMAP`), and whether the pin is unmasked. A line its devices
/// share has one vector, every handler of it called (`irq::register`).
const PCI_PINS: usize = 64;
static PCI_VECTOR: [AtomicU8; PCI_PINS] = [const { AtomicU8::new(0) }; PCI_PINS];
static PCI_LOW: [AtomicU32; PCI_PINS] = [const { AtomicU32::new(0) }; PCI_PINS];
static PCI_REMAP: [AtomicU64; PCI_PINS] = [const { AtomicU64::new(0) }; PCI_PINS];
static PCI_UNMASKED: [AtomicBool; PCI_PINS] = [const { AtomicBool::new(false) }; PCI_PINS];

const IOREGSEL: usize = 0x00;
const IOWIN: usize = 0x10;
const IOAPICVER: u32 = 0x01;
const IOREDTBL: u32 = 0x10;

const RED_ACTIVE_LOW: u32 = 1 << 13;
const RED_LEVEL: u32 = 1 << 15;
const RED_MASKED: u32 = 1 << 16;

fn read(reg: u32) -> u32 {
    let b = BASE.load(Ordering::Relaxed);
    unsafe {
        core::ptr::write_volatile((b + IOREGSEL) as *mut u32, reg);
        core::ptr::read_volatile((b + IOWIN) as *const u32)
    }
}

fn write(reg: u32, val: u32) {
    let b = BASE.load(Ordering::Relaxed);
    unsafe {
        core::ptr::write_volatile((b + IOREGSEL) as *mut u32, reg);
        core::ptr::write_volatile((b + IOWIN) as *mut u32, val);
    }
}

/// Map the MADT's I/O APIC, mask every pin and the 8259 PICs (Limine leaves
/// them masked; firmware need not). After the heap: the mapping may need
/// page-table frames.
pub fn init() {
    // The 8259s' data ports: every line masked.
    outb(0x21, 0xFF);
    outb(0xA1, 0xFF);
    let Some((phys, gsi_base)) = crate::acpi::madt().and_then(|m| m.ioapic) else {
        return;
    };
    let Some(va) = super::paging::map_mmio(phys, 0x20) else {
        return;
    };
    BASE.store(va, Ordering::SeqCst);
    GSI_BASE.store(gsi_base, Ordering::SeqCst);
    let entries = ((read(IOAPICVER) >> 16) & 0xFF) + 1;
    ENTRIES.store(entries, Ordering::SeqCst);
    for i in 0..entries {
        write(IOREDTBL + 2 * i, RED_MASKED);
        write(IOREDTBL + 2 * i + 1, 0);
    }
    crate::console::status_info(&alloc::format!("ioapic: {entries} pins, gsi base {gsi_base}"));
}

/// The redirection entry of ISA `irq`'s GSI, if this I/O APIC serves it.
fn entry(irq: u8) -> Option<(u32, u32)> {
    if BASE.load(Ordering::Relaxed) == 0 {
        return None;
    }
    let (gsi, flags) = crate::acpi::isa_irq(irq);
    let pin = gsi.checked_sub(GSI_BASE.load(Ordering::Relaxed))?;
    if pin >= ENTRIES.load(Ordering::Relaxed) {
        return None;
    }
    // MPS INTI flags: polarity 0b11 active low, trigger 0b11 level; the
    // ISA default (0b00) is active high, edge.
    let mut low = 0;
    if flags & 0b11 == 0b11 {
        low |= RED_ACTIVE_LOW;
    }
    if (flags >> 2) & 0b11 == 0b11 {
        low |= RED_LEVEL;
    }
    Some((IOREDTBL + 2 * pin, low))
}

/// Give ISA `irq` a device vector, its pin still masked: the vector is its
/// `irq::dispatch` number. `None` without an I/O APIC serving it, or out of
/// vectors.
pub fn route(irq: u8) -> Option<u32> {
    if irq >= 16 {
        return None;
    }
    let (reg, _) = entry(irq)?;
    // A pin a PCI line has already: not shared with an ISA one.
    if PCI_VECTOR.get(((reg - IOREDTBL) / 2) as usize).is_some_and(|v| v.load(Ordering::SeqCst) != 0) {
        return None;
    }
    // The destination field has 8 bits: a BSP with a larger x2APIC id
    // would need interrupt remapping.
    if crate::smp::cpu_hw_id(0) > 0xFF {
        return None;
    }
    let vector = match ISA_VECTOR[irq as usize].load(Ordering::SeqCst) {
        0 => {
            let v = super::interrupts::alloc_vector()?;
            ISA_VECTOR[irq as usize].store(v, Ordering::SeqCst);
            v
        }
        v => v,
    };
    Some(u32::from(vector))
}

/// Unmask ISA `irq`'s pin (after [`route`]): fixed delivery to the BSP's
/// LAPIC, physical destination mode; through the IOMMU's interrupt
/// remapping where there is one (the entry names the pin's remapping
/// entry the unit's way, `iommu::remap_ioapic`).
pub fn unmask(irq: u8) {
    let Some((reg, low)) = entry(irq) else {
        return;
    };
    let vector = ISA_VECTOR.get(irq as usize).map_or(0, |v| v.load(Ordering::SeqCst));
    if vector == 0 {
        return;
    }
    ISA_UNMASKED[irq as usize].store(true, Ordering::SeqCst);
    program(reg, low, vector, &ISA_REMAP[irq as usize]);
}

/// Mask ISA `irq`'s pin again (its last handler went). Its vector and
/// remapping entry stay the pin's, for the next [`unmask`]: at most one
/// per ISA IRQ.
pub fn mask(irq: u8) {
    if irq >= 16 {
        return;
    }
    let Some((reg, low)) = entry(irq) else {
        return;
    };
    ISA_UNMASKED[irq as usize].store(false, Ordering::SeqCst);
    write(reg, low | RED_MASKED);
}

/// Write redirection entry `reg` for `vector` with trigger and polarity
/// `low`, unmasked: fixed delivery to the BSP's LAPIC, physical
/// destination mode, or through the IOMMU's interrupt remapping where
/// there is one (its entry made once, kept in `remap`).
fn program(reg: u32, low: u32, vector: u8, remap: &AtomicU64) {
    let remapped = match remap.load(Ordering::SeqCst) {
        0 => crate::iommu::remap_ioapic(vector, low & RED_LEVEL != 0).inspect(|&(high, bits)| {
            remap.store(1 << 63 | u64::from(high) << 32 | u64::from(bits), Ordering::SeqCst);
        }),
        v => Some(((v >> 32) as u32 & 0x7fff_ffff, v as u32)),
    };
    match remapped {
        Some((high, bits)) => {
            write(reg + 1, high);
            write(reg, low | bits);
        }
        None => {
            write(reg + 1, (crate::smp::cpu_hw_id(0) as u32) << 24);
            write(reg, low | u32::from(vector));
        }
    }
}

/// The pin of GSI `gsi`, if this I/O APIC serves it.
fn gsi_pin(gsi: u32) -> Option<usize> {
    if BASE.load(Ordering::Relaxed) == 0 {
        return None;
    }
    let pin = gsi.checked_sub(GSI_BASE.load(Ordering::Relaxed))?;
    (pin < ENTRIES.load(Ordering::Relaxed) && (pin as usize) < PCI_PINS).then_some(pin as usize)
}

/// Give PCI GSI `gsi` (a level-triggered, active-low line: `level`,
/// `active_low`) a device vector, its pin still masked; the line's devices
/// share it. `None` without an I/O APIC pin for it, a pin an ISA IRQ has,
/// a line routed already with another trigger, or out of vectors.
pub fn route_gsi(gsi: u32, level: bool, active_low: bool) -> Option<u32> {
    let pin = gsi_pin(gsi)?;
    // The destination field has 8 bits, as for an ISA line.
    if crate::smp::cpu_hw_id(0) > 0xFF {
        return None;
    }
    let isa = (0..16u8).any(|irq| {
        ISA_VECTOR[irq as usize].load(Ordering::SeqCst) != 0 && crate::acpi::isa_irq(irq).0 == gsi
    });
    if isa {
        return None;
    }
    let low = if level { RED_LEVEL } else { 0 } | if active_low { RED_ACTIVE_LOW } else { 0 };
    match PCI_VECTOR[pin].load(Ordering::SeqCst) {
        0 => {
            let v = super::interrupts::alloc_vector()?;
            PCI_LOW[pin].store(low, Ordering::SeqCst);
            PCI_VECTOR[pin].store(v, Ordering::SeqCst);
            Some(u32::from(v))
        }
        v if PCI_LOW[pin].load(Ordering::SeqCst) == low => Some(u32::from(v)),
        _ => None,
    }
}

/// Unmask PCI GSI `gsi`'s pin (after [`route_gsi`]).
pub fn unmask_gsi(gsi: u32) {
    let Some(pin) = gsi_pin(gsi) else {
        return;
    };
    let vector = PCI_VECTOR[pin].load(Ordering::SeqCst);
    if vector == 0 {
        return;
    }
    PCI_UNMASKED[pin].store(true, Ordering::SeqCst);
    program(IOREDTBL + 2 * pin as u32, PCI_LOW[pin].load(Ordering::SeqCst), vector, &PCI_REMAP[pin]);
}

/// The PCI line whose pin raises `vector` given back (its last handler
/// gone): the pin masked, the IOMMU's remapping entry for it and the
/// vector freed. Whether a pin had it.
pub fn release_vector(vector: u8) -> bool {
    let Some(pin) = (0..PCI_PINS).find(|&p| vector != 0 && PCI_VECTOR[p].load(Ordering::SeqCst) == vector) else {
        return false;
    };
    let reg = IOREDTBL + 2 * pin as u32;
    write(reg, read(reg) | RED_MASKED);
    PCI_UNMASKED[pin].store(false, Ordering::SeqCst);
    if PCI_REMAP[pin].swap(0, Ordering::SeqCst) != 0 {
        crate::iommu::unremap_ioapic(vector);
    }
    PCI_VECTOR[pin].store(0, Ordering::SeqCst);
    PCI_LOW[pin].store(0, Ordering::SeqCst);
    super::interrupts::free_vector(vector);
    true
}

/// Write every unmasked pin's entry again: interrupt remapping came on
/// after it was routed (the console's pin).
pub fn refresh() {
    for irq in 0..16u8 {
        if ISA_UNMASKED[irq as usize].load(Ordering::SeqCst) {
            unmask(irq);
        }
    }
    let base = GSI_BASE.load(Ordering::Relaxed);
    for pin in 0..PCI_PINS {
        if PCI_UNMASKED[pin].load(Ordering::SeqCst) {
            unmask_gsi(base + pin as u32);
        }
    }
}

fn outb(port: u16, value: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}
