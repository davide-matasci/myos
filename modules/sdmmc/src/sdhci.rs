//! A standard SDHCI controller (SD Host Controller Simplified Specification
//! 3.00): QEMU's `sdhci-pci` on every arch, the Raspberry Pi 4's EMMC2.
//! The data moves through the buffer data port (no DMA), completion by
//! polling the interrupt status. Every access is 32 bits wide, which every
//! controller takes and the Broadcom ones need (their 8- and 16-bit writes
//! are lost): the transfer mode is written with the command, the
//! write-1-to-clear status on its own.

use crate::card::{BLOCK, Data, Error, Host, Response};
use crate::{now_ns, until};

/// Block size (bits 11:0) and block count (31:16).
const BLOCK_SIZE: usize = 0x04;
const ARGUMENT: usize = 0x08;
/// Transfer mode (15:0) and command (31:16): writing the command starts it.
const TRANSFER: usize = 0x0c;
const RESPONSE: usize = 0x10;
const BUFFER: usize = 0x20;
const PRESENT: usize = 0x24;
/// Host control (7:0), power control (15:8).
const HOST_CONTROL: usize = 0x28;
/// Clock control (15:0), timeout control (23:16), software reset (31:24).
const CLOCK: usize = 0x2c;
/// Normal interrupt status (15:0), error interrupt status (31:16).
const STATUS: usize = 0x30;
const STATUS_ENABLE: usize = 0x34;
const SIGNAL_ENABLE: usize = 0x38;
const CAPABILITIES: usize = 0x40;
/// The specification version in bits 23:16 (`0xfc`'s upper half).
const VERSION: usize = 0xfc;

const PRESENT_CMD_INHIBIT: u32 = 1 << 0;
const PRESENT_DAT_INHIBIT: u32 = 1 << 1;

const HOST_4BIT: u32 = 1 << 1;
/// Bus power on at 3.3 V.
const POWER_33: u32 = (0x7 << 1 | 1) << 8;

const CLOCK_INTERNAL: u32 = 1 << 0;
const CLOCK_STABLE: u32 = 1 << 1;
const CLOCK_CARD: u32 = 1 << 2;
/// The longest data timeout (TMCLK × 2^27).
const TIMEOUT_MAX: u32 = 0xe << 16;
const RESET_ALL: u32 = 1 << 24;
const RESET_CMD: u32 = 1 << 25;
const RESET_DAT: u32 = 1 << 26;

const INT_CMD_DONE: u32 = 1 << 0;
const INT_XFER_DONE: u32 = 1 << 1;
const INT_WRITE_READY: u32 = 1 << 4;
const INT_READ_READY: u32 = 1 << 5;
const INT_ERROR: u32 = 1 << 15;
/// Command timeout (the first error bit) and data timeout.
const INT_TIMEOUT: u32 = 1 << 16 | 1 << 20;

const MODE_BLOCK_COUNT: u32 = 1 << 1;
const MODE_READ: u32 = 1 << 4;
const MODE_MULTI: u32 = 1 << 5;

const CMD_136: u32 = 1;
const CMD_48: u32 = 2;
const CMD_48_BUSY: u32 = 3;
const CMD_CRC: u32 = 1 << 3;
const CMD_INDEX: u32 = 1 << 4;
const CMD_DATA: u32 = 1 << 5;

/// The base clock when the capabilities do not give it (the Pi 4's EMMC2
/// does not): an upper bound, so a divisor from it never runs the card
/// faster than asked.
const BASE_CLOCK_UNKNOWN: u32 = 250_000_000;
/// How long a command or a block may take.
const TIMEOUT_NS: u64 = 1_000_000_000;

pub struct Sdhci {
    regs: usize,
    /// The specification version, 0 for 1.00.
    version: u32,
    base_clock: u32,
}

impl Sdhci {
    /// Reset the controller at `regs` (kernel virtual) and power the bus
    /// on; `None` when it does not come out of the reset.
    pub fn new(regs: usize) -> Option<Sdhci> {
        let mut h = Sdhci { regs, version: 0, base_clock: 0 };
        h.write(CLOCK, RESET_ALL);
        if !until(TIMEOUT_NS, || h.read(CLOCK) & RESET_ALL == 0) {
            return None;
        }
        h.version = (h.read(VERSION) >> 16) & 0xff;
        let mhz = (h.read(CAPABILITIES) >> 8) & if h.version >= 2 { 0xff } else { 0x3f };
        h.base_clock = if mhz == 0 { BASE_CLOCK_UNKNOWN } else { mhz * 1_000_000 };
        h.write(HOST_CONTROL, POWER_33);
        // Every status latched, none signalled: completion is polled.
        h.write(STATUS_ENABLE, 0xffff_0033);
        h.write(SIGNAL_ENABLE, 0);
        h.write(STATUS, !0);
        Some(h)
    }

