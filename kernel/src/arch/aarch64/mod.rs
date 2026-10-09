//! AArch64: Limine on QEMU `virt` (UEFI). MMU is already on.

mod interrupts;
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

/// Identity-map device MMIO on TTBR0 so UART/GIC still work.
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

/// Take the board's device bases from the platform description: the GIC
/// (v2: distributor and CPU interface; v3: distributor and redistributors),
/// the console (a PL011 or a 16550), the PL031 RTC and the PCIe host bridge
/// (ECAM, bus range, 32-bit MMIO window). Required: a board the description
/// does not cover does not boot.
pub fn apply_platform(p: &crate::platform::Platform) -> Result<(), &'static str> {
    use crate::platform::Intc;
    if p.source.is_none() {
        return Err("no ACPI tables and no device tree from the bootloader");
    }
    match p.intc.map(|c| c.value) {
        Some(Intc::GicV2 { gicd, gicc }) => interrupts::set_gic(gicd as usize, gicc as usize),
        Some(Intc::GicV3 { gicd, gicr, gicr_size }) if gicr != 0 => {
            interrupts::set_gicv3(gicd as usize, gicr as usize, gicr_size as usize)
        }
        Some(Intc::GicV3 { .. }) => return Err("platform: GICv3 without a redistributor region"),
        _ => return Err("platform: no GIC"),
    }
    match p.uart.map(|c| c.value) {
        Some(uart) if !uart.io => serial::set_uart(uart),
        _ => return Err("platform: no memory-mapped UART"),
    }
    if let Some(rtc) = p.rtc {
        clock::set_rtc_base(rtc.value.base as usize);
    }
    let host = p.pci.ok_or("platform: no PCIe host bridge")?.value;
    let (mmio, mmio_size) = p
        .pci_windows
        .and_then(|w| w.value.mmio32)
        .ok_or("platform: PCIe host bridge has no 32-bit MMIO range")?;
    pci::set_host(host.ecam, host.ecam_size, host.bus_end, mmio, mmio_size);
    Ok(())
}

/// A GICv2 interrupt specifier (`type, number, flags`): SPI `n` is INTID
/// 32 + n, PPI `n` is 16 + n.
pub fn irq_from_dt(cells: &[u32]) -> Option<u32> {
    match cells {
        [0, n, _] if *n < 988 => Some(32 + n),
        [1, n, _] if *n < 16 => Some(16 + n),
        _ => None,
    }
}

/// A device's interrupt, a GIC SPI, is its own `irq::dispatch` number;
/// it stays disabled until [`irq_unmask`].
pub fn irq_route(irq: u32) -> Option<u32> {
    (32..1020).contains(&irq).then_some(irq)
}

/// Enable GIC SPI `irq`, delivered to the BSP.
pub fn irq_unmask(irq: u32) {
    interrupts::gic_enable_spi(irq);
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

/// Route a PCI function's INTx line as the device tree's PCIe
/// `interrupt-map` says (QEMU `virt`: GIC SPIs 3..6 with the standard slot
/// swizzle); enable that SPI on CPU 0. Legacy INTx: the handler must read
/// the device ISR to deassert the line.
pub fn pci_irq_setup(bus: u8, slot: u8, func: u8) -> Option<crate::irq::PciIrq> {
    let pin = (crate::pci::cfg_read32(bus, slot, func, 0x3C) >> 8) & 0xFF;
    if pin == 0 || pin > 4 {
        return None;
    }
    let spec = crate::dt::pci_intx(bus, slot, func, pin as u8)?;
    let intid = irq_from_dt(spec.cells())?;
    interrupts::gic_enable_spi(intid);
    Some(crate::irq::PciIrq {
        irq: intid,
        msix_entry: None,
    })
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
