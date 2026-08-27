#![no_std]
#![no_main]

mod font;
mod framebuffer;
mod serial;

use bootloader_api::{entry_point, BootInfo};
use core::fmt::Write;
use core::panic::PanicInfo;

use framebuffer::FrameBufferWriter;
use serial::SerialPort;

const HELLO: &str = "Hello from myos";

/// QEMU `isa-debug-exit` success code. Absent that device, the write is ignored
/// and the kernel falls through to `hlt`.
const QEMU_SUCCESS: u32 = 0x10;
const QEMU_FAILURE: u32 = 0x11;

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    let mut serial = SerialPort::new();
    let _ = writeln!(serial, "{HELLO}");

    if let Some(fb) = boot_info.framebuffer.as_mut() {
        let mut writer = FrameBufferWriter::new(fb);
        writer.clear();
        let _ = writeln!(writer, "{HELLO}");
    } else {
        let _ = writeln!(serial, "no framebuffer");
    }

    serial.flush();
    exit_qemu(QEMU_SUCCESS);
    halt();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let mut serial = SerialPort::new();
    let _ = writeln!(serial, "panic: {info}");
    serial.flush();
    exit_qemu(QEMU_FAILURE);
    halt();
}

fn exit_qemu(code: u32) {
    // iobase=0xf4, iosize=0x04 — a no-op if the device was not added to QEMU.
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") 0xf4_u16,
            in("eax") code,
            options(nomem, nostack, preserves_flags)
        );
    }
}

fn halt() -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}
