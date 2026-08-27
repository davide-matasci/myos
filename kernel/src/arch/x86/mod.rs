//! x86_64: bootloader 0.11 already dropped us in long mode with a framebuffer.

mod paging;
mod serial;
pub use serial::SerialPort;

use bootloader_api::BootInfo;

pub const QEMU_SUCCESS: u32 = 0x10;
pub const QEMU_FAILURE: u32 = 0x11;

pub fn init_paging(boot_info: &'static mut BootInfo) {
    paging::init(boot_info);
}

pub fn map_writable(start: usize, size: usize) {
    paging::map_writable(start, size);
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
