//! The Raspberry Pi 3's card slot: the BCM2835's own SD host controller
//! (`brcm,bcm2835-sdhost`), not an SDHCI one. Broadcom does not document
//! it; the register layout is the one Linux's `bcm2835` MMC driver and
//! QEMU's `bcm2835_sdhost` model use. The data moves through its 16-word
//! FIFO by the CPU (its DREQ to the BCM2835 DMA engine is not used: issue
//! #576); a wait for the FIFO or the end of a write sleeps on the
//! controller's interrupt once it is routed ([`Sdhost::irq_routed`]),
//! else it polls. It keeps the GPIO pins as the firmware and U-Boot left
//! them (48 to 53 on the slot).

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::card::{Data, Error, Host, Response};
use crate::{api, now_ns, until};

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
/// The interrupt bits: the FIFO's data (QEMU's model raises the SDIO bit
/// for it), a block written, a busy card done.
const HSTS_IRQS: u32 = 0x100 | 0x200 | 0x400;

const HCFG_INTERNAL_WIDE: u32 = 1 << 1;
const HCFG_WIDE: u32 = 1 << 2;
const HCFG_SLOW_CARD: u32 = 1 << 3;
/// Interrupts on the FIFO's data and on a block's end.
const HCFG_DATA_IRQ: u32 = 1 << 4;
const HCFG_BLOCK_IRQ: u32 = 1 << 8;

const EDM_FSM: u32 = 0xf;
const EDM_FSM_IDENT: u32 = 0x0;
const EDM_FSM_DATA: u32 = 0x1;
const EDM_FIFO_SHIFT: u32 = 4;
const EDM_FIFO: u32 = 0x1f;
const EDM_WRITE_THRESHOLD_SHIFT: u32 = 9;
const EDM_READ_THRESHOLD_SHIFT: u32 = 14;
const EDM_THRESHOLD: u32 = 0x1f;
const FIFO_WORDS: u32 = 16;

/// The core clock when the firmware does not say it (its mailbox, through
/// `rpi_firmware`): at most 400 MHz on a Pi 3, 250 MHz when `config.txt`
/// has `enable_uart=1`. The divisors are then for the highest, so the card
/// is never run faster than asked.
const CORE_CLOCK_MAX: u32 = 400_000_000;
const TIMEOUT_NS: u64 = 1_000_000_000;
/// How long a wait spins before it sleeps on the interrupt, and the
/// longest sleep between looks, should the interrupt not come.
const SPIN_NS: u64 = 50_000;
const NAP_NS: u64 = 2_000_000;

/// The routed controller's registers and its configuration without the
/// interrupt enables, for the handler (there is one on a Pi 3).
static IRQ_REGS: AtomicUsize = AtomicUsize::new(0);
static IRQ_HCFG: AtomicU32 = AtomicU32::new(0);

/// The controller's interrupt (`irq_enable`): its enables off, the waiter
/// woken. The status stays for the waiter.
pub unsafe extern "C" fn irq_handler(_ctx: *mut core::ffi::c_void) {
    let regs = IRQ_REGS.load(Ordering::Acquire);
    if regs == 0 {
        return;
    }
    let write = |off: usize, v: u32| unsafe { core::ptr::write_volatile((regs + off) as *mut u32, v) };
    write(SDHCFG, IRQ_HCFG.load(Ordering::Relaxed));
    write(SDHSTS, HSTS_IRQS);
    api().wake(regs);
}

pub struct Sdhost {
    regs: usize,
    hcfg: u32,
    core_clock: u32,
    /// The card clock now, in Hz.
    clock: u32,
    /// The interrupt is routed: waits sleep on it.
    irq: bool,
}

impl Sdhost {
    /// Reset the controller at `regs` (kernel virtual) and power the card
    /// on. `core_clock` is the core clock the firmware says.
    pub fn new(regs: usize, core_clock: Option<u32>) -> Sdhost {
        let h = Sdhost {
            regs,
            hcfg: HCFG_SLOW_CARD | HCFG_INTERNAL_WIDE,
            core_clock: core_clock.unwrap_or(CORE_CLOCK_MAX),
            clock: 0,
            irq: false,
        };
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

    /// The interrupt now reaches [`irq_handler`]: waits sleep on it.
    pub fn irq_routed(&mut self) {
        IRQ_HCFG.store(self.hcfg, Ordering::Relaxed);
        IRQ_REGS.store(self.regs, Ordering::Release);
        self.irq = true;
    }

    /// `irq on` or `irq off`, for `/dev/<disk>/info`.
    pub fn irq_mode(&self) -> &'static str {
        if self.irq { "on" } else { "off" }
    }

