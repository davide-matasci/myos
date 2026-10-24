//! The PC's interrupt controllers as one (`irqchip`): each CPU's local APIC
//! (its vectors, the IPIs, the end of interrupt: `interrupts`) and the I/O
//! APIC behind them, which gives the legacy ISA interrupts a vector
//! (`ioapic`). A PC has them whether or not ACPI describes them: the
//! local APIC's base is in its MSR.

use crate::irqchip::{Driver, IrqChip, Ipi, Target};
use crate::platform::Intc;

pub static DRIVER: Driver = Driver {
    name: "apic",
    dt: &[],
    xlate,
    start,
};

/// No device tree on a PC.
fn xlate(_node: usize, _cells: &[u32]) -> Option<u32> {
    None
}

/// Nothing to map yet: the local APIC's page needs the heap's page-table
/// frames (`interrupts::init`), the I/O APIC is the MADT's (`ioapic::init`).
fn start(_intc: &Intc) -> Result<&'static dyn IrqChip, &'static str> {
    Ok(&Apic)
}

struct Apic;

impl IrqChip for Apic {
    fn name(&self) -> &'static str {
        "apic"
    }

    fn init_cpu(&self) {
        super::interrupts::lapic_init_cpu();
    }

    /// ISA interrupt `irq` (0..15) to a LAPIC vector through the I/O APIC,
    /// still masked; `None` without an I/O APIC.
    fn route(&self, irq: u32) -> Option<u32> {
        super::ioapic::route(u8::try_from(irq).ok()?)
    }

    fn unmask(&self, irq: u32) {
        if let Ok(irq) = u8::try_from(irq) {
            super::ioapic::unmask(irq);
        }
    }

    /// The vector the CPU took names the interrupt: nothing to ask.
    fn claim(&self) -> Option<u32> {
        None
    }

    fn complete(&self, _id: u32) {
        super::interrupts::lapic_eoi();
    }

    fn send_ipi(&self, to: Target, ipi: Ipi) {
        super::interrupts::send_ipi(to, ipi);
    }
}
