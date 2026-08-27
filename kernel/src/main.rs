#![no_std]
#![no_main]

mod arch;
#[cfg(target_arch = "x86_64")]
mod font;
#[cfg(target_arch = "x86_64")]
mod framebuffer;

use core::fmt::Write;
use core::panic::PanicInfo;

use arch::SerialPort;

const HELLO: &str = "Hello from myos";

#[cfg(target_arch = "x86_64")]
bootloader_api::entry_point!(kernel_main_x86);

#[cfg(target_arch = "x86_64")]
fn kernel_main_x86(boot_info: &'static mut bootloader_api::BootInfo) -> ! {
    let mut serial = SerialPort::new();
    let _ = writeln!(serial, "{HELLO}");

    if let Some(fb) = boot_info.framebuffer.as_mut() {
        let mut writer = framebuffer::FrameBufferWriter::new(fb);
        writer.clear();
        let _ = writeln!(writer, "{HELLO}");
    } else {
        let _ = writeln!(serial, "no framebuffer");
    }

    serial.flush();
    arch::exit_qemu(arch::QEMU_SUCCESS);
    arch::halt();
}

/// Called from `_start` in `arch/aarch64/boot.rs`.
#[cfg(target_arch = "aarch64")]
pub(crate) fn kernel_main_aarch64() -> ! {
    let mut serial = SerialPort::new();
    let _ = writeln!(serial, "{HELLO}");
    serial.flush();
    arch::exit_qemu(arch::QEMU_SUCCESS);
    arch::halt();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let mut serial = SerialPort::new();
    let _ = writeln!(serial, "panic: {info}");
    serial.flush();
    arch::exit_qemu(arch::QEMU_FAILURE);
    arch::halt();
}
