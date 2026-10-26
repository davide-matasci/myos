//! The UHCI controller (Universal Host Controller Interface 1.1): its I/O
//! registers, its frame list and the transfers on it.
//!
//! Every one of the frame list's 1024 entries leads, after the
//! isochronous TDs queued for that frame, to the same three skeleton
//! queue heads (QH): interrupt, control, bulk, in that order. Each
//! endpoint has a QH of its own, linked in after its kind's skeleton when
//! the endpoint is set up and out when it goes (two frames later nothing
//! of it is in use). A transfer is a chain of transfer descriptors (TD),
//! one packet each, put on the endpoint's idle QH; the controller runs
//! them depth first and moves the QH's element pointer along. The data
//! toggle is in each TD: this module keeps an endpoint's.
//!
//! An isochronous OUT packet is a TD of its own in the frame list entry
//! of the frame it is to go out in, a few frames ahead of the controller;
//! taken out again once it went (`isoch_reap`).
//!
//! The registers are in PCI I/O space ([`PciIo`]); every structure the
//! controller reads is below 4 GiB (`DmaDevice::below`).

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering, compiler_fence};

use myos_abi::{
    DMA_FROM_DEVICE, DMA_TO_DEVICE, Lock, PciIo, SleepLock, USB_EGONE, USB_EIO, USB_ESTALL, USB_ETIMEDOUT,
    USB_SPEED_FULL, USB_SPEED_LOW,
};

use crate::{API, api, sleep_ms};

pub const MAX_CTRL: usize = 8;
pub const PORTS: u8 = 2;
const PAGE: usize = 4096;
/// A pipe's page: its QH, then TDs.
const TD_AT: usize = 16;
const TD_STRIDE: usize = 16;
pub const MAX_TDS: usize = 250;
/// The controller's bulk bounce buffer.
pub const BOUNCE_PAGES: usize = 4;
/// Isochronous packets on an endpoint at most (`USB_ISOCH_MAX`).
pub const ISO_TDS: usize = 64;

const USBCMD: u32 = 0x00;
const USBSTS: u32 = 0x02;
const USBINTR: u32 = 0x04;
const FRNUM: u32 = 0x06;
const FRBASEADD: u32 = 0x08;
const SOFMOD: u32 = 0x0C;
const PORTSC: u32 = 0x10;

const CMD_RUN: u16 = 1 << 0;
const CMD_HCRESET: u16 = 1 << 1;
const CMD_GRESET: u16 = 1 << 2;
const CMD_CF: u16 = 1 << 6;
const CMD_MAXP64: u16 = 1 << 7;

const STS_ACK: u16 = 0x1F;
const INTR_TIMEOUT_CRC: u16 = 1 << 0;
const INTR_IOC: u16 = 1 << 2;
const INTR_SHORT: u16 = 1 << 3;

const PORT_CONNECT: u16 = 1 << 0;
const PORT_CONNECT_CHANGE: u16 = 1 << 1;
const PORT_ENABLE: u16 = 1 << 2;
const PORT_ENABLE_CHANGE: u16 = 1 << 3;
const PORT_LOW_SPEED: u16 = 1 << 8;
const PORT_RESET: u16 = 1 << 9;
/// The bits a write sets (the rest read-only or write-1-to-clear).
const PORT_RW: u16 = PORT_ENABLE | 1 << 6 | PORT_RESET | 1 << 12;

const LINK_TERMINATE: u32 = 1;
const LINK_QH: u32 = 1 << 1;
const LINK_DEPTH: u32 = 1 << 2;

// TD control and status.
const TD_ACTIVE: u32 = 1 << 23;
const TD_STALLED: u32 = 1 << 22;
const TD_DATABUF: u32 = 1 << 21;
const TD_BABBLE: u32 = 1 << 20;
const TD_CRC_TIMEOUT: u32 = 1 << 18;
const TD_BITSTUFF: u32 = 1 << 17;
const TD_IOC: u32 = 1 << 24;
const TD_ISO: u32 = 1 << 25;
const TD_LOW_SPEED: u32 = 1 << 26;
const TD_CERR_3: u32 = 3 << 27;
const TD_SPD: u32 = 1 << 29;

