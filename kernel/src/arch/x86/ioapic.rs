//! The I/O APIC: legacy ISA interrupts (the COM1 UART's IRQ 4, the PS/2
//! keyboard's IRQ 1) to LAPIC vectors on the BSP. PCI devices use MSI-X
//! instead (`interrupts::pci_msix_setup`). Only the first I/O APIC of the
//! MADT is driven: it serves GSIs 0..23 on every PC, the ISA ones among
//! them. Without an MADT I/O APIC entry nothing is routed and the callers
//! poll their device.

use core::sync::atomic::{AtomicU8, AtomicU32, AtomicUsize, Ordering};

/// The registers' virtual address (0: no I/O APIC).
static BASE: AtomicUsize = AtomicUsize::new(0);
/// The first GSI of its redirection table.
static GSI_BASE: AtomicU32 = AtomicU32::new(0);
/// Its number of redirection entries.
static ENTRIES: AtomicU32 = AtomicU32::new(0);
/// The vector each ISA IRQ was given (0: not routed).
static ISA_VECTOR: [AtomicU8; 16] = [const { AtomicU8::new(0) }; 16];

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
    entry(irq)?;
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
/// LAPIC, physical destination mode.
pub fn unmask(irq: u8) {
    let Some((reg, low)) = entry(irq) else {
        return;
    };
    let vector = ISA_VECTOR.get(irq as usize).map_or(0, |v| v.load(Ordering::SeqCst));
    if vector == 0 {
        return;
    }
    write(reg + 1, (crate::smp::cpu_hw_id(0) as u32) << 24);
    write(reg, low | u32::from(vector));
}

fn outb(port: u16, value: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}
