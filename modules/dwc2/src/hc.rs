//! The controller: Synopsys's DesignWare USB 2.0 OTG core (DWC2) as a
//! host, the register layout the controller's manuals give (the names
//! Linux's and QEMU's follow: `GAHBCFG`, `HPRT0`, `HCCHAR`, ...). The
//! Raspberry Pi 3's one has 8 host channels, one root port and its DMA,
//! which does not see the CPU's caches.
//!
//! A transfer takes a channel: its registers say which device, endpoint
//! and direction, how many packets of what size, and where in memory
//! (`HCDMA`, a bounce buffer of this module's); enabling it moves them,
//! and the channel halts with `HCINT` saying how it went. The interrupt
//! handler (or, without an interrupt, whoever polls) records the halt in
//! the channel's [`Chan`] and wakes the waiter; the state machines (the
//! stages of a control transfer, a split transaction's start and complete
//! halves, a NAKed interrupt endpoint's retry) run in task context.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use myos_abi::{
    DMA_FROM_DEVICE, DMA_TO_DEVICE, SleepLock, USB_EGONE, USB_EIO, USB_ESTALL, USB_ETIMEDOUT, USB_SPEED_FULL,
    USB_SPEED_HIGH, USB_SPEED_LOW,
};

use crate::api;

/// Host channels the module uses (the Pi 3's controller has 8).
pub const CHANNELS: usize = 8;
/// Bytes of the bulk bounce buffer (16 contiguous pages; a page when the
/// kernel cannot give more).
const BOUNCE_PAGES: usize = 16;
/// The most a channel moves in one go: the transfer size field is 19 bits,
/// the packet count 10.
const MAX_XFER: usize = 0x7_FFFF;
const MAX_PACKETS: usize = 0x3FF;

// Global registers.
const GAHBCFG: usize = 0x008;
const GUSBCFG: usize = 0x00c;
const GRSTCTL: usize = 0x010;
const GINTSTS: usize = 0x014;
const GINTMSK: usize = 0x018;
const GRXFSIZ: usize = 0x024;
const GNPTXFSIZ: usize = 0x028;
const GSNPSID: usize = 0x040;
const GHWCFG2: usize = 0x048;
const HPTXFSIZ: usize = 0x100;
// Host registers.
const HCFG: usize = 0x400;
const HFNUM: usize = 0x408;
const HAINT: usize = 0x414;
const HAINTMSK: usize = 0x418;
const HPRT0: usize = 0x440;
/// Channel `ch`'s registers: characteristics, split control, interrupt
/// status and mask, transfer size, DMA address.
fn reg_hcchar(ch: usize) -> usize {
    0x500 + 0x20 * ch
}
fn reg_hcsplt(ch: usize) -> usize {
    0x504 + 0x20 * ch
}
fn reg_hcint(ch: usize) -> usize {
    0x508 + 0x20 * ch
}
fn reg_hcintmsk(ch: usize) -> usize {
    0x50c + 0x20 * ch
}
fn reg_hctsiz(ch: usize) -> usize {
    0x510 + 0x20 * ch
}
fn reg_hcdma(ch: usize) -> usize {
    0x514 + 0x20 * ch
}

const GAHBCFG_GLBL_INTR_EN: u32 = 1 << 0;
const GAHBCFG_HBSTLEN_INCR4: u32 = 3 << 1;
const GAHBCFG_DMA_EN: u32 = 1 << 5;
const GUSBCFG_PHYIF16: u32 = 1 << 3;
const GUSBCFG_ULPI_UTMI_SEL: u32 = 1 << 4;
const GUSBCFG_PHYSEL: u32 = 1 << 6;
const GUSBCFG_FORCEHOSTMODE: u32 = 1 << 29;
const GUSBCFG_FORCEDEVMODE: u32 = 1 << 30;
const GRSTCTL_CSFTRST: u32 = 1 << 0;
const GRSTCTL_RXFFLSH: u32 = 1 << 4;
const GRSTCTL_TXFFLSH: u32 = 1 << 5;
/// `TXFNUM`: every transmit FIFO.
const GRSTCTL_TXFNUM_ALL: u32 = 0x10 << 6;
const GRSTCTL_AHBIDLE: u32 = 1 << 31;
const GINTSTS_PRTINT: u32 = 1 << 24;
const GINTSTS_HCHINT: u32 = 1 << 25;
const GINTSTS_DISCONNINT: u32 = 1 << 29;

