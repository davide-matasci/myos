//! A standard SDHCI controller (SD Host Controller Simplified Specification
//! 3.00): QEMU's `sdhci-pci` on every arch, the Raspberry Pi 4's EMMC2,
//! the Pi 3's and 4's Wi-Fi controller. A memory card's blocks move by
//! ADMA2 through a bounce buffer the controller reaches (`dma_alloc`,
//! synced with `dma_sync`: the Pi's DMA is not cache-coherent); an SDIO
//! card's and any transfer the bounce buffer does not take move through
//! the buffer data port. A wait sleeps on the controller's interrupt once
//! it is routed ([`Sdhci::irq_routed`]), else it polls. Every access is 32
//! bits wide, which every controller takes and the Broadcom ones need
//! (their 8- and 16-bit writes are lost): the transfer mode is written
//! with the command, the write-1-to-clear status on its own.

use core::sync::atomic::{AtomicUsize, Ordering};

use myos_abi::{DMA_FROM_DEVICE, DMA_TO_DEVICE};

use crate::card::{Data, Error, Host, Response};
use crate::{MAX_BLOCKS, MAX_SLOTS, api, now_ns, until};

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
/// The ADMA2 descriptor table's address (32 bits).
const ADMA_ADDRESS: usize = 0x58;
/// The specification version in bits 23:16 (`0xfc`'s upper half).
const VERSION: usize = 0xfc;

const PRESENT_CMD_INHIBIT: u32 = 1 << 0;
const PRESENT_DAT_INHIBIT: u32 = 1 << 1;

const HOST_4BIT: u32 = 1 << 1;
const HOST_HIGH_SPEED: u32 = 1 << 2;
/// The DMA the transfer mode's DMA bit starts: ADMA2 with 32-bit addresses.
const HOST_DMA_MASK: u32 = 0x3 << 3;
const HOST_DMA_ADMA2: u32 = 0x2 << 3;
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

const CAP_ADMA2: u32 = 1 << 19;
const CAP_HIGH_SPEED: u32 = 1 << 21;

const INT_CMD_DONE: u32 = 1 << 0;
const INT_XFER_DONE: u32 = 1 << 1;
const INT_WRITE_READY: u32 = 1 << 4;
const INT_READ_READY: u32 = 1 << 5;
/// An SDIO card's interrupt (DAT1 low), level: it stays until the card is
/// served, so the handler masks it.
const INT_CARD: u32 = 1 << 8;
const INT_ERROR: u32 = 1 << 15;
/// Command timeout (the first error bit) and data timeout.
const INT_TIMEOUT: u32 = 1 << 16 | 1 << 20;
/// Every error bit, signalled whenever a wait is.
const INT_ERRORS: u32 = 0xffff_0000;
/// What a wait sleeps on: the interrupt handler masks these.
const INT_WAITS: u32 = INT_ERRORS | INT_CMD_DONE | INT_XFER_DONE | INT_WRITE_READY | INT_READ_READY;

const MODE_DMA: u32 = 1 << 0;
const MODE_BLOCK_COUNT: u32 = 1 << 1;
const MODE_READ: u32 = 1 << 4;
const MODE_MULTI: u32 = 1 << 5;

const CMD_136: u32 = 1;
const CMD_48: u32 = 2;
const CMD_48_BUSY: u32 = 3;
const CMD_CRC: u32 = 1 << 3;
const CMD_INDEX: u32 = 1 << 4;
const CMD_DATA: u32 = 1 << 5;

/// An ADMA2 descriptor's attributes: valid, a transfer, and the last one.
const ADMA_VALID: u64 = 1 << 0;
const ADMA_END: u64 = 1 << 1;
const ADMA_TRANSFER: u64 = 1 << 5;
/// The bytes one descriptor moves: a page of the bounce buffer.
const PAGE: usize = 4096;
/// The bounce buffer: the most one command moves.
const BOUNCE_PAGES: usize = MAX_BLOCKS * crate::card::BLOCK / PAGE;

/// The base clock when neither the firmware nor the capabilities give it:
/// an upper bound, so a divisor from it never runs the card faster than
/// asked.
const BASE_CLOCK_UNKNOWN: u32 = 250_000_000;
/// How long a command or a block may take.
const TIMEOUT_NS: u64 = 1_000_000_000;
/// How long a wait spins before it sleeps on the interrupt: a command
/// ends within it, a data transfer seldom does.
const SPIN_NS: u64 = 50_000;
/// The longest sleep between looks, should an interrupt not come.
const NAP_NS: u64 = 2_000_000;

fn reg_read(regs: usize, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((regs + off) as *const u32) }
}