pub const PID_SETUP: u32 = 0x2D;
pub const PID_IN: u32 = 0x69;
pub const PID_OUT: u32 = 0xE1;

/// A transfer's endpoint.
#[derive(Clone, Copy)]
pub struct Target {
    pub address: u8,
    pub endpoint: u8,
    pub speed: u8,
    pub max_packet: u16,
}

/// The skeleton a pipe's QH goes after.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Interrupt = 0,
    Control = 1,
    Bulk = 2,
}

/// An endpoint's QH and its TDs (one page), and a data page for a control
/// or queued endpoint's transfers.
#[derive(Clone, Copy)]
pub struct Pipe {
    pub bus: u64,
    pub va: *mut u8,
    pub data_bus: u64,
    pub data: *mut u8,
    pub kind: Kind,
}

unsafe impl Send for Pipe {}

fn rd(p: *mut u8, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile(p.add(off).cast::<u32>()) }
}

fn wr(p: *mut u8, off: usize, v: u32) {
    unsafe { core::ptr::write_volatile(p.add(off).cast::<u32>(), v) }
}

impl Pipe {
    pub fn td_bus(&self, i: usize) -> u32 {
        (self.bus + (TD_AT + i * TD_STRIDE) as u64) as u32
    }

    fn td(&self, i: usize) -> *mut u8 {
        unsafe { self.va.add(TD_AT + i * TD_STRIDE) }
    }

    pub fn status(&self, i: usize) -> u32 {
        rd(self.td(i), 4)
    }

    fn element(&self) -> u32 {
        rd(self.va, 4)
    }

    fn set_element(&self, v: u32) {
        wr(self.va, 4, v)
    }

    /// TD `i`: one packet of `len` bytes at `buf` to or from `t`, with
    /// toggle `dt`; the last of the chain when `last` (interrupting).
    pub fn set_td(&self, i: usize, t: &Target, pid: u32, dt: bool, buf: u64, len: usize, last: bool) {
        let td = self.td(i);
        let link = if last { LINK_TERMINATE } else { self.td_bus(i + 1) | LINK_DEPTH };
        let maxlen = if len == 0 { 0x7FF } else { (len as u32 - 1) & 0x7FF };
        let token = pid | u32::from(t.address & 0x7F) << 8 | u32::from(t.endpoint & 0xF) << 15 | u32::from(dt) << 19 | maxlen << 21;
        let status = TD_ACTIVE
            | TD_CERR_3
            | if pid == PID_IN { TD_SPD } else { 0 }
            | if t.speed == USB_SPEED_LOW { TD_LOW_SPEED } else { 0 }
            | if last { TD_IOC } else { 0 };
        wr(td, 0, link);
        wr(td, 8, token);
        wr(td, 12, buf as u32);
        wr(td, 4, status);
    }
}

/// An isochronous OUT endpoint's packets: TDs (one page) and their data
/// slots, used in turn; the frame each TD waits for.
pub struct Iso {
    pub tds_bus: u64,
    pub tds: *mut u8,
    pub data_bus: u64,
    pub data: *mut u8,
    pub data_pages: usize,
    pub slot: usize,
    pub next: usize,
    /// Per TD: the frame it is in (`u16::MAX`: free).
    pub frame: [u16; ISO_TDS],
    /// The frame after the last one queued.
    pub tail: u16,
    pub in_flight: usize,
    /// Completed, not reaped yet.
    pub done: u32,
}

unsafe impl Send for Iso {}

pub struct Bounce {
    pub bus: u64,
    pub va: *mut u8,
}

unsafe impl Send for Bounce {}

pub struct Controller {
    pub io: PciIo,
    /// The PCI function (a rescan skips it).
    pub pci: (u8, u8, u8),
    pub dma: i32,
    frames: *mut u32,
    /// The three skeleton QHs (interrupt, control, bulk), in one page.
    skel_bus: u64,
    skel: *mut u8,
    pub bounce: SleepLock<Bounce>,
    /// Schedule edits one at a time; the QHs linked, `(bus, va)`, for
    /// walking the chains.
    schedule: Lock<Linked>,
    pub irq_on: AtomicBool,
    pub core_index: AtomicU32,
    pub irqs: AtomicU32,
    pub events: AtomicU32,
}

