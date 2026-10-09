//! x86_64: Limine already dropped us in long mode with HHDM + framebuffer.

pub mod gdt;
mod interrupts;
mod ioapic;
pub use interrupts::{ipi_reschedule, ipi_reschedule_cpu, ipi_tlb_shootdown};
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

/// COM1's interrupt: ISA IRQ 4 (see [`irq_route`]).
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

/// PCI configuration space goes through the MCFG's ECAM window when the
/// description has one; the rest needs nothing from it (the LAPIC base
/// comes from its MSR, the console from the COM1 port, configuration space
/// from port 0xCF8 without an MCFG), so a PC without ACPI tables boots too.
pub fn apply_platform(p: &crate::platform::Platform) -> Result<(), &'static str> {
    if let Some(host) = p.pci {
        pci::set_ecam(host.value.ecam, host.value.bus_start, host.value.bus_end);
    }
    Ok(())
}

/// No device-tree interrupt specifiers on x86_64.
pub fn irq_from_dt(_cells: &[u32]) -> Option<u32> {
    None
}

/// Give legacy ISA interrupt `irq` (0..15) a LAPIC vector through the I/O
/// APIC, still masked: the vector is its `irq::dispatch` number. `None`
/// without an I/O APIC.
pub fn irq_route(irq: u32) -> Option<u32> {
    ioapic::route(u8::try_from(irq).ok()?)
}

/// Unmask ISA `irq` after [`irq_route`]: delivered to the BSP.
pub fn irq_unmask(irq: u32) {
    if let Ok(irq) = u8::try_from(irq) {
        ioapic::unmask(irq);
    }
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
    let irq = interrupts::pci_msix_setup(bus, slot, func)?;
    Some(crate::irq::PciIrq {
        irq,
        msix_entry: Some(0),
    })
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