fn reg_write(regs: usize, off: usize, value: u32) {
    unsafe { core::ptr::write_volatile((regs + off) as *mut u32, value) }
}

/// The card interrupt of the controller at `regs` raised to the CPU (`on`)
/// or masked: from any context, the status and signal enables' bit only
/// (the command path owns the others).
pub fn card_irq_enable(regs: usize, on: bool) {
    for reg in [STATUS_ENABLE, SIGNAL_ENABLE] {
        let v = reg_read(regs, reg) & !INT_CARD;
        reg_write(regs, reg, v | if on { INT_CARD } else { 0 });
    }
}

/// The card at `regs` is asserting its interrupt.
pub fn card_irq_pending(regs: usize) -> bool {
    reg_read(regs, STATUS) & INT_CARD != 0
}

/// The registers of the memory card slots whose interrupt is routed, by
/// slot: what the handler looks at (0: none).
static IRQ_REGS: [AtomicUsize; MAX_SLOTS] = [const { AtomicUsize::new(0) }; MAX_SLOTS];

/// The completion interrupt of slot `ctx`'s controller (`irq_enable`,
/// `pci_irq_enable`; a shared line may be another device's): what a wait
/// sleeps on masked, the waiter woken. The status stays for the waiter.
pub unsafe extern "C" fn irq_handler(ctx: *mut core::ffi::c_void) {
    let Some(slot) = IRQ_REGS.get(ctx as usize) else {
        return;
    };
    let regs = slot.load(Ordering::Acquire);
    if regs == 0 {
        return;
    }
    let signal = reg_read(regs, SIGNAL_ENABLE);
    if reg_read(regs, STATUS) & signal & INT_WAITS == 0 {
        return;
    }
    reg_write(regs, SIGNAL_ENABLE, signal & !INT_WAITS);
    api().wake(regs);
}

/// The ADMA2 bounce buffer and its descriptor table.
struct Dma {
    handle: i32,
    table: *mut u64,
    table_bus: u32,
    buf: *mut u8,
    buf_bus: u32,
}

// The buffers are the controller's, behind its slot's lock.
unsafe impl Send for Dma {}

impl Dma {
    /// The bounce buffer and table in memory the controller of DMA handle
    /// `handle` reaches below 4 GiB (ADMA2's 32-bit addresses), or `None`.
    fn new(handle: i32) -> Option<Dma> {
        if handle < 0 {
            return None;
        }
        let (mut table_bus, mut buf_bus) = (0u64, 0u64);
        let table = api().dma_alloc(handle, 1, &mut table_bus) as *mut u64;
        let buf = api().dma_alloc(handle, BOUNCE_PAGES, &mut buf_bus);
        if table.is_null() || buf.is_null() {
            return None;
        }
        let low = |bus: u64, len: usize| bus + len as u64 <= 1 << 32;
        if !low(table_bus, PAGE) || !low(buf_bus, BOUNCE_PAGES * PAGE) {
            return None;
        }
        Some(Dma { handle, table, table_bus: table_bus as u32, buf, buf_bus: buf_bus as u32 })
    }

    /// The bounce buffer's first `len` bytes.
    fn buf(&mut self, len: usize) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.buf, len) }
    }

    /// The table for `len` bytes of the buffer: a descriptor a page.
    fn describe(&mut self, len: usize) {
        let n = len.div_ceil(PAGE);
        for i in 0..n {
            let size = (len - i * PAGE).min(PAGE) as u64;
            let addr = u64::from(self.buf_bus) + (i * PAGE) as u64;
            let attr = ADMA_VALID | ADMA_TRANSFER | if i + 1 == n { ADMA_END } else { 0 };
            unsafe { self.table.add(i).write_volatile(addr << 32 | size << 16 | attr) };
        }
        api().dma_sync(self.handle, self.table as *const u8, n * 8, DMA_TO_DEVICE);
    }
}

pub struct Sdhci {
    regs: usize,
    /// The specification version, 0 for 1.00.
    version: u32,
    caps: u32,
    base_clock: u32,
    /// The card clock now, in Hz.
    clock: u32,
    /// Broadcom's bcm2835 SDHCI: its capabilities are not to be believed
    /// (no DMA of its own), and high speed is the clock alone, the host
    /// control's bit unset (Linux's `SDHCI_QUIRK_NO_HISPD_BIT`).
    bcm2835: bool,
    dma: Option<Dma>,
    /// The completion interrupt is routed: waits sleep on it.
    irq: bool,
}

// The DMA buffers are the controller's alone.
unsafe impl Send for Sdhci {}

