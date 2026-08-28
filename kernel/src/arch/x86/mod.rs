//! x86_64: Limine already dropped us in long mode with HHDM + framebuffer.

pub mod gdt;
mod interrupts;
mod serial;
pub use serial::SerialPort;

pub const QEMU_SUCCESS: u32 = 0x10;
pub const QEMU_FAILURE: u32 = 0x11;

pub fn early_init() {}

pub fn init_interrupts() {
    interrupts::init();
}

pub fn wait_for_interrupt_proof() {
    interrupts::wait_for_interrupt_proof();
}

/// QEMU `isa-debug-exit` at iobase 0xf4. A no-op if the device was not added.
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
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}
