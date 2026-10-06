//! RISC-V64: Limine on QEMU `virt` (UEFI). Sv39 MMU is already on.

mod interrupts;
pub use interrupts::{enable_ipi, ipi_reschedule, ipi_reschedule_cpu, ipi_tlb_shootdown};
pub mod paging;
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

pub const QEMU_SUCCESS: u32 = 0x10;
pub const QEMU_FAILURE: u32 = 0x11;

/// Add device MMIO to Limine's Sv39 root when the low slot is still free.
pub fn early_init() {
    paging::map_devices();
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

/// Take the board's device bases from the platform description: the PLIC,
/// the 16550 console, the goldfish RTC, the `time` CSR rate and the PCIe
/// host bridge (ECAM, bus range, 64-bit MMIO window). Required: a board
/// the description does not cover does not boot.
pub fn apply_platform(p: &crate::platform::Platform) -> Result<(), &'static str> {
    use crate::platform::{Intc, UartKind};
    if p.source.is_none() {
        return Err("no device tree from the bootloader");
    }
    match p.intc.map(|c| c.value) {
        Some(Intc::Plic { base }) => interrupts::plic::set_base(base as usize),
        _ => return Err("platform: no PLIC"),
    }
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
    // The 64-bit window: the 32-bit one (0x4000_0000 on QEMU `virt`) is
    // user address space in Sv39 root[1].
    let (mmio, mmio_size) = p
        .pci_windows
        .and_then(|w| w.value.mmio64)
        .ok_or("platform: PCIe host bridge has no 64-bit MMIO range")?;
    pci::set_host(host.ecam, host.ecam_size, host.bus_end, mmio, mmio_size);
    Ok(())
}

/// A PLIC interrupt specifier: the source number.
pub fn irq_from_dt(cells: &[u32]) -> Option<u32> {
    match cells {
        [src] if *src != 0 && *src < 1024 => Some(*src),
        _ => None,
    }
}

/// Program this hart's timer for a sleep deadline sooner than its next tick.
pub fn timer_deadline(deadline_ns: u64) {
    interrupts::timer_deadline(deadline_ns);
}

/// Route a PCI function's INTx line as the device tree's PCIe
/// `interrupt-map` says (QEMU `virt`: PLIC sources 32..35 with the
/// standard slot swizzle); enable that source for the boot hart. Legacy
/// INTx: the handler must read the device ISR to deassert the line.
pub fn pci_irq_setup(bus: u8, slot: u8, func: u8) -> Option<crate::irq::PciIrq> {
    let pin = (crate::pci::cfg_read32(bus, slot, func, 0x3C) >> 8) & 0xFF;
    if pin == 0 || pin > 4 {
        return None;
    }
    let spec = crate::dt::pci_intx(bus, slot, func, pin as u8)?;
    let src = irq_from_dt(spec.cells())?;
    interrupts::plic::enable(src);
    Some(crate::irq::PciIrq {
        irq: src,
        msix_entry: None,
    })
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
