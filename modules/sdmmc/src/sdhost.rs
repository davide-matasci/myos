//! The Raspberry Pi 3's card slot: the BCM2835's own SD host controller
//! (`brcm,bcm2835-sdhost`), not an SDHCI one. Broadcom does not document
//! it; the register layout is the one Linux's `bcm2835` MMC driver and
//! QEMU's `bcm2835_sdhost` model use. The data moves through its 16-word
//! FIFO (no DMA), completion by polling. It keeps the GPIO pins as the
//! firmware and U-Boot left them (48 to 53 on the slot).

use crate::card::{BLOCK, Data, Error, Host, Response};
use crate::{now_ns, until};

const SDCMD: usize = 0x00;
const SDARG: usize = 0x04;
/// The data timeout, in card clocks.
const SDTOUT: usize = 0x08;
/// The card clock: the core clock / (divisor + 2).
const SDCDIV: usize = 0x0c;
const SDRSP0: usize = 0x10;
const SDHSTS: usize = 0x20;
const SDVDD: usize = 0x30;
/// Its state machine (3:0) and how full the FIFO is (8:4).
const SDEDM: usize = 0x34;
const SDHCFG: usize = 0x38;
/// Block size in bytes.
const SDHBCT: usize = 0x3c;
const SDDATA: usize = 0x40;
const SDHBLC: usize = 0x50;

const CMD_READ: u32 = 0x40;
const CMD_WRITE: u32 = 0x80;
const CMD_LONG: u32 = 0x200;
const CMD_NO_RESPONSE: u32 = 0x400;
const CMD_FAIL: u32 = 0x4000;
const CMD_NEW: u32 = 0x8000;

/// The error bits of the host status (write 1 to clear), and the
/// interrupt ones, cleared with them.
const HSTS_TIMEOUT: u32 = 0x40 | 0x80;
const HSTS_ERRORS: u32 = 0x08 | 0x10 | 0x20 | HSTS_TIMEOUT;
const HSTS_CLEAR: u32 = 0x7f8;

const HCFG_INTERNAL_WIDE: u32 = 1 << 1;
const HCFG_WIDE: u32 = 1 << 2;
const HCFG_SLOW_CARD: u32 = 1 << 3;

const EDM_FSM: u32 = 0xf;
const EDM_FSM_IDENT: u32 = 0x0;
const EDM_FSM_DATA: u32 = 0x1;
const EDM_FIFO_SHIFT: u32 = 4;
const EDM_FIFO: u32 = 0x1f;
const EDM_WRITE_THRESHOLD_SHIFT: u32 = 9;
const EDM_READ_THRESHOLD_SHIFT: u32 = 14;
const EDM_THRESHOLD: u32 = 0x1f;
const FIFO_WORDS: u32 = 16;

/// The core clock is not asked (that is the firmware's mailbox): at most
/// 400 MHz on a Pi 3, 250 MHz when `config.txt` has `enable_uart=1`. The
/// divisors are for the highest, so the card is never run faster than
/// asked.
const CORE_CLOCK_MAX: u32 = 400_000_000;
const TIMEOUT_NS: u64 = 1_000_000_000;

pub struct Sdhost {
    regs: usize,
    hcfg: u32,
}

impl Sdhost {
    /// Reset the controller at `regs` (kernel virtual) and power the card
    /// on.
    pub fn new(regs: usize) -> Sdhost {
        let h = Sdhost { regs, hcfg: HCFG_SLOW_CARD | HCFG_INTERNAL_WIDE };
        h.write(SDVDD, 0);
        for off in [SDCMD, SDARG, SDCDIV, SDHCFG, SDHBCT, SDHBLC] {
            h.write(off, 0);
        }
        h.write(SDTOUT, 0xf0_0000);
        h.write(SDHSTS, HSTS_CLEAR);
        let edm = h.read(SDEDM) & !(EDM_THRESHOLD << EDM_WRITE_THRESHOLD_SHIFT | EDM_THRESHOLD << EDM_READ_THRESHOLD_SHIFT);
        h.write(SDEDM, edm | 4 << EDM_WRITE_THRESHOLD_SHIFT | 4 << EDM_READ_THRESHOLD_SHIFT);
        delay_ms(20);
        h.write(SDVDD, 1);
        delay_ms(20);
        h.write(SDHCFG, h.hcfg);
        h
    }

