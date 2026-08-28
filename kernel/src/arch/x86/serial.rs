//! COM1 (0x3F8) UART. Enough to print a line to QEMU `-serial stdio`.

use core::fmt;

const COM1: u16 = 0x3F8;

pub struct SerialPort;

impl SerialPort {
    pub fn new() -> Self {
        // 38400 8N1, FIFO on. Harmless if QEMU already set the port up.
        outb(COM1 + 1, 0x00); // disable interrupts
        outb(COM1 + 3, 0x80); // enable DLAB
        outb(COM1 + 0, 0x03); // divisor 3 → 38400 baud
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03); // 8N1
        outb(COM1 + 2, 0xC7); // FIFO on
        outb(COM1 + 4, 0x0B); // RTS/DSR
        Self
    }

    pub fn write_byte(&mut self, byte: u8) {
        if byte == b'\n' {
            self.write_byte_raw(b'\r');
        }
        self.write_byte_raw(byte);
    }

    fn write_byte_raw(&mut self, byte: u8) {
        while inb(COM1 + 5) & 0x20 == 0 {}
        outb(COM1, byte);
    }

    pub fn flush(&mut self) {
        while inb(COM1 + 5) & 0x40 == 0 {}
    }
}

/// Non-blocking read from COM1. Returns `None` if the RX FIFO is empty.
pub fn read_byte() -> Option<u8> {
    let status = inb(COM1 + 5);
    if status & 0x01 == 0 {
        return None;
    }
    // Drop break/framing/parity/overrun garbage (common with no cable attached).
    if status & 0x1E != 0 {
        let _ = inb(COM1);
        return None;
    }
    let b = inb(COM1);
    // Floating RX lines often read 0xFF; never treat as input.
    if b == 0xFF {
        return None;
    }
    Some(b)
}

/// Discard anything already in the RX FIFO (call once during boot).
pub fn flush_rx() {
    for _ in 0..256 {
        if inb(COM1 + 5) & 0x01 == 0 {
            return;
        }
        let _ = inb(COM1);
    }
}

impl fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.write_byte(byte);
        }
        Ok(())
    }
}

#[inline]
fn outb(port: u16, value: u8) {
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}

#[inline]
fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!(
            "in al, dx",
            in("dx") port,
            out("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}
