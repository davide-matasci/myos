//! NS16550 UART on QEMU `virt` (base 0x1000_0000).

use core::fmt;

const UART0: usize = 0x1000_0000;
const THR: usize = 0x00;
const LSR: usize = 0x05;

const LSR_TX_IDLE: u8 = 1 << 5;
const LSR_RX_READY: u8 = 1 << 0;

pub struct SerialPort;

impl SerialPort {
    pub fn new() -> Self {
        Self
    }

    pub fn write_byte(&mut self, byte: u8) {
        if byte == b'\n' {
            self.write_byte_raw(b'\r');
        }
        self.write_byte_raw(byte);
    }

    fn write_byte_raw(&mut self, byte: u8) {
        while read8(LSR) & LSR_TX_IDLE == 0 {}
        write8(THR, byte);
    }

    pub fn flush(&mut self) {
        while read8(LSR) & LSR_TX_IDLE == 0 {}
    }
}

pub fn read_byte() -> Option<u8> {
    if read8(LSR) & LSR_RX_READY == 0 {
        return None;
    }
    Some(read8(THR))
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
fn read8(offset: usize) -> u8 {
    unsafe { core::ptr::read_volatile((UART0 + offset) as *const u8) }
}

#[inline]
fn write8(offset: usize, value: u8) {
    unsafe { core::ptr::write_volatile((UART0 + offset) as *mut u8, value) }
}
