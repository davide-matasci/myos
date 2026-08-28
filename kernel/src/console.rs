//! Kernel console: output goes to serial and, when Limine provides one, the framebuffer.
//!
//! On real hardware you usually see the Limine framebuffer on the monitor while
//! COM1 (x86) carries the interactive shell. Without mirroring, the screen
//! stops at "Hello from myos" even though the OS keeps booting on serial.

use core::fmt::{self, Write};
use spin::{Mutex, Once};

use crate::arch::SerialPort;
use crate::framebuffer::FrameBufferWriter;

static FB: Once<Mutex<FrameBufferWriter<'static>>> = Once::new();

pub fn init_fb(writer: FrameBufferWriter<'static>) {
    let _ = FB.call_once(|| Mutex::new(writer));
}

pub fn has_fb() -> bool {
    FB.get().is_some()
}

pub fn write_byte(byte: u8) {
    SerialPort::new().write_byte(byte);
    if byte == b'\r' {
        return;
    }
    if let Some(fb) = FB.get() {
        fb.lock().put_byte(byte);
    }
}

pub fn write_str(s: &str) {
    let _ = Console.write_str(s);
}

pub fn flush() {
    SerialPort::new().flush();
}

struct Console;

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        SerialPort::new().write_str(s)?;
        if let Some(fb) = FB.get() {
            fb.lock().write_str(s)?;
        }
        Ok(())
    }
}