const HPRT0_CONNSTS: u32 = 1 << 0;
const HPRT0_CONNDET: u32 = 1 << 1;
const HPRT0_ENA: u32 = 1 << 2;
const HPRT0_ENACHG: u32 = 1 << 3;
const HPRT0_OVRCURRCHG: u32 = 1 << 5;
const HPRT0_RST: u32 = 1 << 8;
const HPRT0_PWR: u32 = 1 << 12;
const HPRT0_SPD_SHIFT: u32 = 17;
/// The bits a write of `HPRT0` must not carry over from a read: the enable
/// is write-1-to-clear (writing it back disables the port), and so are
/// the change bits.
const HPRT0_W1C: u32 = HPRT0_ENA | HPRT0_CONNDET | HPRT0_ENACHG | HPRT0_OVRCURRCHG;
const HPRT0_CHANGES: u32 = HPRT0_CONNDET | HPRT0_ENACHG | HPRT0_OVRCURRCHG;

const HCCHAR_MPS_MASK: u32 = 0x7FF;
const HCCHAR_EPNUM_SHIFT: u32 = 11;
const HCCHAR_EPDIR_IN: u32 = 1 << 15;
const HCCHAR_LSPDDEV: u32 = 1 << 17;
const HCCHAR_EPTYPE_SHIFT: u32 = 18;
const HCCHAR_MULTICNT_SHIFT: u32 = 20;
const HCCHAR_DEVADDR_SHIFT: u32 = 22;
const HCCHAR_ODDFRM: u32 = 1 << 29;
const HCCHAR_CHDIS: u32 = 1 << 30;
const HCCHAR_CHENA: u32 = 1 << 31;

const HCSPLT_PRTADDR_SHIFT: u32 = 0;
const HCSPLT_HUBADDR_SHIFT: u32 = 7;
const HCSPLT_XACTPOS_ALL: u32 = 3 << 14;
const HCSPLT_COMPSPLT: u32 = 1 << 16;
const HCSPLT_SPLTENA: u32 = 1 << 31;

pub const HCINT_XFERCOMPL: u32 = 1 << 0;
pub const HCINT_CHHLTD: u32 = 1 << 1;
const HCINT_AHBERR: u32 = 1 << 2;
pub const HCINT_STALL: u32 = 1 << 3;
pub const HCINT_NAK: u32 = 1 << 4;
pub const HCINT_ACK: u32 = 1 << 5;
pub const HCINT_NYET: u32 = 1 << 6;
const HCINT_XACTERR: u32 = 1 << 7;
const HCINT_BBLERR: u32 = 1 << 8;
const HCINT_DATATGLERR: u32 = 1 << 10;
const HCINT_ERRORS: u32 = HCINT_AHBERR | HCINT_XACTERR | HCINT_BBLERR | HCINT_DATATGLERR;

const TSIZ_XFERSIZE_MASK: u32 = 0x7_FFFF;
const TSIZ_PKTCNT_SHIFT: u32 = 19;
const TSIZ_PKTCNT_MASK: u32 = 0x3FF << TSIZ_PKTCNT_SHIFT;
const TSIZ_PID_SHIFT: u32 = 29;
/// The data PIDs, as `HCTSIZ` codes them.
pub const PID_DATA0: u32 = 0;
pub const PID_DATA1: u32 = 2;
pub const PID_SETUP: u32 = 3;

