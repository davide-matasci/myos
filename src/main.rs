#![no_std]
#![no_main]

mod vga_buffer;

use core::fmt::Write;
use core::panic::PanicInfo;

use vga_buffer::Writer;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let mut vga = Writer::new();
    let _ = writeln!(vga, "Hello from myos");
    halt();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let mut vga = Writer::new();
    let _ = writeln!(vga, "panic: {info}");
    halt();
}

fn halt() -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}