unsafe impl Sync for Controller {}
unsafe impl Send for Controller {}

pub const MAX_LINKED: usize = 128;

pub struct Linked {
    qhs: [(u32, usize); MAX_LINKED],
    n: usize,
}

impl Controller {
    pub fn key(&self) -> usize {
        self as *const Self as usize
    }

    fn skel_qh(&self, k: Kind) -> *mut u8 {
        unsafe { self.skel.add(k as usize * 32) }
    }

    fn skel_bus(&self, k: Kind) -> u32 {
        (self.skel_bus + k as u64 * 32) as u32
    }

    pub fn frame_number(&self) -> u16 {
        self.io.read16(FRNUM) & 0x7FF
    }
}

pub fn wmb() {
    compiler_fence(Ordering::Release);
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("fence rw,rw", options(nostack, preserves_flags));
    }
}

pub fn sync_to(c: &Controller, va: *mut u8, len: usize) {
    api().dma_sync(c.dma, va, len, DMA_TO_DEVICE);
}

pub fn sync_from(c: &Controller, va: *mut u8, len: usize) {
    api().dma_sync(c.dma, va, len, DMA_FROM_DEVICE);
    compiler_fence(Ordering::SeqCst);
}

// ---- Pages ------------------------------------------------------------------

const POOL: usize = 64;
static FREE: Lock<([(i32, u64, usize); POOL], usize)> = Lock::new(([(-1, 0, 0); POOL], 0));

/// `n` zeroed contiguous pages below 4 GiB (one from the pool when it has
/// one: the kernel's DMA pages are never returned).
pub fn pages(dma: i32, n: usize) -> Option<(u64, *mut u8)> {
    if n == 1 {
        let recycled = {
            let mut pool = FREE.lock();
            let k = pool.1;
            let hit = pool.0[..k].iter().rposition(|&(d, _, _)| d == dma);
            hit.map(|i| {
                let page = pool.0[i];
                pool.0[i] = pool.0[k - 1];
                pool.1 = k - 1;
                page
            })
        };
        if let Some((_, bus, va)) = recycled {
            let va = va as *mut u8;
            unsafe { core::ptr::write_bytes(va, 0, PAGE) };
            api().dma_sync(dma, va, PAGE, DMA_TO_DEVICE);
            return Some((bus, va));
        }
    }
    let mut bus = 0u64;
    let va = api().dma_alloc(dma, n, &mut bus);
    if va.is_null() || bus + (n * PAGE) as u64 > 1 << 32 {
        return None;
    }
    Some((bus, va))
}

pub fn free_page(dma: i32, bus: u64, va: *mut u8) {
    let mut pool = FREE.lock();
    let k = pool.1;
    if k < POOL {
        pool.0[k] = (dma, bus, va as usize);
        pool.1 = k + 1;
    }
}

// ---- Bring-up ---------------------------------------------------------------