/// A channel's record: what the interrupt handler leaves there when the
/// channel halts, and what the task context needs between the halts.
/// Atomics only: the handler holds no lock.
pub struct Chan {
    /// Taken by a transfer (`take`), given back when it is over.
    pub busy: AtomicBool,
    /// Halted since the last enable: `hcint` and `tsiz` are the halt's.
    pub done: AtomicBool,
    pub hcint: AtomicU32,
    pub tsiz: AtomicU32,
    /// Queued by the core (`transfer_start`): a halt wakes the core's
    /// thread, not a waiter.
    pub queued: AtomicBool,
    /// Its transfer is to be tried again from `retry_at` (monotonic ns; 0:
    /// not waiting): a NAKed interrupt endpoint.
    pub retry_at: AtomicU64,
    /// The device the transfer is for (`device_abort` fails it).
    pub owner: AtomicU32,
    /// `device_abort` struck while the transfer ran.
    pub aborted: AtomicBool,
}

impl Chan {
    const fn new() -> Self {
        Chan {
            busy: AtomicBool::new(false),
            done: AtomicBool::new(false),
            hcint: AtomicU32::new(0),
            tsiz: AtomicU32::new(0),
            queued: AtomicBool::new(false),
            retry_at: AtomicU64::new(0),
            owner: AtomicU32::new(0),
            aborted: AtomicBool::new(false),
        }
    }

    /// What a waiter blocks on and the handler wakes.
    pub fn key(&self) -> usize {
        self.done.as_ptr() as usize
    }
}

/// The bulk bounce buffer.
pub struct Bounce {
    pub va: *mut u8,
    pub bus: u64,
    pub len: usize,
}

// SAFETY: the buffer is the controller's own pages.
unsafe impl Send for Bounce {}

/// A controller and what hangs from it, reached as `&Controller` from
/// every context: each part is an atomic, behind a lock, or written by
/// `init` alone.
pub struct Controller {
    pub regs: usize,
    /// Its DMA handle (`dma_attach`).
    pub dma: i32,
    pub channels: usize,
    pub chans: [Chan; CHANNELS],
    /// A page per channel, `(bus address, va)`: the setup packet and the
    /// data of a control transfer, an interrupt transfer's report.
    pub pages: [(u64, *mut u8); CHANNELS],
    /// The bulk bounce buffer, held for a transfer.
    pub bounce: SleepLock<Bounce>,
    /// The interrupt is routed (`irq_enable` succeeded).
    pub irq_on: AtomicBool,
    /// Channel halts handled, interrupts taken, for `/proc/usb`.
    pub events: AtomicU32,
    pub irqs: AtomicU32,
    /// The core's index for the controller (`controller_register`), what
    /// its port changes and completions are reported against; `u32::MAX`
    /// until registered.
    pub core_index: AtomicU32,
}

// SAFETY: see the struct: nothing in it is written through a shared
// reference but atomics and the locked bounce buffer; the raw pointers are
// the controller's own DMA pages.
unsafe impl Sync for Controller {}
unsafe impl Send for Controller {}

fn read(regs: usize, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((regs + off) as *const u32) }
}

fn write(regs: usize, off: usize, value: u32) {
    unsafe { core::ptr::write_volatile((regs + off) as *mut u32, value) }
}