    fn read(&self, off: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.regs + off) as *const u32) }
    }

    fn write(&self, off: usize, value: u32) {
        unsafe { core::ptr::write_volatile((self.regs + off) as *mut u32, value) }
    }

    /// The error in the host status, cleared: `None` without one.
    fn error(&self) -> Option<Error> {
        let hsts = self.read(SDHSTS);
        if hsts & HSTS_ERRORS == 0 {
            return None;
        }
        self.write(SDHSTS, HSTS_CLEAR);
        Some(if hsts & HSTS_TIMEOUT != 0 { Error::Timeout } else { Error::Bus })
    }

    fn fifo_words(&self) -> u32 {
        (self.read(SDEDM) >> EDM_FIFO_SHIFT) & EDM_FIFO
    }

    /// Move `buf` through the FIFO, a word at a time as it has one (a
    /// read) or room for one (a write).
    fn pio(&self, buf: &mut [u8], write: bool) -> Result<(), Error> {
        for word in buf.chunks_mut(4) {
            let deadline = now_ns() + TIMEOUT_NS;
            while if write { self.fifo_words() >= FIFO_WORDS } else { self.fifo_words() == 0 } {
                if let Some(e) = self.error() {
                    return Err(e);
                }
                if now_ns() > deadline {
                    return Err(Error::Timeout);
                }
                core::hint::spin_loop();
            }
            if write {
                self.write(SDDATA, u32::from_le_bytes([word[0], word[1], word[2], word[3]]));
            } else {
                word.copy_from_slice(&self.read(SDDATA).to_le_bytes());
            }
        }
        if write {
            // Until the last block has left for the card.
            let idle = |s: &Self| matches!(s.read(SDEDM) & EDM_FSM, EDM_FSM_IDENT | EDM_FSM_DATA);
            if !until(TIMEOUT_NS, || idle(self)) {
                return Err(Error::Timeout);
            }
        }
        self.error().map_or(Ok(()), Err)
    }
}

impl Host for Sdhost {
    fn name(&self) -> &'static str {
        "sdhost"
    }

    fn set_clock(&mut self, hz: u32) {
        let div = CORE_CLOCK_MAX.div_ceil(hz).saturating_sub(2).min(0x7ff);
        self.write(SDCDIV, div);
    }

    fn set_wide(&mut self, wide: bool) {
        self.hcfg = (self.hcfg & !HCFG_WIDE) | if wide { HCFG_WIDE } else { 0 };
        self.write(SDHCFG, self.hcfg);
    }

    fn command(&mut self, index: u8, arg: u32, response: Response, data: Option<Data>) -> Result<[u32; 4], Error> {
        if !until(TIMEOUT_NS, || self.read(SDCMD) & CMD_NEW == 0) {
            return Err(Error::Timeout);
        }
        self.write(SDHSTS, HSTS_CLEAR);
        let mut cmd = u32::from(index)
            | match response {
                Response::None => CMD_NO_RESPONSE,
                Response::Long => CMD_LONG,
                _ => 0,
            };
        if let Some(d) = &data {
            self.write(SDHBCT, BLOCK as u32);
            self.write(SDHBLC, (d.buf.len() / BLOCK) as u32);
            cmd |= if d.write { CMD_WRITE } else { CMD_READ };
        }
        self.write(SDARG, arg);
        self.write(SDCMD, cmd | CMD_NEW);
        if !until(TIMEOUT_NS, || self.read(SDCMD) & CMD_NEW == 0) {
            return Err(Error::Timeout);
        }
        if self.read(SDCMD) & CMD_FAIL != 0 {
            return Err(self.error().unwrap_or(Error::Timeout));
        }
        let mut resp = [0u32; 4];
        for (i, r) in resp.iter_mut().enumerate() {
            *r = self.read(SDRSP0 + 4 * i);
        }
        if let Some(d) = data {
            self.pio(d.buf, d.write)?;
        }
        // A busy card (R1b) is waited for by the card layer, with CMD13.
        Ok(resp)
    }
}

fn delay_ms(ms: u64) {
    until(ms * 1_000_000, || false);
}