/// Take the controller from the firmware's legacy support (LEGSUP, in its
/// PCI configuration space at 0xC0: SMIs off, its interrupt routed to the
/// PIRQ line), reset it, and start it on the skeleton schedule.
pub fn init(io: PciIo, pci: (u8, u8, u8), dma: i32) -> Result<Controller, &'static str> {
    let api = api();
    let (b, s, f) = pci;
    let legsup = api.pci_cfg_read32(b, s, f, 0xC0);
    api.pci_cfg_write32(b, s, f, 0xC0, (legsup & 0xFFFF_0000) | 0x8F00);
    // Global reset (10 ms at least), then the controller's own.
    io.write16(USBCMD, CMD_GRESET);
    sleep_ms(50);
    io.write16(USBCMD, 0);
    sleep_ms(10);
    io.write16(USBCMD, CMD_HCRESET);
    let mut reset = false;
    for _ in 0..100 {
        if io.read16(USBCMD) & CMD_HCRESET == 0 {
            reset = true;
            break;
        }
        sleep_ms(1);
    }
    if !reset {
        return Err("reset timed out");
    }
    let (frames_bus, frames_va) = pages(dma, 1).ok_or("no DMA page")?;
    let (skel_bus, skel_va) = pages(dma, 1).ok_or("no DMA page")?;
    let (bounce_bus, bounce_va) = pages(dma, BOUNCE_PAGES).ok_or("no DMA pages")?;
    let c = Controller {
        io,
        pci,
        dma,

        frames: frames_va.cast(),
        skel_bus,
        skel: skel_va,
        bounce: SleepLock::new(&API, Bounce { bus: bounce_bus, va: bounce_va }),
        schedule: Lock::new(Linked { qhs: [(0, 0); MAX_LINKED], n: 0 }),
        irq_on: AtomicBool::new(false),
        core_index: AtomicU32::new(u32::MAX),
        irqs: AtomicU32::new(0),
        events: AtomicU32::new(0),
    };
    // Skeletons: interrupt -> control -> bulk -> end, all empty.
    for k in [Kind::Interrupt, Kind::Control, Kind::Bulk] {
        let next = match k {
            Kind::Interrupt => c.skel_bus(Kind::Control) | LINK_QH,
            Kind::Control => c.skel_bus(Kind::Bulk) | LINK_QH,
            Kind::Bulk => LINK_TERMINATE,
        };
        wr(c.skel_qh(k), 0, next);
        wr(c.skel_qh(k), 4, LINK_TERMINATE);
    }
    for i in 0..1024 {
        unsafe { core::ptr::write_volatile(c.frames.add(i), c.skel_bus(Kind::Interrupt) | LINK_QH) };
    }
    sync_to(&c, skel_va, PAGE);
    sync_to(&c, frames_va, PAGE);
    io.write16(USBINTR, 0);
    io.write16(USBSTS, STS_ACK);
    io.write32(FRBASEADD, frames_bus as u32);
    io.write16(FRNUM, 0);
    io.write8(SOFMOD, 64);
    io.write16(USBCMD, CMD_RUN | CMD_CF | CMD_MAXP64);
    // The interrupt to the PIRQ line, the legacy traps off.
    api.pci_cfg_write32(b, s, f, 0xC0, (legsup & 0xFFFF_0000) | 0x2000);
    Ok(c)
}

pub fn enable_interrupts(c: &Controller) {
    c.io.write16(USBINTR, INTR_IOC | INTR_SHORT | INTR_TIMEOUT_CRC);
}

/// The controller's status, acknowledged: a transfer done (or failed)
/// wakes its waiters and the core.
pub fn process(c: &Controller) {
    let sts = c.io.read16(USBSTS) & STS_ACK;
    if sts == 0 {
        return;
    }
    c.io.write16(USBSTS, sts);
    c.events.fetch_add(1, Ordering::Relaxed);
    api().wake(c.key());
    crate::core_wake(c);
}

/// Port connection changes (UHCI raises no interrupt for them): the core
/// hears of each.
pub fn poll_ports(c: &Controller) {
    for p in 0..PORTS {
        if c.io.read16(PORTSC + 2 * u32::from(p)) & (PORT_CONNECT_CHANGE | PORT_ENABLE_CHANGE) != 0 {
            crate::core_port_change(c, p);
        }
    }
}

// ---- Ports ------------------------------------------------------------------

pub struct PortState {
    pub connected: bool,
    pub enabled: bool,
    pub speed: u8,
}

/// Root port `port`, its change bits acknowledged.
pub fn port_status(c: &Controller, port: u8) -> PortState {
    let reg = PORTSC + 2 * u32::from(port);
    let v = c.io.read16(reg);
    c.io.write16(reg, (v & PORT_RW) | (v & (PORT_CONNECT_CHANGE | PORT_ENABLE_CHANGE)));
    PortState {
        connected: v & PORT_CONNECT != 0,
        enabled: v & PORT_ENABLE != 0,
        speed: if v & PORT_LOW_SPEED != 0 { USB_SPEED_LOW } else { USB_SPEED_FULL },
    }
}

