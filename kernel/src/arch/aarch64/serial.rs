//! PL011 UART, at the address the device tree gives (`arm,pl011`).

use core::fmt;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Until `set_base` runs: QEMU `virt`'s PL011, so that a boot without a
/// usable device tree can still say so on the console it most likely has.
static BASE: AtomicUsize = AtomicUsize::new(0x0900_0000);

pub fn set_base(base: usize) {
    BASE.store(base, Ordering::SeqCst);
}

#[inline]
fn uart0() -> usize {
    BASE.load(Ordering::Relaxed)
}
const UARTDR: usize = 0x00;
const UARTFR: usize = 0x18;
const UARTIBRD: usize = 0x24;
const UARTFBRD: usize = 0x28;
const UARTLCR_H: usize = 0x2C;
const UARTCR: usize = 0x30;
const UARTIMSC: usize = 0x38;
const UARTICR: usize = 0x44;

const FR_TXFF: u32 = 1 << 5;
const FR_BUSY: u32 = 1 << 3;
const FR_RXFE: u32 = 1 << 4;

pub struct SerialPort;

impl SerialPort {
    pub fn new() -> Self {
        use core::sync::atomic::{AtomicBool, Ordering};
        // Program the PL011 once. The console constructs a `SerialPort` per
        // output byte; re-running this sequence each time disabled the UART
        // and rewrote LCR_H (which flushes the FIFOs) in the middle of
        // incoming data — the same input-loss class as the x86 FCR reset.
        static INITIALIZED: AtomicBool = AtomicBool::new(false);
        if INITIALIZED.swap(true, Ordering::AcqRel) {
            return Self;
        }
        // 115200 8N1-ish. QEMU's PL011 largely ignores baud, but a real init
        // sequence still makes TXE/UARTEN explicit.
        write32(UARTCR, 0);
        write32(UARTICR, 0x7FF);
        write32(UARTIBRD, 13);
        write32(UARTFBRD, 1);
        write32(UARTLCR_H, 0x70); // 8 bits, FIFO on
        write32(UARTIMSC, 0);
        write32(UARTCR, 0x301); // UARTEN | TXE | RXE
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
        while read32(UARTFR) & FR_TXFF != 0 {}
        write32(UARTDR, byte as u32);
    }

    pub fn flush(&mut self) {
        while read32(UARTFR) & FR_BUSY != 0 {}
    }
}

/// Non-blocking read from PL011. Returns `None` if the RX FIFO is empty.
pub fn read_byte() -> Option<u8> {
    if read32(UARTFR) & FR_RXFE != 0 {
        return None;
    }
    Some(read32(UARTDR) as u8)
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
fn read32(offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((uart0() + offset) as *const u32) }
}

#[inline]
fn write32(offset: usize, value: u32) {
    unsafe { core::ptr::write_volatile((uart0() + offset) as *mut u32, value) }
}