impl Sdhci {
    /// Reset the controller at `regs` (kernel virtual) and power the bus
    /// on; `None` when it does not come out of the reset. `clock` is the
    /// base clock the firmware says (it wins over the capabilities, which
    /// some controllers leave 0); `bcm2835` marks Broadcom's own SDHCI.
    pub fn new(regs: usize, clock: Option<u32>, bcm2835: bool) -> Option<Sdhci> {
        let mut h = Sdhci {
            regs,
            version: 0,
            caps: 0,
            base_clock: 0,
            clock: 0,
            bcm2835,
            dma: None,
            irq: false,
        };
        h.write(CLOCK, RESET_ALL);
        if !until(TIMEOUT_NS, || h.read(CLOCK) & RESET_ALL == 0) {
            return None;
        }
        h.version = (h.read(VERSION) >> 16) & 0xff;
        h.caps = h.read(CAPABILITIES);
        let mhz = (h.caps >> 8) & if h.version >= 2 { 0xff } else { 0x3f };
        h.base_clock = match clock {
            Some(hz) => hz,
            None if mhz != 0 => mhz * 1_000_000,
            None => BASE_CLOCK_UNKNOWN,
        };
        h.write(HOST_CONTROL, POWER_33);
        // Every status latched, none signalled until a wait sleeps on it.
        h.write(STATUS_ENABLE, 0xffff_0033);
        h.write(SIGNAL_ENABLE, 0);
        h.write(STATUS, !0);
        Some(h)
    }

    /// Transfers by ADMA2 through memory of DMA handle `handle`
    /// (`dma_attach`), when the controller has it: whether they will.
    pub fn use_dma(&mut self, handle: i32) -> bool {
        if self.bcm2835 || self.caps & CAP_ADMA2 == 0 || self.version < 1 {
            return false;
        }
        let Some(dma) = Dma::new(handle) else {
            return false;
        };
        self.dma = Some(dma);
        let control = self.read(HOST_CONTROL) & !HOST_DMA_MASK;
        self.write(HOST_CONTROL, control | HOST_DMA_ADMA2);
        true
    }

    /// The completion interrupt now reaches [`irq_handler`] with `slot`
    /// as its context: waits sleep on it.
    pub fn irq_routed(&mut self, slot: usize) {
        IRQ_REGS[slot].store(self.regs, Ordering::Release);
        self.irq = true;
    }

    /// What `/dev/<disk>/info` says of the transfers: `dma adma2` or
    /// `dma none`, `irq on` or `irq off`.
    pub fn modes(&self) -> (&'static str, &'static str) {
        (if self.dma.is_some() { "adma2" } else { "none" }, if self.irq { "on" } else { "off" })
    }

    /// The card clock, in Hz.
    pub fn clock(&self) -> u32 {
        self.clock
    }

    fn read(&self, off: usize) -> u32 {
        reg_read(self.regs, off)
    }

    fn write(&self, off: usize, value: u32) {
        reg_write(self.regs, off, value)
    }

    /// The register window, for the card interrupt's handler.
    pub fn regs(&self) -> usize {
        self.regs
    }

    /// Until `bits` of the interrupt status are set: then cleared. An error
    /// ends the wait: its kind, the command and data lines reset. With the
    /// interrupt routed, a wait longer than a moment sleeps until it comes
    /// (or a short nap, should it not).
    fn wait_status(&self, bits: u32) -> Result<(), Error> {
        let start = now_ns();
        let deadline = start + TIMEOUT_NS;
        let result = loop {
            let seq = if self.irq { api().wait_seq() } else { 0 };
            let status = self.read(STATUS);
            if status & INT_ERROR != 0 {
                self.write(STATUS, !0);
                self.write(CLOCK, self.read(CLOCK) | RESET_CMD | RESET_DAT);
                until(TIMEOUT_NS, || self.read(CLOCK) & (RESET_CMD | RESET_DAT) == 0);
                break Err(if status & INT_TIMEOUT != 0 { Error::Timeout } else { Error::Bus });
            }
            if status & bits == bits {
                self.write(STATUS, bits);
                break Ok(());
            }
            let now = now_ns();
            if now > deadline {
                break Err(Error::Timeout);
            }
            if !self.irq || now - start < SPIN_NS {
                core::hint::spin_loop();
                continue;
            }
            // Signalled from now: a status that came since the look above
            // raises the interrupt at once, and its wake ends the sleep.
            let signal = self.read(SIGNAL_ENABLE) & !INT_WAITS;
            self.write(SIGNAL_ENABLE, signal | bits | INT_ERRORS);
            api().block_until(self.regs, seq, deadline.min(now + NAP_NS));
        };
        if self.irq {
            self.write(SIGNAL_ENABLE, self.read(SIGNAL_ENABLE) & !INT_WAITS);
        }
        result
    }