/// Reset root port `port` (50 ms), then enable it (the controller does not
/// on its own).
pub fn port_reset(c: &Controller, port: u8) -> bool {
    let reg = PORTSC + 2 * u32::from(port);
    let v = c.io.read16(reg) & PORT_RW;
    c.io.write16(reg, v | PORT_RESET);
    sleep_ms(50);
    let v = c.io.read16(reg) & PORT_RW;
    c.io.write16(reg, v & !PORT_RESET);
    sleep_ms(1);
    for _ in 0..10 {
        let v = c.io.read16(reg);
        if v & PORT_CONNECT == 0 {
            return false;
        }
        if v & PORT_ENABLE != 0 {
            c.io.write16(reg, (v & PORT_RW) | PORT_CONNECT_CHANGE | PORT_ENABLE_CHANGE);
            sleep_ms(10);
            return true;
        }
        c.io.write16(reg, (v & PORT_RW) | PORT_ENABLE);
        sleep_ms(10);
    }
    false
}

// ---- Pipes ------------------------------------------------------------------

/// A new endpoint's QH (and a data page when `data`), idle, linked in
/// after its kind's skeleton.
pub fn pipe_new(c: &Controller, kind: Kind, data: bool) -> Option<Pipe> {
    let (bus, va) = pages(c.dma, 1)?;
    let (data_bus, data_va) = if data {
        match pages(c.dma, 1) {
            Some(p) => p,
            None => {
                free_page(c.dma, bus, va);
                return None;
            }
        }
    } else {
        (0, core::ptr::null_mut())
    };
    let p = Pipe { bus, va, data_bus, data: data_va, kind };
    p.set_element(LINK_TERMINATE);
    link(c, &p);
    Some(p)
}

fn link(c: &Controller, p: &Pipe) {
    let mut linked = c.schedule.lock();
    if linked.n < MAX_LINKED {
        let n = linked.n;
        linked.qhs[n] = (p.bus as u32, p.va as usize);
        linked.n = n + 1;
    }
    let skel = c.skel_qh(p.kind);
    wr(p.va, 0, rd(skel, 0));
    sync_to(c, p.va, PAGE);
    wmb();
    wr(skel, 0, p.bus as u32 | LINK_QH);
    sync_to(c, c.skel, PAGE);
}

/// Unlink `p`; two frames later the controller has nothing of it.
fn unlink(c: &Controller, p: &Pipe) {
    {
        let mut linked = c.schedule.lock();
        if let Some(i) = linked.qhs[..linked.n].iter().position(|q| q.0 == p.bus as u32) {
            let n = linked.n;
            linked.qhs[i] = linked.qhs[n - 1];
            linked.n = n - 1;
        }
        let target = p.bus as u32 | LINK_QH;
        let mut at = c.skel_qh(p.kind);
        for _ in 0..=MAX_LINKED {
            let next = rd(at, 0);
            if next == target {
                wr(at, 0, rd(p.va, 0));
                sync_to(c, at, 16);
                break;
            }
            match linked.qhs[..linked.n].iter().find(|q| q.0 == next & !0xF) {
                Some(q) => at = q.1 as *mut u8,
                None => break,
            }
        }
    }
    wmb();
    sleep_ms(2);
}

pub fn pipe_free(c: &Controller, p: &Pipe) {
    unlink(c, p);
    free_page(c.dma, p.bus, p.va);
    if !p.data.is_null() {
        free_page(c.dma, p.data_bus, p.data);
    }
}

/// Put the TDs written (from the first) on the idle QH `p`.
pub fn start(c: &Controller, p: &Pipe) {
    sync_to(c, p.va, PAGE);
    wmb();
    p.set_element(p.td_bus(0));
    sync_to(c, p.va, 16);
    wmb();
}

/// The queue empty again (after a stall, a short packet or a cancel).
pub fn clear(c: &Controller, p: &Pipe) {
    p.set_element(LINK_TERMINATE);
    for i in 0..MAX_TDS {
        wr(p.td(i), 4, 0);
    }
    sync_to(c, p.va, PAGE);
    wmb();
}

