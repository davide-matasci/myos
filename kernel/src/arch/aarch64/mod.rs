//! AArch64: Limine on QEMU `virt` (UEFI). MMU is already on.

mod bcm2836;
mod gic;
mod interrupts;
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

pub fn serial_flush_rx() {}

/// The console UART's interrupt, as the platform description gives it.
pub fn serial_irq() -> Option<u32> {
    crate::platform::get().uart.and_then(|u| u.value.irq)
}

/// Interrupt when received data waits (after its interrupt is routed).
pub fn serial_rx_irq_on() {
    serial::rx_irq_on();
}

pub const QEMU_SUCCESS: u32 = 0x10;
pub const QEMU_FAILURE: u32 = 0x11;

/// The device window (`paging`). Nothing is written to a UART before the
/// platform description names the board's (`apply_platform`).
pub fn early_init() {
    paging::map_devices();
}

pub fn init_interrupts() {
    interrupts::init();
}

pub fn wait_for_interrupt_proof() {
    interrupts::wait_for_interrupt_proof();
}


fn current_el() -> u64 {
    let el: u64;
    unsafe {
        core::arch::asm!("mrs {el}, CurrentEL", el = out(reg) el, options(nomem, nostack, preserves_flags));
    }
    (el >> 2) & 3
}

pub fn ap_init(logical: usize) {
    interrupts::ap_init(logical);
    crate::user::ap_init();
}

/// The ACPI tables describe the platform when the firmware has them (EDK2);
/// the device tree fills in the rest.
pub const PREFER_ACPI: bool = true;

/// The interrupt controllers the kernel drives (`irqchip`), in the order
/// the device tree is searched for them.
pub static IRQCHIPS: &[&crate::irqchip::Driver] = &[&gic::GICV2, &gic::GICV3, &bcm2836::DRIVER];

/// An aarch64 board's interrupt controller is always described.
pub const IRQCHIP_DEFAULT: Option<&crate::irqchip::Driver> = None;

/// Take the board's device bases from the platform description: the
/// interrupt controller (`IRQCHIPS`), the console (a PL011 or a 16550), the
/// PL031 RTC and the PCIe host bridge (ECAM, bus range, 32-bit MMIO
/// window). The interrupt controller and the console are required: a board
/// the description does not cover there does not boot.
pub fn apply_platform(p: &crate::platform::Platform) -> Result<(), &'static str> {
    let applied = apply(p);
    if applied.is_err() {
        // The fatal line goes to the board's UART if it got that far, else
        // to the one it most likely has (QEMU `virt`'s).
        serial::map_default();
    }
    applied
}

fn apply(p: &crate::platform::Platform) -> Result<(), &'static str> {
    if p.source.is_none() {
        return Err("no ACPI tables and no device tree from the bootloader");
    }
    // Each device's registers are mapped into the device window first.
    let map = |phys: u64, size: u64, what: &'static str| paging::map_mmio(phys, size).ok_or(what);
    crate::irqchip::start(p)?;
    match p.uart.map(|c| c.value) {
        Some(uart) if !uart.io => serial::set_uart(uart, map(uart.base, 0x1000, "platform: cannot map the UART")?),
        _ => return Err("platform: no memory-mapped UART"),
    }
    if let Some(rtc) = p.rtc {
        clock::set_rtc_base(map(rtc.value.base, 0x1000, "platform: cannot map the RTC")?);
    }
    // A board without a PCIe host bridge the kernel drives has no PCI.
    if let Some(host) = p.pci.map(|c| c.value) {
        let mmio = p
            .pci_windows
            .and_then(|w| w.value.mmio32)
            .ok_or("platform: PCIe host bridge has no 32-bit MMIO range")?;
        pci::set_host(host, mmio);
    }
    Ok(())
}

/// Program this CPU's timer for a sleep deadline sooner than its next tick.
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

/// A PCI function's INTx line, as the device tree's PCIe `interrupt-map`
/// says (QEMU `virt`: GIC SPIs 3..6 with the standard slot swizzle).
/// ACPI's PCI interrupt routes are x86_64's: here the device tree's
/// `interrupt-map` routes INTx (`irq::pci_intx_setup`).
pub fn pci_intx_route(_bus: u8, _device: u8, _pin: u8, _gsi: u32, _flags: u32) -> bool {
    false
}

/// [`crate::modules`]'s `phys_map`: not offered here (only the acpi
/// module's AML interpreter, x86_64's, asks).
pub fn phys_map(_phys: u64, _len: usize) -> Option<usize> {
    None
}

pub fn pci_irq_setup(bus: u8, slot: u8, func: u8) -> Option<crate::irq::PciIrq> {
    crate::irq::pci_intx_setup(bus, slot, func)
}

/// A PCI function's interrupt given back (`irq::pci_irq_disable`): its
/// INTx off. The controller's line is not allocated, only its handlers.
pub fn pci_irq_release(bus: u8, slot: u8, func: u8, _irq: u32, _last: bool) {
    crate::irq::pci_intx_release(bus, slot, func);
}

pub fn wait_interrupt() {
    unsafe {
        core::arch::asm!("wfi", options(nostack, preserves_flags));
    }
}

/// Enter with IRQs masked; WFI completes as soon as an interrupt is pending
/// (masked or not), then unmask so it is taken. Returns with IRQs on.
pub fn idle_wait() {
    unsafe {
        core::arch::asm!("wfi", "msr daifclr, #3", options(nostack));
    }
}

/// Power off (PSCI `SYSTEM_OFF`): QEMU exits. The code is not passed on.
pub fn exit_qemu(_code: u32) {
    power::psci(power::SYSTEM_OFF);
}

pub fn halt() -> ! {
    loop {
        unsafe {
            core::arch::asm!("wfe", options(nostack, preserves_flags));
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    crate::kernel_main()
}
