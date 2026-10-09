//! COM1 (0x3F8) UART. Enough to print a line to QEMU `-serial stdio`.

use core::fmt;

const COM1: u16 = 0x3F8;

pub struct SerialPort;

/// Set once the line is programmed. `SerialPort::new()` used to reprogram
/// the UART on every call — and the console constructs one per output byte.
/// Besides eight port writes per character under TCG, the FCR write (bit 1:
/// reset RX FIFO) discarded whatever input had arrived since the last drain:
/// typing while the kernel echoed lost whole 14-byte FIFO batches once the
/// drain tick dropped from 10 kHz to 1 kHz (`cat /proc/cpuinfo` → `cpuinf`).
static INITIALIZED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

impl SerialPort {
    pub fn new() -> Self {
        use core::sync::atomic::Ordering;
        if !INITIALIZED.swap(true, Ordering::AcqRel) {
            // 38400 8N1, FIFO on. Harmless if QEMU already set the port up.
            outb(COM1 + 1, 0x00); // disable interrupts
            outb(COM1 + 3, 0x80); // enable DLAB
            outb(COM1 + 0, 0x03); // divisor 3 → 38400 baud
            outb(COM1 + 1, 0x00);
            outb(COM1 + 3, 0x03); // 8N1
            outb(COM1 + 2, 0xC7); // FIFO on, RX/TX FIFO reset, trigger 14
            outb(COM1 + 4, 0x0B); // RTS/DSR
        }
        Self
    }

    pub fn write_byte(&mut self, byte: u8) {
        // UART ONLCR: turn LF into CRLF for serial terminals. If the writer
        // already sent CR (oksh emacs historically emitted CR+LF), do not inject
        // another CR — that became CR CR LF, which the boot test host normalizes to a blank
        // line and flakes the histrecall seed needle.
        // Also coalesce runs of CR: two writer CRs before LF used to reach
        // the wire as `\r\r\n` (LAST_WAS_CR only suppressed the *injected* CR).
        use core::sync::atomic::{AtomicBool, Ordering};
        static LAST_WAS_CR: AtomicBool = AtomicBool::new(false);
        if byte == b'\n' {
            if !LAST_WAS_CR.swap(false, Ordering::Relaxed) {
                self.write_byte_raw(b'\r');
            }
            self.write_byte_raw(b'\n');
        } else if byte == b'\r' {
            if !LAST_WAS_CR.swap(true, Ordering::Relaxed) {
                self.write_byte_raw(b'\r');
            }
        } else {
            LAST_WAS_CR.store(false, Ordering::Relaxed);
            self.write_byte_raw(byte);
        }
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
    // Keep the data byte even when the UART reports break/framing/overrun.
    // QEMU `-serial stdio` often sets those bits on CR/LF.
    let b = inb(COM1);
    if b == 0xFF {
        return None;
    }
    Some(b)
}

/// Interrupt on received data (IER bit 0: data available, or the FIFO's
/// character timeout). MCR's OUT2, set at init, gates the line to the PIC.
pub fn rx_irq_on() {
    SerialPort::new();
    outb(COM1 + 1, 0x01);
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
