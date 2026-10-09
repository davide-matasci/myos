//! The console UART, as the platform description names it: a PL011
//! (`arm,pl011`, the SBSA UART; QEMU `virt`) or a 16550 (`ns16550a`,
//! `snps,dw-apb-uart`: most SoCs), the latter with the board's register
//! stride and access width (`reg-shift`, `reg-io-width`; the SPCR's access
//! size).

use core::fmt;
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use crate::platform::{Uart, UartKind};

/// Until `set_uart` runs: QEMU `virt`'s PL011, so that a boot without a
/// usable description can still say so on the console it most likely has.
static BASE: AtomicUsize = AtomicUsize::new(0x0900_0000);
/// 0: PL011, 1: 16550.
static KIND: AtomicU8 = AtomicU8::new(0);
/// 16550: register `n` is at `BASE + (n << SHIFT)`, `WIDTH` bytes wide.
static SHIFT: AtomicU8 = AtomicU8::new(0);
static WIDTH: AtomicU8 = AtomicU8::new(1);

pub fn set_uart(uart: Uart) {
    BASE.store(uart.base as usize, Ordering::SeqCst);
    SHIFT.store(uart.reg_shift, Ordering::SeqCst);
    WIDTH.store(uart.reg_width.max(1), Ordering::SeqCst);
    KIND.store(matches!(uart.kind, UartKind::Ns16550) as u8, Ordering::SeqCst);
}

#[inline]
fn uart0() -> usize {
    BASE.load(Ordering::Relaxed)
}

#[inline]
fn is_16550() -> bool {
    KIND.load(Ordering::Relaxed) == 1
}

// PL011 registers.
const UARTDR: usize = 0x00;
const UARTFR: usize = 0x18;
const UARTIBRD: usize = 0x24;
const UARTFBRD: usize = 0x28;
const UARTLCR_H: usize = 0x2C;
const UARTCR: usize = 0x30;
const UARTIMSC: usize = 0x38;
const UARTICR: usize = 0x44;

/// UARTIMSC: the receive and receive-timeout interrupts.
const IMSC_RX: u32 = (1 << 4) | (1 << 6);

const FR_TXFF: u32 = 1 << 5;
const FR_BUSY: u32 = 1 << 3;
const FR_RXFE: u32 = 1 << 4;

// 16550 registers (indices, before the stride).
const THR: usize = 0;
const IER: usize = 1;
const LSR: usize = 5;
const LSR_RX_READY: u32 = 1 << 0;
const LSR_TX_IDLE: u32 = 1 << 5;

pub struct SerialPort;

impl SerialPort {
    pub fn new() -> Self {
        use core::sync::atomic::{AtomicBool, Ordering};
        if is_16550() {
            // The firmware programmed the line; the 16550 is used as found.
            return Self;
        }
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
        if is_16550() {
            while reg_read(LSR) & LSR_TX_IDLE == 0 {}
            reg_write(THR, u32::from(byte));
            return;
        }
        while read32(UARTFR) & FR_TXFF != 0 {}
        write32(UARTDR, byte as u32);
    }

    pub fn flush(&mut self) {
        if is_16550() {
            while reg_read(LSR) & LSR_TX_IDLE == 0 {}
            return;
        }
        while read32(UARTFR) & FR_BUSY != 0 {}
    }
}

/// Interrupt on received data: a 16550's IER bit 0 (data available, or the
/// FIFO's character timeout), a PL011's receive and receive-timeout
/// interrupts (both clear as the FIFO is read empty).
pub fn rx_irq_on() {
    SerialPort::new();
    if is_16550() {
        reg_write(IER, 0x01);
    } else {
        write32(UARTIMSC, IMSC_RX);
    }
}

/// Non-blocking read. Returns `None` if the RX FIFO is empty.
pub fn read_byte() -> Option<u8> {
    if is_16550() {
        if reg_read(LSR) & LSR_RX_READY == 0 {
            return None;
        }
        return Some(reg_read(THR) as u8);
    }
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

/// 16550 register `n`, at the board's stride and width.
#[inline]
fn reg_read(n: usize) -> u32 {
    let addr = uart0() + (n << SHIFT.load(Ordering::Relaxed));
    unsafe {
        match WIDTH.load(Ordering::Relaxed) {
            4 => core::ptr::read_volatile(addr as *const u32),
            2 => u32::from(core::ptr::read_volatile(addr as *const u16)),
            _ => u32::from(core::ptr::read_volatile(addr as *const u8)),
        }
    }
}

#[inline]
fn reg_write(n: usize, value: u32) {
    let addr = uart0() + (n << SHIFT.load(Ordering::Relaxed));
    unsafe {
        match WIDTH.load(Ordering::Relaxed) {
            4 => core::ptr::write_volatile(addr as *mut u32, value),
            2 => core::ptr::write_volatile(addr as *mut u16, value as u16),
            _ => core::ptr::write_volatile(addr as *mut u8, value as u8),
        }
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