/// Spin until `done`, for at most `timeout_ns`: whether it was.
fn until(timeout_ns: u64, mut done: impl FnMut() -> bool) -> bool {
    let deadline = api().monotonic_ns() + timeout_ns;
    loop {
        if done() {
            return true;
        }
        if api().monotonic_ns() > deadline {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// Bring the controller at `regs` up: a core reset, the PHY, host mode,
/// the FIFOs, DMA, the port's power, the interrupts it may raise.
pub fn init(regs: usize, dma: i32) -> Result<Controller, &'static str> {
    if dma < 0 {
        return Err("no DMA handle");
    }
    // "OT2", "OT3": the core's id, so a wrong node is not driven.
    if read(regs, GSNPSID) >> 16 != 0x4F54 {
        return Err("not a DWC2 core");
    }
    let channels = (((read(regs, GHWCFG2) >> 14) & 0xF) as usize + 1).min(CHANNELS);
    if !until(100_000_000, || read(regs, GRSTCTL) & GRSTCTL_AHBIDLE != 0) {
        return Err("AHB not idle");
    }
    write(regs, GRSTCTL, GRSTCTL_CSFTRST);
    if !until(100_000_000, || read(regs, GRSTCTL) & GRSTCTL_CSFTRST == 0) {
        return Err("does not reset");
    }
    let _ = until(100_000_000, || read(regs, GRSTCTL) & GRSTCTL_AHBIDLE != 0);
    crate::sleep_ms(10);
    // The PHY: UTMI+, 8 bits wide (the Pi's); host mode, whatever the
    // ID pin says.
    let mut cfg = read(regs, GUSBCFG);
    cfg &= !(GUSBCFG_ULPI_UTMI_SEL | GUSBCFG_PHYIF16 | GUSBCFG_PHYSEL | GUSBCFG_FORCEDEVMODE);
    cfg |= GUSBCFG_FORCEHOSTMODE;
    write(regs, GUSBCFG, cfg);
    // A mode change takes up to 25 ms.
    crate::sleep_ms(30);
    // The FIFOs (in words): receive, non-periodic and periodic transmit,
    // the sizes the Pi's firmware and Linux use; then flushed.
    write(regs, GRXFSIZ, 774);
    write(regs, GNPTXFSIZ, (256 << 16) | 774);
    write(regs, HPTXFSIZ, (512 << 16) | (774 + 256));
    write(regs, GRSTCTL, GRSTCTL_TXFFLSH | GRSTCTL_TXFNUM_ALL);
    let _ = until(10_000_000, || read(regs, GRSTCTL) & GRSTCTL_TXFFLSH == 0);
    write(regs, GRSTCTL, GRSTCTL_RXFFLSH);
    let _ = until(10_000_000, || read(regs, GRSTCTL) & GRSTCTL_RXFFLSH == 0);
    // The host: the PHY clock as the high-speed PHY has it.
    write(regs, HCFG, 0);
    // DMA on, bursts of four, interrupts on.
    write(regs, GAHBCFG, GAHBCFG_DMA_EN | GAHBCFG_HBSTLEN_INCR4 | GAHBCFG_GLBL_INTR_EN);
    // The port's power.
    let hprt0 = read(regs, HPRT0);
    if hprt0 & HPRT0_PWR == 0 {
        write(regs, HPRT0, (hprt0 & !HPRT0_W1C) | HPRT0_PWR);
    }
    // Every channel quiet, the interrupts we take: the port, the channels
    // (each on its halt), a disconnect.
    for ch in 0..channels {
        write(regs, reg_hcintmsk(ch), 0);
        write(regs, reg_hcint(ch), !0);
    }
    write(regs, HAINTMSK, (1u32 << channels) - 1);
    write(regs, GINTSTS, !0);
    write(regs, GINTMSK, GINTSTS_PRTINT | GINTSTS_HCHINT | GINTSTS_DISCONNINT);

    let mut pages = [(0u64, core::ptr::null_mut()); CHANNELS];
    for p in pages.iter_mut().take(channels) {
        let mut bus = 0u64;
        let va = api().dma_alloc(dma, 1, &mut bus);
        if va.is_null() {
            return Err("no memory");
        }
        *p = (bus, va);
    }
    let mut bus = 0u64;
    let (bounce, bounce_len) = {
        let va = api().dma_alloc(dma, BOUNCE_PAGES, &mut bus);
        if !va.is_null() {
            (va, BOUNCE_PAGES * 4096)
        } else {
            let va = api().dma_alloc(dma, 1, &mut bus);
            if va.is_null() {
                return Err("no memory");
            }
            (va, 4096)
        }
    };
    Ok(Controller {
        regs,
        dma,
        channels,
        chans: [const { Chan::new() }; CHANNELS],
        pages,
        bounce: SleepLock::new(&crate::API, Bounce { va: bounce, bus, len: bounce_len }),
        irq_on: AtomicBool::new(false),
        events: AtomicU32::new(0),
        irqs: AtomicU32::new(0),
        core_index: AtomicU32::new(u32::MAX),
    })
}

// ---- The root port ------------------------------------------------------------

/// The port's state: connected, enabled, the device's speed.
pub struct PortStatus {
    pub connected: bool,
    pub enabled: bool,
    pub speed: u8,
}

pub fn port_status(c: &Controller) -> PortStatus {
    let s = read(c.regs, HPRT0);
    PortStatus {
        connected: s & HPRT0_CONNSTS != 0,
        enabled: s & HPRT0_ENA != 0,
        speed: match (s >> HPRT0_SPD_SHIFT) & 3 {
            0 => USB_SPEED_HIGH,
            1 => USB_SPEED_FULL,
            _ => USB_SPEED_LOW,
        },
    }
}

/// Acknowledge the port's change bits (which deasserts its interrupt).
pub fn port_ack(c: &Controller) {
    let s = read(c.regs, HPRT0);
    if s & HPRT0_CHANGES != 0 {
        write(c.regs, HPRT0, (s & !HPRT0_W1C) | (s & HPRT0_CHANGES));
    }
}

/// Reset the port: the device answers at address 0 after; true once the
/// port is enabled.
pub fn port_reset(c: &Controller) -> bool {
    let s = read(c.regs, HPRT0) & !HPRT0_W1C;
    write(c.regs, HPRT0, s | HPRT0_RST);
    crate::sleep_ms(60);
    write(c.regs, HPRT0, s & !HPRT0_RST);
    for _ in 0..20 {
        crate::sleep_ms(5);
        let s = read(c.regs, HPRT0);
        if s & HPRT0_ENA != 0 {
            port_ack(c);
            return true;
        }
    }
    false
}

// ---- Interrupts ------------------------------------------------------------

/// The controller's interrupt: the port's changes to the core, the halted
/// channels into their records.
pub fn process_interrupts(c: &Controller) {
    let sts = read(c.regs, GINTSTS) & read(c.regs, GINTMSK);
    if sts & GINTSTS_DISCONNINT != 0 {
        write(c.regs, GINTSTS, GINTSTS_DISCONNINT);
        crate::core_port_change(c);
    }
    if sts & GINTSTS_PRTINT != 0 {
        port_ack(c);
        crate::core_port_change(c);
    }
    if sts & GINTSTS_HCHINT != 0 {
        let haint = read(c.regs, HAINT);
        for ch in 0..c.channels {
            if haint & (1 << ch) == 0 {
                continue;
            }
            let hcint = read(c.regs, reg_hcint(ch));
            write(c.regs, reg_hcint(ch), hcint);
            if hcint & HCINT_CHHLTD == 0 {
                continue;
            }
            c.events.fetch_add(1, Ordering::Relaxed);
            let chan = &c.chans[ch];
            chan.tsiz.store(read(c.regs, reg_hctsiz(ch)), Ordering::Relaxed);
            chan.hcint.store(hcint, Ordering::Relaxed);
            chan.done.store(true, Ordering::Release);
            if chan.queued.load(Ordering::Acquire) {
                crate::core_wake(c);
            } else {
                api().wake(chan.key());
            }
        }
    }
}

// ---- Channels and transfers ------------------------------------------------

/// A free channel, taken for device `owner`.
pub fn take(c: &Controller, owner: u32) -> Option<usize> {
    (0..c.channels).find(|&ch| {
        let chan = &c.chans[ch];
        if chan.busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed).is_err() {
            return false;
        }
        chan.owner.store(owner, Ordering::Relaxed);
        chan.aborted.store(false, Ordering::Relaxed);
        chan.queued.store(false, Ordering::Relaxed);
        chan.retry_at.store(0, Ordering::Relaxed);
        chan.done.store(false, Ordering::Release);
        true
    })
}

pub fn give_back(c: &Controller, ch: usize) {
    let chan = &c.chans[ch];
    chan.queued.store(false, Ordering::Relaxed);
    chan.retry_at.store(0, Ordering::Relaxed);
    chan.owner.store(0, Ordering::Relaxed);
    chan.busy.store(false, Ordering::Release);
}

/// What a transaction is addressed to: the device, the endpoint, its kind
/// (0 control, 2 bulk, 3 interrupt) and max packet size, and the hub whose
/// transaction translator it goes through.
#[derive(Clone, Copy)]
pub struct Target {
    pub address: u8,
    pub speed: u8,
    pub endpoint: u8,
    pub kind: u8,
    pub max_packet: u16,
    pub tt: Option<(u8, u8)>,
}

impl Target {
    /// `HCCHAR` for a transaction `to_host` or not.
    fn hcchar(&self, to_host: bool) -> u32 {
        let mut v = u32::from(self.max_packet) & HCCHAR_MPS_MASK
            | u32::from(self.endpoint & 0xF) << HCCHAR_EPNUM_SHIFT
            | u32::from(self.kind) << HCCHAR_EPTYPE_SHIFT
            | 1 << HCCHAR_MULTICNT_SHIFT
            | u32::from(self.address) << HCCHAR_DEVADDR_SHIFT;
        if to_host {
            v |= HCCHAR_EPDIR_IN;
        }
        if self.speed == USB_SPEED_LOW {
            v |= HCCHAR_LSPDDEV;
        }
        v
    }

    /// `HCSPLT`: a split transaction's start half, or none.
    fn hcsplt(&self) -> u32 {
        match self.tt {
            Some((hub, port)) => {
                HCSPLT_SPLTENA | HCSPLT_XACTPOS_ALL | u32::from(hub) << HCSPLT_HUBADDR_SHIFT | u32::from(port) << HCSPLT_PRTADDR_SHIFT
            }
            None => 0,
        }
    }
}

/// Program channel `ch` with one transaction set (`packets` of the target's
/// size, `len` bytes at `bus`, the first with `pid`) and enable it; the
/// channel halts into its record when it is over.
pub fn start(c: &Controller, ch: usize, t: &Target, to_host: bool, bus: u64, len: usize, packets: usize, pid: u32, complete_split: bool) {
    let chan = &c.chans[ch];
    chan.done.store(false, Ordering::Release);
    write(c.regs, reg_hcint(ch), !0);
    write(c.regs, reg_hcintmsk(ch), HCINT_CHHLTD);
    let mut split = t.hcsplt();
    if complete_split {
        split |= HCSPLT_COMPSPLT;
    }
    write(c.regs, reg_hcsplt(ch), split);
    write(c.regs, reg_hctsiz(ch), (len as u32 & TSIZ_XFERSIZE_MASK) | ((packets as u32) << TSIZ_PKTCNT_SHIFT) & TSIZ_PKTCNT_MASK | pid << TSIZ_PID_SHIFT);
    write(c.regs, reg_hcdma(ch), bus as u32);
    let mut hcchar = t.hcchar(to_host);
    // A periodic transfer goes in the next frame: its parity.
    if t.kind == 3 && read(c.regs, HFNUM) & 1 == 0 {
        hcchar |= HCCHAR_ODDFRM;
    }
    crate::dma_wmb();
    write(c.regs, reg_hcchar(ch), hcchar | HCCHAR_CHENA);
}

/// A channel that did not halt in time: its registers and the controller's
/// interrupt state, for the log (`[ WARN ] dwc2: channel 0 timed out ...`).
pub fn timed_out(c: &Controller, ch: usize) {
    let mut buf = [0u8; 160];
    let mut w = crate::Writer { buf: &mut buf, len: 0 };
    w.str("dwc2: channel ");
    w.dec(ch as u32);
    w.str(" timed out: hcchar ");
    w.hex(read(c.regs, reg_hcchar(ch)));
    w.str(" hcint ");
    w.hex(read(c.regs, reg_hcint(ch)));
    w.str(" hctsiz ");
    w.hex(read(c.regs, reg_hctsiz(ch)));
    w.str(" haint ");
    w.hex(read(c.regs, HAINT));
    w.str(" gintsts ");
    w.hex(read(c.regs, GINTSTS));
    w.str(" hprt0 ");
    w.hex(read(c.regs, HPRT0));
    let n = w.len;
    myos_abi::status_warn(api(), core::str::from_utf8(&buf[..n]).unwrap_or("dwc2: timed out"));
}

/// Stop channel `ch`, whatever it is doing: disabled, then halted.
pub fn disable(c: &Controller, ch: usize) {
    let v = read(c.regs, reg_hcchar(ch));
    if v & HCCHAR_CHENA != 0 {
        write(c.regs, reg_hcchar(ch), (v & !HCCHAR_CHENA) | HCCHAR_CHDIS | HCCHAR_CHENA);
        let _ = until(10_000_000, || c.chans[ch].done.load(Ordering::Acquire) || read(c.regs, reg_hcint(ch)) & HCINT_CHHLTD != 0);
        write(c.regs, reg_hcint(ch), !0);
    }
}

/// How a halted channel's transaction set went: the bytes moved and the
/// packets it took, or what stopped it.
pub enum Halt {
    /// Every packet moved (or a short one ended the set).
    Done { bytes: usize, packets: usize },
    /// The device had nothing (IN) or no room (OUT) yet.
    Nak,
    /// A split transaction's start half was taken: its complete half next.
    Ack,
    /// A split transaction's complete half is not ready yet: again.
    Nyet,
    Error(i32),
}

/// Read channel `ch`'s halt: `len` and `packets` what it was started with.
pub fn halt(c: &Controller, ch: usize, len: usize, packets: usize) -> Halt {
    let chan = &c.chans[ch];
    if chan.aborted.load(Ordering::Acquire) {
        return Halt::Error(USB_EGONE);
    }
    let hcint = chan.hcint.load(Ordering::Acquire);
    let tsiz = chan.tsiz.load(Ordering::Relaxed);
    if hcint & HCINT_XFERCOMPL != 0 {
        let left = (tsiz & TSIZ_XFERSIZE_MASK) as usize;
        let pkts_left = ((tsiz & TSIZ_PKTCNT_MASK) >> TSIZ_PKTCNT_SHIFT) as usize;
        return Halt::Done { bytes: len.saturating_sub(left), packets: packets.saturating_sub(pkts_left) };
    }
    if hcint & HCINT_STALL != 0 {
        return Halt::Error(USB_ESTALL);
    }
    if hcint & HCINT_ERRORS != 0 {
        return Halt::Error(USB_EIO);
    }
    if hcint & HCINT_NAK != 0 {
        return Halt::Nak;
    }
    if hcint & HCINT_NYET != 0 {
        return Halt::Nyet;
    }
    if hcint & HCINT_ACK != 0 {
        return Halt::Ack;
    }
    // Halted with nothing said: disabled (a timeout, `device_abort`).
    Halt::Error(USB_ETIMEDOUT)
}

/// Wait for channel `ch` to halt, until `deadline` (monotonic ns): false on
/// timeout. The interrupt handler wakes the waiter; the waiter also looks
/// itself every few milliseconds, so a controller whose interrupt does
/// not reach us still works.
pub fn wait(c: &Controller, ch: usize, deadline: u64) -> bool {
    let chan = &c.chans[ch];
    loop {
        if chan.done.load(Ordering::Acquire) || chan.aborted.load(Ordering::Acquire) {
            return true;
        }
        let seq = api().wait_seq();
        if !c.irq_on.load(Ordering::Relaxed) {
            process_interrupts(c);
        }
        if chan.done.load(Ordering::Acquire) || chan.aborted.load(Ordering::Acquire) {
            return true;
        }
        let now = api().monotonic_ns();
        if now >= deadline {
            return false;
        }
        api().block_until(chan.key(), seq, deadline.min(now + 2_000_000));
    }
}

/// Move `len` bytes at `bus` (the direction `to_host`) through channel
/// `ch` to `t`, starting with `pid`, waiting until `deadline`: the bytes
/// moved and the packets it took. A NAK is retried until the deadline (a
/// bulk or control endpoint: the device is busy); a split transaction's
/// halves and retries are run here too.
pub fn transfer(c: &Controller, ch: usize, t: &Target, to_host: bool, bus: u64, len: usize, pid: u32, deadline: u64) -> Result<(usize, usize), i32> {
    let mps = usize::from(t.max_packet).max(1);
    // Behind a transaction translator a transaction set is one packet.
    let per_set = if t.tt.is_some() { mps } else { (MAX_XFER / mps * mps).min(MAX_PACKETS * mps) };
    let mut moved = 0usize;
    let mut packets = 0usize;
    let mut pid = pid;
    loop {
        let chunk = (len - moved).min(per_set);
        let n = chunk.div_ceil(mps).max(1);
        let (bytes, pkts) = set(c, ch, t, to_host, bus + moved as u64, chunk, n, pid, deadline)?;
        moved += bytes;
        packets += pkts;
        if pkts & 1 != 0 {
            pid = if pid == PID_DATA0 { PID_DATA1 } else { PID_DATA0 };
        }
        // A short packet, or everything, ends the transfer.
        if bytes < chunk || moved >= len || chunk == 0 {
            return Ok((moved, packets));
        }
    }
}

/// One transaction set, with its retries and, behind a transaction
/// translator, its complete half.
fn set(c: &Controller, ch: usize, t: &Target, to_host: bool, bus: u64, len: usize, packets: usize, pid: u32, deadline: u64) -> Result<(usize, usize), i32> {
    loop {
        start(c, ch, t, to_host, bus, len, packets, pid, false);
        if !wait(c, ch, deadline) {
            timed_out(c, ch);
            disable(c, ch);
            return Err(USB_ETIMEDOUT);
        }
        match halt(c, ch, len, packets) {
            Halt::Done { bytes, packets } => return Ok((bytes, packets)),
            Halt::Nak | Halt::Nyet => {
                // The device is busy: again, a little later.
                if api().monotonic_ns() >= deadline {
                    return Err(USB_ETIMEDOUT);
                }
                crate::sleep_ms(1);
            }
            Halt::Ack if t.tt.is_some() => {
                // The start half was taken: the complete half, until the
                // translator has the answer.
                loop {
                    start(c, ch, t, to_host, bus, len, packets, pid, true);
                    if !wait(c, ch, deadline) {
                        disable(c, ch);
                        return Err(USB_ETIMEDOUT);
                    }
                    match halt(c, ch, len, packets) {
                        Halt::Done { bytes, packets } => return Ok((bytes, packets)),
                        Halt::Nyet => {
                            if api().monotonic_ns() >= deadline {
                                return Err(USB_ETIMEDOUT);
                            }
                            // The next microframe.
                            crate::sleep_ms(1);
                        }
                        Halt::Nak => break, // the start half again
                        Halt::Ack => break,
                        Halt::Error(e) => return Err(e),
                    }
                }
                crate::sleep_ms(1);
            }
            Halt::Ack => return Err(USB_EIO),
            Halt::Error(e) => return Err(e),
        }
    }
}

/// Hand `len` bytes at `va` over to the controller, or read what it wrote.
pub fn sync_to(c: &Controller, va: *mut u8, len: usize) {
    api().dma_sync(c.dma, va, len.max(1), DMA_TO_DEVICE);
}

pub fn sync_from(c: &Controller, va: *mut u8, len: usize) {
    api().dma_sync(c.dma, va, len.max(1), DMA_FROM_DEVICE);
}

/// `len` bytes at `va`, as a slice.
pub unsafe fn bytes(va: *mut u8, len: usize) -> &'static mut [u8] {
    unsafe { core::slice::from_raw_parts_mut(va, len) }
}