    /// The divisor field of the clock control for at most `hz`, and the
    /// clock it gives.
    fn divisor(&self, hz: u32) -> (u32, u32) {
        if self.base_clock <= hz {
            return (0, self.base_clock);
        }
        // The card clock is base / (2 × N): a 10-bit N from version 3.00,
        // before it a power of two up to 128.
        let n = self.base_clock.div_ceil(2 * hz);
        if self.version >= 2 {
            let n = n.min(0x3ff);
            ((n & 0xff) << 8 | (n >> 8) << 6, self.base_clock / (2 * n))
        } else {
            let n = n.next_power_of_two().min(0x80);
            (n << 8, self.base_clock / (2 * n))
        }
    }

    /// The data of a command by ADMA2 through the bounce buffer, when the
    /// controller has one and it takes `d` (whole words, at most its
    /// size).
    fn dma_fits(&self, d: &Data) -> bool {
        self.dma.is_some() && d.buf.len() % 4 == 0 && d.buf.len() <= BOUNCE_PAGES * PAGE
    }

    /// `d` into the bounce buffer for the controller (a write's data, a
    /// read's room), the table set.
    fn dma_start(&mut self, d: &Data) {
        let len = d.buf.len();
        let dma = self.dma.as_mut().unwrap();
        if d.write {
            dma.buf(len).copy_from_slice(d.buf);
        }
        // Handed over before the device writes too: no dirty line of the
        // buffer may land on what it reads into.
        api().dma_sync(dma.handle, dma.buf, len, DMA_TO_DEVICE);
        dma.describe(len);
        let table = dma.table_bus;
        self.write(ADMA_ADDRESS, table);
    }

    /// What the controller read, out of the bounce buffer into `d`.
    fn dma_finish(&mut self, d: &mut Data) {
        if d.write {
            return;
        }
        let len = d.buf.len();
        let dma = self.dma.as_mut().unwrap();
        api().dma_sync(dma.handle, dma.buf, len, DMA_FROM_DEVICE);
        d.buf.copy_from_slice(dma.buf(len));
    }
}

impl Host for Sdhci {
    fn name(&self) -> &'static str {
        "sdhci"
    }

    fn set_clock(&mut self, hz: u32) {
        // The card clock off while the divisor changes.
        self.write(CLOCK, TIMEOUT_MAX);
        let (divisor, clock) = self.divisor(hz);
        let control = TIMEOUT_MAX | divisor | CLOCK_INTERNAL;
        self.write(CLOCK, control);
        until(TIMEOUT_NS, || self.read(CLOCK) & CLOCK_STABLE != 0);
        self.write(CLOCK, control | CLOCK_CARD);
        self.clock = clock;
    }

    fn set_wide(&mut self, wide: bool) {
        let control = self.read(HOST_CONTROL) & !HOST_4BIT;
        self.write(HOST_CONTROL, control | if wide { HOST_4BIT } else { 0 });
    }

    fn high_speed(&self) -> bool {
        self.bcm2835 || self.caps & CAP_HIGH_SPEED != 0
    }

    fn set_high_speed(&mut self, on: bool) {
        if self.bcm2835 {
            return;
        }
        let control = self.read(HOST_CONTROL) & !HOST_HIGH_SPEED;
        self.write(HOST_CONTROL, control | if on { HOST_HIGH_SPEED } else { 0 });
    }

    fn delay_ms(&mut self, ms: u32) {
        crate::sleep_ms(u64::from(ms));
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
        let dma = data.as_ref().is_some_and(|d| self.dma_fits(d));
        if let Some(d) = &data {
            let blocks = (d.buf.len() / d.block) as u32;
            self.write(BLOCK_SIZE, blocks << 16 | d.block as u32);
            cmd |= CMD_DATA;
            mode = MODE_BLOCK_COUNT | if blocks > 1 { MODE_MULTI } else { 0 } | if d.write { 0 } else { MODE_READ };
            if dma {
                self.dma_start(d);
                mode |= MODE_DMA;
            }
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
            Some(mut d) if dma => {
                self.wait_status(INT_XFER_DONE)?;
                self.dma_finish(&mut d);
            }
            Some(d) => {
                let ready = if d.write { INT_WRITE_READY } else { INT_READ_READY };
                for block in d.buf.chunks_mut(d.block) {
                    self.wait_status(ready)?;
                    // A byte-mode SDIO transfer may end short of a word.
                    for word in block.chunks_mut(4) {
                        if d.write {
                            let mut w = [0u8; 4];
                            w[..word.len()].copy_from_slice(word);
                            self.write(BUFFER, u32::from_le_bytes(w));
                        } else {
                            word.copy_from_slice(&self.read(BUFFER).to_le_bytes()[..word.len()]);
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
