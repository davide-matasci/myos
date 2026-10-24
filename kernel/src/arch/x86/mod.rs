//! x86_64: Limine already dropped us in long mode with HHDM + framebuffer.

mod apic;
pub mod gdt;
mod interrupts;
mod ioapic;
mod paging;
pub mod pci;
mod power;
pub use power::POWER_METHODS;
mod serial;
pub use serial::SerialPort;

pub mod clock;
mod cpu;
pub use cpu::*;
pub mod elf;
mod exception;
pub mod fpu;
pub mod switch;
pub mod tp;
mod smp;
pub use smp::*;
pub mod upaging;
mod user;
pub use user::*;

pub fn serial_read_byte() -> Option<u8> {
    serial::read_byte()
}

pub fn serial_flush_rx() {
    serial::flush_rx();
}

/// COM1's interrupt: ISA IRQ 4 (routed by the I/O APIC, `apic`).
pub fn serial_irq() -> Option<u32> {
    Some(4)
}

/// Interrupt when received data waits (after its IRQ is routed).
pub fn serial_rx_irq_on() {
    serial::rx_irq_on();
}

pub const QEMU_SUCCESS: u32 = 0x10;
pub const QEMU_FAILURE: u32 = 0x11;

pub fn early_init() {}

/// After the heap: the ECAM window's page tables need frames.
pub fn init_interrupts() {
    pci::ecam_ready();
    crate::console::status_info(&alloc::format!("pci config: {}", pci::config_source()));
    interrupts::init();
    ioapic::init();
}

pub fn wait_for_interrupt_proof() {
    interrupts::wait_for_interrupt_proof();
}

pub fn ap_init(logical: usize) {
    interrupts::ap_init(logical);
    crate::user::ap_init();
}

/// ACPI describes the platform; there is no device tree.
pub const PREFER_ACPI: bool = true;

/// The interrupt controllers the kernel drives (`irqchip`).
pub static IRQCHIPS: &[&crate::irqchip::Driver] = &[&apic::DRIVER];

/// Every PC has its APICs, described by the MADT or not.
pub const IRQCHIP_DEFAULT: Option<&crate::irqchip::Driver> = Some(&apic::DRIVER);

/// PCI configuration space goes through the MCFG's ECAM window when the
/// description has one; the rest needs nothing from it (the LAPIC base
/// comes from its MSR, the console from the COM1 port, configuration space
/// from port 0xCF8 without an MCFG), so a PC without ACPI tables boots too.
pub fn apply_platform(p: &crate::platform::Platform) -> Result<(), &'static str> {
    crate::irqchip::start(p)?;
    if let Some(host) = p.pci {
        pci::set_ecam(host.value.base, host.value.bus_start, host.value.bus_end);
    }
    Ok(())
}

/// Fire this CPU's timer at a sleep's deadline when it is before the next tick.
pub fn timer_deadline(deadline_ns: u64) {
    interrupts::timer_deadline(deadline_ns);
}

/// Stop this CPU's tick while it halts with nothing to run: its timer fires
/// once, at `wake_ns`. Interrupts off.
pub fn timer_idle(wake_ns: u64) {
    interrupts::timer_idle(wake_ns);
}

/// The tick back after [`timer_idle`]; a no-op when it was not stopped.
/// Interrupts off.
pub fn timer_resume() {
    interrupts::timer_resume();
}

/// Route a PCI function's interrupt: MSI-X entry 0 → a LAPIC vector on the
/// BSP (no IOAPIC / PIRQ routing needed).
pub fn pci_irq_setup(bus: u8, slot: u8, func: u8) -> Option<crate::irq::PciIrq> {
    use crate::irq::{Delivery, PciIrq};
    if let Some(irq) = interrupts::pci_msix_setup(bus, slot, func) {
        return Some(PciIrq { irq, delivery: Delivery::MsiX(0) });
    }
    let irq = interrupts::pci_msi_setup(bus, slot, func)?;
    Some(PciIrq { irq, delivery: Delivery::Msi })
}

/// Undo [`pci_irq_setup`] (its handler is gone): the function's message
/// off, its vector and IOMMU remapping entry given back. Every MSI has a
/// vector of its own, so `_last` is always true.
pub fn pci_irq_teardown(bus: u8, slot: u8, func: u8, irq: u32, _last: bool) {
    if let Ok(vector) = u8::try_from(irq) {
        interrupts::pci_msi_teardown(bus, slot, func, vector);
    }
}

/// The device vectors taken, and how many there are.
pub fn irq_vectors() -> (u32, u32) {
    interrupts::vectors()
}

/// A device vector (its `irq::dispatch` number) and the MSI message address
/// that raises it on the BSP, for a device's own message register.
pub fn msi_vector() -> Option<(u32, u32)> {
    interrupts::alloc_msi()
}

/// The local APICs run in x2APIC mode (32-bit ids).
pub fn x2apic_mode() -> bool {
    interrupts::x2apic_mode()
}

/// Write the I/O APIC pins routed so far again: interrupt remapping came
/// on, their entries take the remappable format now.
pub fn ioapic_refresh() {
    ioapic::refresh();
}

/// Brief halt until the next interrupt (idle loop).
pub fn wait_interrupt() {
    unsafe {
        core::arch::asm!("sti; hlt", options(nostack));
    }
}

/// Enter with IRQs off; sleep until an interrupt is pending (one that is
/// already pending ends the halt at once); return with IRQs on.
pub fn idle_wait() {
    unsafe {
        core::arch::asm!("sti; hlt", options(nostack));
    }
}

/// QEMU `isa-debug-exit` at iobase 0xf4 (the test runs: its exit status is
/// `code << 1 | 1`). A no-op if the device was not added.
pub fn exit_qemu(code: u32) {
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") 0xf4_u16,
            in("eax") code,
            options(nomem, nostack, preserves_flags)
        );
    }
}

pub fn halt() -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, preserves_flags));
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    crate::kernel_main()
}