    fn read(&self, off: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.regs + off) as *const u32) }
    }

    fn write(&self, off: usize, value: u32) {
        unsafe { core::ptr::write_volatile((self.regs + off) as *mut u32, value) }
    }

    /// Until `bits` of the interrupt status are set: then cleared. An error
    /// ends the wait: its kind, the command and data lines reset.
    fn wait_status(&self, bits: u32) -> Result<(), Error> {
        let deadline = now_ns() + TIMEOUT_NS;
        loop {
            let status = self.read(STATUS);
            if status & INT_ERROR != 0 {
                self.write(STATUS, !0);
                self.write(CLOCK, self.read(CLOCK) | RESET_CMD | RESET_DAT);
                until(TIMEOUT_NS, || self.read(CLOCK) & (RESET_CMD | RESET_DAT) == 0);
                return Err(if status & INT_TIMEOUT != 0 { Error::Timeout } else { Error::Bus });
            }
            if status & bits == bits {
                self.write(STATUS, bits);
                return Ok(());
            }
            if now_ns() > deadline {
                return Err(Error::Timeout);
            }
            core::hint::spin_loop();
        }
    }

    /// The divisor field of the clock control for at most `hz`.
    fn divisor(&self, hz: u32) -> u32 {
        if self.base_clock <= hz {
            return 0;
        }
        // The card clock is base / (2 × N): a 10-bit N from version 3.00,
        // before it a power of two up to 128.
        let n = self.base_clock.div_ceil(2 * hz);
        if self.version >= 2 {
            let n = n.min(0x3ff);
            (n & 0xff) << 8 | (n >> 8) << 6
        } else {
            n.next_power_of_two().min(0x80) << 8
        }
    }
}

impl Host for Sdhci {
    fn name(&self) -> &'static str {
        "sdhci"
    }

    fn set_clock(&mut self, hz: u32) {
        // The card clock off while the divisor changes.
        self.write(CLOCK, TIMEOUT_MAX);
        let clock = TIMEOUT_MAX | self.divisor(hz) | CLOCK_INTERNAL;
        self.write(CLOCK, clock);
        until(TIMEOUT_NS, || self.read(CLOCK) & CLOCK_STABLE != 0);
        self.write(CLOCK, clock | CLOCK_CARD);
    }

    fn set_wide(&mut self, wide: bool) {
        let control = self.read(HOST_CONTROL) & !HOST_4BIT;
        self.write(HOST_CONTROL, control | if wide { HOST_4BIT } else { 0 });
    }

    fn command(&mut self, index: u8, arg: u32, response: Response, data: Option<Data>) -> Result<[u32; 4], Error> {
        let inhibit = match (&data, response) {
            (Some(_), _) | (_, Response::ShortBusy) => PRESENT_CMD_INHIBIT | PRESENT_DAT_INHIBIT,
            _ => PRESENT_CMD_INHIBIT,
        };
        if !until(TIMEOUT_NS, || self.read(PRESENT) & inhibit == 0) {
            return Err(Error::Timeout);
        }
        self.write(STATUS, !0);
        let mut cmd = u32::from(index) << 8
            | match response {
                Response::None => 0,
                Response::Short => CMD_48 | CMD_CRC | CMD_INDEX,
                Response::ShortNoCrc => CMD_48,
                Response::ShortBusy => CMD_48_BUSY | CMD_CRC | CMD_INDEX,
                Response::Long => CMD_136 | CMD_CRC,
            };
        let mut mode = 0;
        if let Some(d) = &data {
            let blocks = (d.buf.len() / BLOCK) as u32;
            self.write(BLOCK_SIZE, blocks << 16 | BLOCK as u32);
            cmd |= CMD_DATA;
            mode = MODE_BLOCK_COUNT | if blocks > 1 { MODE_MULTI } else { 0 } | if d.write { 0 } else { MODE_READ };
        }
        self.write(ARGUMENT, arg);
        self.write(TRANSFER, cmd << 16 | mode);
        self.wait_status(INT_CMD_DONE)?;
        let mut resp = [0u32; 4];
        for (i, r) in resp.iter_mut().enumerate() {
            *r = self.read(RESPONSE + 4 * i);
        }
        if response == Response::Long {
            // The registers hold bits 127:8 (no CRC) from bit 0.
            resp = [resp[0] << 8, resp[1] << 8 | resp[0] >> 24, resp[2] << 8 | resp[1] >> 24, resp[3] << 8 | resp[2] >> 24];
        }
        match data {
            Some(d) => {
                let ready = if d.write { INT_WRITE_READY } else { INT_READ_READY };
                for block in d.buf.chunks_mut(BLOCK) {
                    self.wait_status(ready)?;
                    for word in block.chunks_mut(4) {
                        if d.write {
                            self.write(BUFFER, u32::from_le_bytes([word[0], word[1], word[2], word[3]]));
                        } else {
                            word.copy_from_slice(&self.read(BUFFER).to_le_bytes());
                        }
                    }
                }
                self.wait_status(INT_XFER_DONE)?;
            }
            // The end of the busy signal.
            None if response == Response::ShortBusy => self.wait_status(INT_XFER_DONE)?,
            None => {}
        }
        Ok(resp)
    }
}