    /// The card clock, in Hz.
    pub fn clock(&self) -> u32 {
        self.clock
    }

    /// Until `ready`, or an error (its kind), or the timeout. With the
    /// interrupt routed, a wait longer than a moment sleeps until it comes
    /// with `hcfg_irqs` enabled (or a short nap, should it not).
    fn wait(&self, hcfg_irqs: u32, mut ready: impl FnMut(&Self) -> bool) -> Result<(), Error> {
        let start = now_ns();
        let deadline = start + TIMEOUT_NS;
        let result = loop {
            let seq = if self.irq { api().wait_seq() } else { 0 };
            if ready(self) {
                break Ok(());
            }
            if let Some(e) = self.error() {
                break Err(e);
            }
            let now = now_ns();
            if now > deadline {
                break Err(Error::Timeout);
            }
            if !self.irq || now - start < SPIN_NS {
                core::hint::spin_loop();
                continue;
            }
            // Enabled from now: a FIFO that filled since the look above
            // raises the interrupt at once, and its wake ends the sleep.
            self.write(SDHSTS, HSTS_IRQS);
            self.write(SDHCFG, self.hcfg | hcfg_irqs);
            api().block_until(self.regs, seq, deadline.min(now + NAP_NS));
        };
        if self.irq {
            self.write(SDHCFG, self.hcfg);
            self.write(SDHSTS, HSTS_IRQS);
        }
        result
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

    /// Move `buf` through the FIFO: as many words at a time as it has (a
    /// read) or has room for (a write), waiting when it has none.
    fn pio(&self, buf: &mut [u8], write: bool) -> Result<(), Error> {
        let mut words = buf.chunks_mut(4).peekable();
        while words.peek().is_some() {
            let mut n = 0;
            self.wait(HCFG_DATA_IRQ, |s| {
                let fill = s.fifo_words();
                n = if write { FIFO_WORDS.saturating_sub(fill) } else { fill };
                n > 0
            })?;
            for word in words.by_ref().take(n as usize) {
                if write {
                    self.write(SDDATA, u32::from_le_bytes([word[0], word[1], word[2], word[3]]));
                } else {
                    word.copy_from_slice(&self.read(SDDATA).to_le_bytes());
                }
            }
        }
        if write {
            // Until the last block has left for the card.
            self.wait(HCFG_BLOCK_IRQ, |s| matches!(s.read(SDEDM) & EDM_FSM, EDM_FSM_IDENT | EDM_FSM_DATA))?;
        }
        self.error().map_or(Ok(()), Err)
    }
}

impl Host for Sdhost {
    fn name(&self) -> &'static str {
        "sdhost"
    }

    fn set_clock(&mut self, hz: u32) {
        let div = self.core_clock.div_ceil(hz).saturating_sub(2).min(0x7ff);
        self.write(SDCDIV, div);
        self.clock = self.core_clock / (div + 2);
    }

    fn set_wide(&mut self, wide: bool) {
        self.hcfg = (self.hcfg & !HCFG_WIDE) | if wide { HCFG_WIDE } else { 0 };
        IRQ_HCFG.store(self.hcfg, Ordering::Relaxed);
        self.write(SDHCFG, self.hcfg);
    }

    /// High speed is the clock alone (Linux's `bcm2835` driver runs cards
    /// at 50 MHz so); the slow-card mode stays, as it does there.
    fn high_speed(&self) -> bool {
        true
    }

    fn delay_ms(&mut self, ms: u32) {
        delay_ms(u64::from(ms));
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
            self.write(SDHBCT, d.block as u32);
            self.write(SDHBLC, (d.buf.len() / d.block) as u32);
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
    crate::sleep_ms(ms);
}