/// What the `n` TDs on `p` came to: `None` while they run; else the bytes
/// the data packets moved and how many went (for the toggle), or the
/// error. `status` is a control transfer's status TD (the setup TD 0, the
/// data TDs between). A short IN packet stops the queue at its TD (SPD):
/// that ends a bulk or interrupt transfer, and a control transfer's data
/// stage, whose queue then goes on at the status TD.
pub fn outcome(c: &Controller, p: &Pipe, n: usize, status: Option<usize>) -> Option<Result<(usize, usize), i32>> {
    sync_from(c, p.va, PAGE);
    for i in 0..n {
        let s = p.status(i);
        if s & TD_STALLED != 0 {
            return Some(Err(if s & (TD_BABBLE | TD_DATABUF | TD_CRC_TIMEOUT | TD_BITSTUFF) != 0 { USB_EIO } else { USB_ESTALL }));
        }
    }
    let data = match status {
        Some(st) => 1..st,
        None => 0..n,
    };
    if status.is_some() && p.status(0) & TD_ACTIVE != 0 {
        return None;
    }
    let (mut moved, mut packets, mut short) = (0, 0, false);
    for i in data {
        let s = p.status(i);
        if s & TD_ACTIVE != 0 {
            return None;
        }
        // Lengths are n - 1, 0x7FF for none.
        let actual = ((s + 1) & 0x7FF) as usize;
        let token = rd(p.td(i), 8);
        let asked = (((token >> 21) + 1) & 0x7FF) as usize;
        moved += actual;
        packets += 1;
        if token & 0xFF == PID_IN && actual < asked {
            short = true;
            break;
        }
    }
    match status {
        None => {
            if short {
                p.set_element(LINK_TERMINATE);
                sync_to(c, p.va, 16);
            }
            Some(Ok((moved, packets)))
        }
        Some(st) => {
            if p.status(st) & TD_ACTIVE == 0 {
                return Some(Ok((moved, packets)));
            }
            if short && p.element() & !0xF != p.td_bus(st) {
                p.set_element(p.td_bus(st));
                sync_to(c, p.va, 16);
            }
            None
        }
    }
}

/// Wait for the `n` TDs on `p` until `deadline`: their outcome, or a
/// timeout (or [`USB_EGONE`] when `gone` is raised) with the queue
/// emptied.
pub fn wait(c: &Controller, p: &Pipe, n: usize, status: Option<usize>, deadline: u64, gone: &AtomicBool) -> Result<(usize, usize), i32> {
    loop {
        let seq = api().wait_seq();
        if !c.irq_on.load(Ordering::Relaxed) {
            process(c);
        }
        if let Some(r) = outcome(c, p, n, status) {
            if r.is_err() {
                clear(c, p);
            }
            return r;
        }
        let now = api().monotonic_ns();
        if gone.load(Ordering::Acquire) || now >= deadline {
            clear(c, p);
            sleep_ms(2);
            return Err(if gone.load(Ordering::Acquire) { USB_EGONE } else { USB_ETIMEDOUT });
        }
        let step = if c.irq_on.load(Ordering::Relaxed) { 10_000_000 } else { 1_000_000 };
        api().block_until(c.key(), seq, (now + step).min(deadline));
    }
}

// ---- Isochronous ------------------------------------------------------------

/// An isochronous OUT endpoint's packet ring for packets of up to
/// `max_packet` bytes.
pub fn iso_new(c: &Controller, max_packet: u16) -> Option<Iso> {
    let slot = usize::from(max_packet.max(16)).next_power_of_two().min(1024);
    let data_pages = (ISO_TDS * slot).div_ceil(PAGE);
    let (tds_bus, tds) = pages(c.dma, 1)?;
    let (data_bus, data) = pages(c.dma, data_pages)?;
    Some(Iso { tds_bus, tds, data_bus, data, data_pages, slot, next: 0, frame: [u16::MAX; ISO_TDS], tail: 0, in_flight: 0, done: 0 })
}

