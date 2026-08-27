//! AArch64 QEMU `virt`: no rust-osdev bootloader, just `-kernel` into RAM.

mod boot;
mod paging;
mod serial;
pub use serial::SerialPort;

pub const QEMU_SUCCESS: u32 = 0x10;
pub const QEMU_FAILURE: u32 = 0x11;

pub fn init_paging() {
    paging::init();
}

pub fn map_writable(start: usize, size: usize) {
    paging::map_writable(start, size);
}

/// PSCI SYSTEM_OFF via HVC. QEMU `virt` uses HVC as the PSCI conduit, and
/// treating this as a shutdown makes the process exit (CI does not hang).
pub fn exit_qemu(_code: u32) {
    unsafe {
        core::arch::asm!(
            "hvc #0",
            in("x0") 0x8400_0008_u64,
            options(nomem, nostack)
        );
    }
}

pub fn halt() -> ! {
    loop {
        unsafe {
            core::arch::asm!("wfe", options(nomem, nostack, preserves_flags));
        }
    }
}
