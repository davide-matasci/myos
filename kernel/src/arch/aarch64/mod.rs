//! AArch64: Limine on QEMU `virt` (UEFI). MMU is already on.

mod interrupts;
pub use interrupts::{ipi_reschedule, ipi_reschedule_cpu, ipi_tlb_shootdown};
mod paging;
pub mod pci;
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

/// PSCI SYSTEM_OFF. EL1 uses HVC (QEMU virt conduit); EL2 uses SMC.

pub fn ap_init(logical: usize) {
    interrupts::ap_init(logical);
    crate::user::ap_init();
}

/// QEMU `virt` wires PCIe INTA..D to GIC SPIs 3..6 (INTID 35..38) with the
/// standard slot swizzle; enable that SPI on CPU 0. Legacy INTx: the handler
/// must read the device ISR to deassert the line.
pub fn pci_irq_setup(bus: u8, slot: u8, func: u8) -> Option<crate::irq::PciIrq> {
    const VIRT_PCIE_IRQ: u32 = 3;
    let pin = (crate::pci::cfg_read32(bus, slot, func, 0x3C) >> 8) & 0xFF;
    if pin == 0 || pin > 4 {
        return None;
    }
    let line = (u32::from(slot) + pin - 1) % 4;
    let intid = 32 + VIRT_PCIE_IRQ + line;
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

pub fn exit_qemu(_code: u32) {
    let cmd: u64 = 0x8400_0008;
    unsafe {
        if current_el() >= 2 {
            core::arch::asm!("smc #0", in("x0") cmd, options(nostack));
        } else {
            core::arch::asm!("hvc #0", in("x0") cmd, options(nostack));
        }
    }
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
