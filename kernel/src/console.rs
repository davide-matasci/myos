//! Kernel console: output goes to serial and, when Limine provides one, the framebuffer.
//!
//! On real hardware you usually see the Limine framebuffer on the monitor while
//! COM1 (x86) carries the interactive shell. Without mirroring, the screen
//! stops at "Hello from myos" even though the OS keeps booting on serial.
//!
//! Boot status lines use a structured `[ TAG ] label` form. Serial is plain
//! text; the framebuffer renders tags and labels in a soft dark-theme palette.

use core::fmt::{self, Write};
use spin::{Mutex, Once};

use crate::arch::SerialPort;
use crate::framebuffer::{self, FrameBufferWriter};

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

/// Banner line (e.g. `Hello from myos`) in accent color on the framebuffer.
pub fn write_banner(s: &str) {
    SerialPort::new().write_str(s).ok();
    if let Some(fb) = FB.get() {
        fb.lock().put_str_colored(s, framebuffer::ACCENT);
    }
}

/// Dim informational text (stdin hints, etc.).
pub fn write_info(s: &str) {
    SerialPort::new().write_str(s).ok();
    if let Some(fb) = FB.get() {
        fb.lock().put_str_colored(s, framebuffer::DIM);
    }
}

/// `[ OK ] label` — tag in green on the framebuffer, plain text on serial.
pub fn status_ok(label: &str) {
    write_status("OK", framebuffer::OK, label);
}

/// `[ FAIL ] label` — tag in red on the framebuffer.
pub fn status_fail(label: &str) {
    write_status("FAIL", framebuffer::FAIL, label);
}

/// `[ .. ] label` — in-progress boot step (blue tag on the framebuffer).
pub fn status_progress(label: &str) {
    write_status("..", framebuffer::INFO, label);
}

fn write_status(tag: &str, tag_color: (u8, u8, u8), label: &str) {
    let mut serial = SerialPort::new();
    let _ = serial.write_str("[ ");
    let _ = serial.write_str(tag);
    let _ = serial.write_str(" ]");
    if !label.is_empty() {
        let _ = serial.write_str(" ");
        let _ = serial.write_str(label);
    }
    let _ = serial.write_str("\n");
    if let Some(fb) = FB.get() {
        fb.lock().put_status_line(tag, tag_color, label);
    }
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
