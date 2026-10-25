//! RISC-V64: Limine on QEMU `virt` (UEFI). Sv39 MMU is already on.

mod interrupts;
pub use interrupts::enable_ipi;
pub mod paging;
pub mod pci;
mod aia;
mod plic;
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

/// Add device MMIO to Limine's Sv39 root when the low slot is still free.
pub fn early_init() {
    paging::map_devices();
}

/// The MSI controller, once the root controller runs
/// (`irqchip::start_msi`): none of its own, the AIA's IMSIC is the
/// root's (`aia`), which says so here.
pub fn start_msi(_p: &crate::platform::Platform) {
    if crate::irqchip::root().name() == "aia" {
        aia::report();
    }
}

pub fn init_interrupts() {
    interrupts::init();
}

pub fn wait_for_interrupt_proof() {
    interrupts::wait_for_interrupt_proof();
}


pub fn ap_init(logical: usize) {
    interrupts::ap_init(logical);
    crate::user::ap_init();
}

/// The device tree describes the platform: the RISC-V ACPI tables (RHCT,
/// the PLIC in the MADT) are younger than the firmware that runs here.
pub const PREFER_ACPI: bool = false;

/// The interrupt controllers the kernel drives (`irqchip`).
pub static IRQCHIPS: &[&crate::irqchip::Driver] = &[&plic::DRIVER, &aia::DRIVER];

/// A riscv64 board's interrupt controller is always described.
pub const IRQCHIP_DEFAULT: Option<&crate::irqchip::Driver> = None;

/// Take the board's device bases from the platform description: the
/// interrupt controller (`IRQCHIPS`: the PLIC or the AIA),
/// the 16550 console, the goldfish RTC, the `time` CSR rate and the PCIe
/// host bridge (ECAM, bus range, 64-bit MMIO window). Required: a board
/// the description does not cover does not boot.
pub fn apply_platform(p: &crate::platform::Platform) -> Result<(), &'static str> {
    use crate::platform::UartKind;
    if p.source.is_none() {
        return Err("no device tree from the bootloader");
    }
    crate::irqchip::start(p)?;
    match p.uart.map(|c| c.value) {
        Some(uart) if uart.kind == UartKind::Ns16550 && !uart.io => {
            serial::set_base(uart.base as usize)
        }
        _ => return Err("platform: no 16550 UART"),
    }
    if let Some(rtc) = p.rtc {
        clock::set_rtc_base(rtc.value.base as usize);
    }
    let hz = p.timer_hz.ok_or("platform: no timebase-frequency")?.value;
    clock::set_timebase(hz);
    let host = p.pci.ok_or("platform: no PCIe host bridge")?.value;
    // The 64-bit window (the 32-bit one, at 0x4000_0000 on QEMU `virt`, is
    // in the user address space; a BAR there is relocated, `paging`).
    let mmio = p
        .pci_windows
        .and_then(|w| w.value.mmio64)
        .ok_or("platform: PCIe host bridge has no 64-bit MMIO range")?;
    if host.kind != crate::platform::PciHostKind::Ecam {
        return Err("platform: PCIe host bridge is not ECAM");
    }
    pci::set_host(host, mmio);
    Ok(())
}

/// Program this hart's timer for a sleep deadline sooner than its next tick.
pub fn timer_deadline(deadline_ns: u64) {
    interrupts::timer_deadline(deadline_ns);
}

/// Stop this hart's tick while it halts with nothing to run: its timer fires
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
/// says (QEMU `virt`: PLIC sources 32..35 with the standard slot swizzle).
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

/// A PCI function's interrupt: its MSI-X, else its MSI, through the
/// board's MSI controller where it has one (`msi::pci_setup`), else its
/// INTx line where the device tree routes it.
pub fn pci_irq_setup(bus: u8, slot: u8, func: u8) -> Option<crate::irq::PciIrq> {
    crate::msi::pci_setup(bus, slot, func).or_else(|| crate::irq::pci_intx_setup(bus, slot, func))
}

/// A PCI function's interrupt given back (`irq::pci_irq_disable`): once
/// `last` (no handler of `irq` left) its MSI-X or MSI off and the
/// controller's interrupt freed; else its INTx off, and the controller's
/// line masked once no handler is left on it (`last`: another slot's
/// device may share the line). An INTx line is not allocated, only its
/// handlers.
pub fn pci_irq_release(bus: u8, slot: u8, func: u8, irq: u32, last: bool) {
    if last && crate::msi::pci_release(bus, slot, func, irq) {
        return;
    }
    crate::irq::pci_intx_release(bus, slot, func);
    if last {
        crate::irqchip::mask(irq);
    }
}

pub fn wait_interrupt() {
    unsafe {
        core::arch::asm!("wfi", options(nostack, preserves_flags));
    }
}

/// Enter with SIE clear; WFI completes once an enabled interrupt is pending
/// (SIE clear or not), then set SIE so it is taken. Returns with IRQs on.
pub fn idle_wait() {
    unsafe {
        core::arch::asm!("wfi", "csrs sstatus, {}", in(reg) 1u64 << 1, options(nostack));
    }
}

/// Power off (SBI `SRST` shutdown): QEMU exits. The code is not passed on.
pub fn exit_qemu(_code: u32) {
    power::system_reset(power::SHUTDOWN);
}

pub fn halt() -> ! {
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nostack, preserves_flags));
        }
    }
}

core::arch::global_asm!(
    r#"
    .section .text._start,"ax",@progbits
    .globl _start
_start:
    call {main}
    "#,
    main = sym kernel_main_riscv64,
);

#[unsafe(no_mangle)]
extern "C" fn kernel_main_riscv64() -> ! {
    crate::kernel_main()
}
