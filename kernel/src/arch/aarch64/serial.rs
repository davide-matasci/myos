//! PL011 UART on QEMU `virt` (base 0x0900_0000).
//!
//! That address is the virt board's long-standing UART0 mapping. QEMU docs
//! say device locations can move, but this one has been stable and matches
//! the board source (`VIRT_UART0`).

use core::fmt;

const UART0: usize = 0x0900_0000;
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
        if byte == b'\n' {
            self.write_byte_raw(b'\r');
        }
        self.write_byte_raw(byte);
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
    unsafe { core::ptr::read_volatile((UART0 + offset) as *const u32) }
}

#[inline]
fn write32(offset: usize, value: u32) {
    unsafe { core::ptr::write_volatile((UART0 + offset) as *mut u32, value) }
}