/// Queue `lens` packets from `data` on `t`, one a frame after the last one
/// queued (at least two frames ahead of the controller): `USB_EIO` when
/// the ring has no room.
pub fn iso_queue(c: &Controller, iso: &mut Iso, t: &Target, data: &[u8], lens: &[u16]) -> i32 {
    iso_reap(c, iso);
    if iso.in_flight + lens.len() > ISO_TDS {
        return myos_abi::USB_EBUSY;
    }
    let now = c.frame_number();
    // Behind the controller (or a fresh start): from two frames ahead.
    let ahead = iso.tail.wrapping_sub(now) & 0x7FF;
    if iso.in_flight == 0 || ahead < 2 || ahead > 1000 {
        iso.tail = (now + 2) & 0x7FF;
    }
    let mut at = 0usize;
    for (k, &len) in lens.iter().enumerate() {
        let len = usize::from(len).min(usize::from(t.max_packet)).min(iso.slot);
        let i = iso.next;
        iso.next = (iso.next + 1) % ISO_TDS;
        let buf = unsafe { iso.data.add(i * iso.slot) };
        unsafe { core::ptr::copy_nonoverlapping(data[at..].as_ptr(), buf, len) };
        sync_to(c, buf, len.max(1));
        at += len;
        let frame = iso.tail & 0x3FF;
        iso.tail = (iso.tail + 1) & 0x7FF;
        let td = unsafe { iso.tds.add(i * 32) };
        let entry = unsafe { c.frames.add(usize::from(frame)) };
        let maxlen = if len == 0 { 0x7FF } else { (len as u32 - 1) & 0x7FF };
        let token = PID_OUT | u32::from(t.address & 0x7F) << 8 | u32::from(t.endpoint & 0xF) << 15 | maxlen << 21;
        let ioc = k + 1 == lens.len() || k % 8 == 7;
        wr(td, 0, unsafe { core::ptr::read_volatile(entry) });
        wr(td, 8, token);
        wr(td, 12, (iso.data_bus + (i * iso.slot) as u64) as u32);
        wr(td, 4, TD_ACTIVE | TD_ISO | if ioc { TD_IOC } else { 0 });
        sync_to(c, td, 32);
        wmb();
        unsafe { core::ptr::write_volatile(entry, (iso.tds_bus + (i * 32) as u64) as u32) };
        sync_to(c, entry.cast(), 4);
        iso.frame[i] = frame;
        iso.in_flight += 1;
    }
    0
}

/// The packets that went since the last look (their TDs taken out of the
/// frame list), added to `iso.done`.
pub fn iso_reap(c: &Controller, iso: &mut Iso) {
    let now = c.frame_number() & 0x3FF;
    for i in 0..ISO_TDS {
        let frame = iso.frame[i];
        if frame == u16::MAX {
            continue;
        }
        let td = unsafe { iso.tds.add(i * 32) };
        sync_from(c, td, 32);
        // Done, or its frame passed without it (missed: counts as gone).
        let passed = (now.wrapping_sub(frame) & 0x3FF) < 512 && now != frame;
        if rd(td, 4) & TD_ACTIVE != 0 && !passed {
            continue;
        }
        let bus = (iso.tds_bus + (i * 32) as u64) as u32;
        let entry = unsafe { c.frames.add(usize::from(frame)) };
        if unsafe { core::ptr::read_volatile(entry) } == bus {
            unsafe { core::ptr::write_volatile(entry, rd(td, 0)) };
        } else {
            // Another endpoint's TD went in front of it: unlink from there.
            let mut at = unsafe { core::ptr::read_volatile(entry) };
            while at & (LINK_TERMINATE | LINK_QH) == 0 {
                let off = (u64::from(at & !0xF) - iso.tds_bus) as usize;
                if off >= PAGE {
                    break;
                }
                let prev = unsafe { iso.tds.add(off) };
                if rd(prev, 0) == bus {
                    wr(prev, 0, rd(td, 0));
                    break;
                }
                at = rd(prev, 0);
            }
        }
        sync_to(c, entry.cast(), 4);
        iso.frame[i] = u16::MAX;
        iso.in_flight -= 1;
        iso.done += 1;
    }
}

/// Every packet of the ring out of the frame list (the endpoint goes).
pub fn iso_free(c: &Controller, mut iso: Iso) {
    for i in 0..ISO_TDS {
        if iso.frame[i] != u16::MAX {
            wr(unsafe { iso.tds.add(i * 32) }, 4, 0);
        }
    }
    sleep_ms(3);
    iso_reap(c, &mut iso);
    // Missed TDs not reaped yet (their frame far ahead) go when theirs
    // passes: wait the list round once.
    if iso.in_flight != 0 {
        sleep_ms(1030);
        iso_reap(c, &mut iso);
    }
    free_page(c.dma, iso.tds_bus, iso.tds);
    for k in 0..iso.data_pages {
        free_page(c.dma, iso.data_bus + (k * PAGE) as u64, unsafe { iso.data.add(k * PAGE) });
    }
}
