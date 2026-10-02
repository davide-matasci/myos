//! RISC-V64: Limine on QEMU `virt` (UEFI). Sv39 MMU is already on.

mod interrupts;
pub use interrupts::{enable_ipi, ipi_reschedule, ipi_reschedule_cpu, ipi_tlb_shootdown};
pub mod paging;
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


/// SBI System Reset extension shutdown (QEMU virt).

pub fn ap_init(logical: usize) {
    interrupts::ap_init(logical);
    crate::user::ap_init();
}

/// QEMU `virt` wires PCIe INTA..D to PLIC sources 32..35 with the standard
/// slot swizzle; enable that source for the boot hart. Legacy INTx: the
/// handler must read the device ISR to deassert the line.
pub fn pci_irq_setup(bus: u8, slot: u8, func: u8) -> Option<crate::irq::PciIrq> {
    const VIRT_PCIE_IRQ: u32 = 0x20;
    let pin = (crate::pci::cfg_read32(bus, slot, func, 0x3C) >> 8) & 0xFF;
    if pin == 0 || pin > 4 {
        return None;
    }
    let line = (u32::from(slot) + pin - 1) % 4;
    let src = VIRT_PCIE_IRQ + line;
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

pub fn exit_qemu(_code: u32) {
    const SBI_SRST: u64 = 0x5352_5354; // "SRST"
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") SBI_SRST,
            in("a6") 0u64,
            in("a0") 0u64,
            in("a1") 0u64,
            options(nomem, nostack),
        );
    }
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
