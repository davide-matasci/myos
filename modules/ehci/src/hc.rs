//! The EHCI controller (Enhanced Host Controller Interface 1.0): its
//! registers, its two schedules and the transfers on them.
//!
//! The asynchronous schedule is a ring of queue heads (QH) the controller
//! walks over and over: a head QH that is never removed (the reclamation
//! head), then one QH per control or bulk endpoint, linked in when the
//! endpoint is set up and out (with the async advance doorbell) when it
//! goes. The periodic schedule is the frame list, every one of its 1024
//! entries pointing at one anchor QH, after which the interrupt
//! endpoints' QHs hang: each is visited once a frame, which polls an
//! interrupt endpoint at least as often as it asks.
//!
//! A transfer is a chain of transfer descriptors (qTD) put on the
//! endpoint's QH while it is idle; the waiter (or the core, for a queued
//! transfer) looks at the qTDs' tokens when the controller interrupts,
//! or every millisecond without an interrupt. The QH keeps the data
//! toggle of bulk and interrupt endpoints (DTC clear); control transfers
//! carry theirs in the qTDs.
//!
//! Every structure the controller reads is in pages below 4 GiB
//! (`DmaDevice::below`): 32-bit pointers, the segment register 0.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering, compiler_fence};

use myos_abi::{
    DMA_FROM_DEVICE, DMA_TO_DEVICE, Lock, SleepLock, USB_EGONE, USB_EIO, USB_ESTALL, USB_ETIMEDOUT, USB_SPEED_FULL,
    USB_SPEED_HIGH, USB_SPEED_LOW,
};

use crate::{API, api, sleep_ms};

/// Controllers the module drives.
pub const MAX_CTRL: usize = 4;
/// Root ports one controller has at most (HCSPARAMS N_PORTS is 4 bits).
pub const MAX_PORTS: usize = 15;
const PAGE: usize = 4096;
/// A pipe's page: its QH (68 bytes with the 64-bit words), then qTDs.
const QTD_AT: usize = 128;
const QTD_STRIDE: usize = 64;
pub const MAX_QTDS: usize = 8;
/// The controller's bulk bounce buffer: 4 pages, one qTD's reach.
pub const BOUNCE_PAGES: usize = 4;

// Capability registers.
const CAPLENGTH: usize = 0x00;
const HCSPARAMS: usize = 0x04;
const HCCPARAMS: usize = 0x08;
// Operational registers (after CAPLENGTH).
const USBCMD: usize = 0x00;
const USBSTS: usize = 0x04;
const USBINTR: usize = 0x08;
const CTRLDSSEGMENT: usize = 0x10;
const PERIODICLISTBASE: usize = 0x14;
const ASYNCLISTADDR: usize = 0x18;
const CONFIGFLAG: usize = 0x40;
const PORTSC: usize = 0x44;

const CMD_RUN: u32 = 1 << 0;
const CMD_RESET: u32 = 1 << 1;
const CMD_PERIODIC: u32 = 1 << 4;
const CMD_ASYNC: u32 = 1 << 5;
const CMD_IAAD: u32 = 1 << 6;
/// Interrupt threshold: 8 microframes (1 ms), the default.
const CMD_ITC_8: u32 = 8 << 16;

const STS_USBINT: u32 = 1 << 0;
const STS_ERRINT: u32 = 1 << 1;
const STS_PCD: u32 = 1 << 2;
const STS_IAA: u32 = 1 << 5;
const STS_HALTED: u32 = 1 << 12;
const STS_ACK: u32 = 0x3F;

const PORT_CONNECT: u32 = 1 << 0;
const PORT_CONNECT_CHANGE: u32 = 1 << 1;
const PORT_ENABLE: u32 = 1 << 2;
const PORT_ENABLE_CHANGE: u32 = 1 << 3;
const PORT_OVER_CURRENT_CHANGE: u32 = 1 << 5;
const PORT_RESET: u32 = 1 << 8;
/// Line status (bits 11:10): K, a low-speed device.
const PORT_LINE_MASK: u32 = 3 << 10;
const PORT_LINE_K: u32 = 1 << 10;
const PORT_POWER: u32 = 1 << 12;
const PORT_OWNER: u32 = 1 << 13;
/// The write-1-to-clear bits, kept 0 in a write that must not clear them.
const PORT_RWC: u32 = PORT_CONNECT_CHANGE | PORT_ENABLE_CHANGE | PORT_OVER_CURRENT_CHANGE;

// Link pointers.
const LINK_TERMINATE: u32 = 1;
const LINK_QH: u32 = 1 << 1;

// qTD token.
pub const TOKEN_ACTIVE: u32 = 1 << 7;
const TOKEN_HALTED: u32 = 1 << 6;
const TOKEN_BUFFER_ERR: u32 = 1 << 5;
const TOKEN_BABBLE: u32 = 1 << 4;
const TOKEN_XACT_ERR: u32 = 1 << 3;
const TOKEN_IOC: u32 = 1 << 15;
const TOKEN_CERR_3: u32 = 3 << 10;
pub const TOKEN_DT: u32 = 1 << 31;
pub const PID_OUT: u32 = 0;
pub const PID_IN: u32 = 1;
pub const PID_SETUP: u32 = 2;

// QH words (32-bit index).
const QH_LINK: usize = 0;
const QH_CHAR: usize = 1;
const QH_CAPS: usize = 2;
const QH_NEXT: usize = 4;
const QH_ALT: usize = 5;
const QH_TOKEN: usize = 6;

/// A transfer's endpoint, as its QH says it.
#[derive(Clone, Copy)]
pub struct Target {
    pub address: u8,
    pub endpoint: u8,
    pub speed: u8,
    pub max_packet: u16,
    /// Control: the toggle is in the qTDs, and below high speed the QH's C
    /// bit is set.
    pub control: bool,
    /// Below high speed: the hub whose transaction translator the
    /// transfers go through, and its port.
    pub tt: Option<(u8, u8)>,
    /// An interrupt endpoint (periodic schedule): its QH's microframe masks.
    pub periodic: bool,
}

/// An endpoint's QH and its qTDs (one page), and a data page for a
/// control or queued endpoint's own transfers.
#[derive(Clone, Copy)]
pub struct Pipe {
    pub bus: u64,
    pub va: *mut u8,
    pub data_bus: u64,
    pub data: *mut u8,
    pub periodic: bool,
}

// SAFETY: DMA pages, touched by whoever holds the device's record.
unsafe impl Send for Pipe {}

impl Pipe {
    fn qh(&self) -> *mut u32 {
        self.va.cast()
    }

    fn qh_word(&self, i: usize) -> u32 {
        unsafe { core::ptr::read_volatile(self.qh().add(i)) }
    }

    fn set_qh_word(&self, i: usize, v: u32) {
        unsafe { core::ptr::write_volatile(self.qh().add(i), v) }
    }

    pub fn qtd_bus(&self, i: usize) -> u32 {
        (self.bus + (QTD_AT + i * QTD_STRIDE) as u64) as u32
    }

    fn qtd(&self, i: usize) -> *mut u32 {
        unsafe { self.va.add(QTD_AT + i * QTD_STRIDE).cast() }
    }

    pub fn token(&self, i: usize) -> u32 {
        unsafe { core::ptr::read_volatile(self.qtd(i).add(2)) }
    }

    /// qTD `i`: `bytes` at bus address `buf` (within 5 pages), `pid`,
    /// the toggle `dt` (control), interrupting when `ioc`; then `next`, a
    /// short packet going on at `alt`.
    #[allow(clippy::too_many_arguments)]
    pub fn set_qtd(&self, i: usize, next: Option<usize>, alt: Option<usize>, pid: u32, dt: bool, buf: u64, bytes: usize, ioc: bool) {
        let q = self.qtd(i);
        let link = |n: Option<usize>| n.map_or(LINK_TERMINATE, |n| self.qtd_bus(n));
        let token = TOKEN_ACTIVE
            | TOKEN_CERR_3
            | (pid << 8)
            | ((bytes as u32 & 0x7FFF) << 16)
            | if ioc { TOKEN_IOC } else { 0 }
            | if dt { TOKEN_DT } else { 0 };
        unsafe {
            core::ptr::write_volatile(q, link(next));
            core::ptr::write_volatile(q.add(1), link(alt));
            for k in 0..5 {
                let page = if k == 0 { buf } else { (buf & !0xFFF) + (k as u64) * PAGE as u64 };
                core::ptr::write_volatile(q.add(3 + k), page as u32);
                core::ptr::write_volatile(q.add(8 + k), 0);
            }
            core::ptr::write_volatile(q.add(2), token);
        }
    }
}

/// Up to [`MAX_LINKED`] QHs in the schedules.
pub const MAX_LINKED: usize = 128;

pub struct Linked {
    qhs: [(u32, usize); MAX_LINKED],
    n: usize,
}

impl Linked {
    fn va(&self, bus: u32) -> Option<*mut u32> {
        self.qhs[..self.n].iter().find(|q| q.0 == bus).map(|q| q.1 as *mut u32)
    }
}

/// The controller's bounce buffer for bulk transfers.
pub struct Bounce {
    pub bus: u64,
    pub va: *mut u8,
}

unsafe impl Send for Bounce {}

pub struct Controller {
    /// Operational registers (kernel virtual).
    op: usize,
    pub ports: u8,
    /// The controller has companion controllers: full- and low-speed
    /// devices are handed to them (the port owner bit).
    pub companions: bool,
    pub dma: i32,
    /// The async schedule's head QH, then the periodic anchor QH.
    heads: Pipe,
    pub bounce: SleepLock<Bounce>,
    /// The QHs linked into the schedules, `(bus address, va)`: an unlink
    /// walks from one to the next by the bus address the links hold.
    schedule: Lock<Linked>,
    /// The doorbell's answer (`STS_IAA`), from the interrupt or a poll.
    iaa: AtomicBool,
    pub irq_on: AtomicBool,
    pub core_index: AtomicU32,
    pub irqs: AtomicU32,
    pub events: AtomicU32,
    pub index: u8,
}

// SAFETY: the registers and the DMA pages are reached through volatile
// accesses; the locks order the rest.
unsafe impl Sync for Controller {}
unsafe impl Send for Controller {}

impl Controller {
    fn read(&self, reg: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.op + reg) as *const u32) }
    }

    fn write(&self, reg: usize, v: u32) {
        unsafe { core::ptr::write_volatile((self.op + reg) as *mut u32, v) }
    }

    fn portsc(&self, port: u8) -> u32 {
        self.read(PORTSC + 4 * usize::from(port))
    }

    fn set_portsc(&self, port: u8, v: u32) {
        self.write(PORTSC + 4 * usize::from(port), v)
    }

    /// What the waiters of this controller block on.
    pub fn key(&self) -> usize {
        self as *const Self as usize
    }
}

/// Writes to memory reach the controller before it is told to look.
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

/// Pages given back (`(dma handle, bus, va)`): devices come and go, and
/// the kernel's DMA pages are never returned.
static FREE: Lock<([(i32, u64, usize); POOL], usize)> = Lock::new(([(-1, 0, 0); POOL], 0));

/// `n` zeroed contiguous pages below 4 GiB.
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

/// Take the controller from the firmware (the USBLEGSUP capability in its
/// PCI configuration space, at HCCPARAMS's EECP), reset it and start both
/// schedules, every port routed to it (CONFIGFLAG) and powered.
pub fn init(mmio: usize, pci: (u8, u8, u8), dma: i32, index: u8) -> Result<Controller, &'static str> {
    let cap = |reg: usize| unsafe { core::ptr::read_volatile((mmio + reg) as *const u32) };
    let caplength = (cap(CAPLENGTH) & 0xFF) as usize;
    let hcs = cap(HCSPARAMS);
    let hcc = cap(HCCPARAMS);
    let ports = (hcs & 0xF) as u8;
    let companions = (hcs >> 12) & 0xF != 0;
    let api = api();
    let (b, s, f) = pci;
    // The firmware's legacy USB support lets go (EHCI 1.0, 5.1).
    let eecp = ((hcc >> 8) & 0xFF) as u8;
    if eecp >= 0x40 {
        let legsup = api.pci_cfg_read32(b, s, f, eecp);
        if legsup & 0xFF == 1 {
            api.pci_cfg_write32(b, s, f, eecp, legsup | 1 << 24);
            for _ in 0..100 {
                if api.pci_cfg_read32(b, s, f, eecp) & (1 << 16) == 0 {
                    break;
                }
                sleep_ms(10);
            }
            // No more SMIs from it.
            api.pci_cfg_write32(b, s, f, eecp + 4, 0);
        }
    }
    let (heads_bus, heads_va) = pages(dma, 1).ok_or("no DMA page")?;
    let (frames_bus, frames_va) = pages(dma, 1).ok_or("no DMA page")?;
    let (bounce_bus, bounce_va) = pages(dma, BOUNCE_PAGES).ok_or("no DMA pages")?;
    let c = Controller {
        op: mmio + caplength,
        ports: ports.min(MAX_PORTS as u8),
        companions,
        dma,
        heads: Pipe { bus: heads_bus, va: heads_va, data_bus: 0, data: core::ptr::null_mut(), periodic: false },
        bounce: SleepLock::new(&API, Bounce { bus: bounce_bus, va: bounce_va }),
        schedule: Lock::new(Linked { qhs: [(0, 0); MAX_LINKED], n: 0 }),
        iaa: AtomicBool::new(false),
        irq_on: AtomicBool::new(false),
        core_index: AtomicU32::new(u32::MAX),
        irqs: AtomicU32::new(0),
        events: AtomicU32::new(0),
        index,
    };
    // Stopped, then reset.
    c.write(USBCMD, c.read(USBCMD) & !CMD_RUN);
    for _ in 0..20 {
        if c.read(USBSTS) & STS_HALTED != 0 {
            break;
        }
        sleep_ms(1);
    }
    c.write(USBCMD, CMD_RESET);
    let mut reset = false;
    for _ in 0..250 {
        if c.read(USBCMD) & CMD_RESET == 0 {
            reset = true;
            break;
        }
        sleep_ms(1);
    }
    if !reset {
        return Err("reset timed out");
    }
    // The async head: a QH of its own ring, the reclamation head (H), never
    // active. The periodic anchor (at 512 in the same page): every frame's
    // entry, the interrupt QHs after it.
    let head = heads_va.cast::<u32>();
    let anchor_bus = heads_bus as u32 + 512;
    unsafe {
        core::ptr::write_volatile(head.add(QH_LINK), heads_bus as u32 | LINK_QH);
        core::ptr::write_volatile(head.add(QH_CHAR), 1 << 15 | 2 << 12);
        core::ptr::write_volatile(head.add(QH_NEXT), LINK_TERMINATE);
        core::ptr::write_volatile(head.add(QH_ALT), LINK_TERMINATE);
        let anchor = heads_va.add(512).cast::<u32>();
        core::ptr::write_volatile(anchor.add(QH_LINK), LINK_TERMINATE);
        core::ptr::write_volatile(anchor.add(QH_CHAR), 2 << 12);
        core::ptr::write_volatile(anchor.add(QH_NEXT), LINK_TERMINATE);
        core::ptr::write_volatile(anchor.add(QH_ALT), LINK_TERMINATE);
        let frames = frames_va.cast::<u32>();
        for i in 0..1024 {
            core::ptr::write_volatile(frames.add(i), anchor_bus | LINK_QH);
        }
    }
    sync_to(&c, heads_va, PAGE);
    sync_to(&c, frames_va, PAGE);
    if hcc & 1 != 0 {
        c.write(CTRLDSSEGMENT, 0);
    }
    c.write(USBINTR, 0);
    c.write(USBSTS, STS_ACK);
    c.write(PERIODICLISTBASE, frames_bus as u32);
    c.write(ASYNCLISTADDR, heads_bus as u32);
    c.write(USBCMD, CMD_ITC_8 | CMD_ASYNC | CMD_PERIODIC | CMD_RUN);
    // Every port routed here; a full- or low-speed device found on one is
    // handed back to the companion (`port_status`, `port_reset`).
    c.write(CONFIGFLAG, 1);
    if hcs & (1 << 4) != 0 {
        for p in 0..c.ports {
            let v = c.portsc(p) & !PORT_RWC;
            c.set_portsc(p, v | PORT_POWER);
        }
    }
    sleep_ms(20);
    Ok(c)
}

/// Interrupts on, once the handler is in place.
pub fn enable_interrupts(c: &Controller) {
    c.write(USBINTR, STS_USBINT | STS_ERRINT | STS_PCD | STS_IAA);
}

/// The controller's status, acknowledged: transfers done wake their
/// waiters and the core, port changes go to the core (`crate::core_*`).
/// From the interrupt handler, or the core's thread without one.
pub fn process(c: &Controller) {
    let sts = c.read(USBSTS) & STS_ACK;
    if sts == 0 {
        return;
    }
    c.write(USBSTS, sts);
    c.events.fetch_add(1, Ordering::Relaxed);
    if sts & STS_IAA != 0 {
        c.iaa.store(true, Ordering::Release);
    }
    if sts & (STS_USBINT | STS_ERRINT | STS_IAA) != 0 {
        api().wake(c.key());
        crate::core_wake(c);
    }
    if sts & STS_PCD != 0 {
        for p in 0..c.ports {
            if c.portsc(p) & (PORT_CONNECT_CHANGE | PORT_ENABLE_CHANGE) != 0 {
                crate::core_port_change(c, p);
            }
        }
    }
}

// ---- Ports ------------------------------------------------------------------

pub struct PortState {
    pub connected: bool,
    pub enabled: bool,
    pub speed: u8,
}

/// Hand port `port`'s device to the companion controller.
fn give_away(c: &Controller, port: u8) {
    let v = c.portsc(port) & !PORT_RWC;
    c.set_portsc(port, v | PORT_OWNER);
}

/// Root port `port`, its change bits acknowledged. A low-speed device (the
/// line in K state) goes to the companion at once and is not reported;
/// a port the companion owns has nothing for the core either.
pub fn port_status(c: &Controller, port: u8) -> PortState {
    // Written back as read: the change bits set are cleared, the rest kept
    // (a 0 written to the enable bit would disable the port).
    let v = c.portsc(port);
    c.set_portsc(port, v);
    let mut connected = v & PORT_CONNECT != 0 && v & PORT_OWNER == 0;
    if connected && v & PORT_ENABLE == 0 && v & PORT_LINE_MASK == PORT_LINE_K && c.companions {
        give_away(c, port);
        connected = false;
    }
    PortState { connected, enabled: connected && v & PORT_ENABLE != 0, speed: USB_SPEED_HIGH }
}

/// Reset root port `port` (50 ms, EHCI 4.2.2): enabled after it only with
/// a high-speed device; a full-speed one goes to the companion and the
/// reset fails here.
pub fn port_reset(c: &Controller, port: u8) -> bool {
    let v = c.portsc(port) & !PORT_RWC;
    c.set_portsc(port, (v & !PORT_ENABLE) | PORT_RESET);
    sleep_ms(50);
    let v = c.portsc(port) & !PORT_RWC;
    c.set_portsc(port, v & !PORT_RESET);
    for _ in 0..20 {
        if c.portsc(port) & PORT_RESET == 0 {
            break;
        }
        sleep_ms(1);
    }
    sleep_ms(2);
    let v = c.portsc(port);
    if v & PORT_ENABLE != 0 {
        return true;
    }
    if v & PORT_CONNECT != 0 && c.companions {
        give_away(c, port);
    }
    false
}

// ---- Pipes ------------------------------------------------------------------

/// A new endpoint's QH (its page, a data page when `data`), set up for
/// `t`, idle, linked into its schedule.
pub fn pipe_new(c: &Controller, t: &Target, data: bool) -> Option<Pipe> {
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
    let p = Pipe { bus, va, data_bus, data: data_va, periodic: t.periodic };
    p.set_qh_word(QH_CHAR, characteristics(t));
    p.set_qh_word(QH_CAPS, capabilities(t));
    p.set_qh_word(QH_NEXT, LINK_TERMINATE);
    p.set_qh_word(QH_ALT, LINK_TERMINATE);
    p.set_qh_word(QH_TOKEN, 0);
    link(c, &p);
    Some(p)
}

/// QH word 1: address, endpoint, speed, toggle control, max packet; below
/// high speed a control endpoint's C bit; NAK reload 0 on the periodic
/// schedule (EHCI 3.6), 4 elsewhere.
fn characteristics(t: &Target) -> u32 {
    let eps = match t.speed {
        USB_SPEED_LOW => 1,
        USB_SPEED_FULL => 0,
        _ => 2,
    };
    let rl = if t.periodic { 0 } else { 4 };
    u32::from(t.address & 0x7F)
        | u32::from(t.endpoint & 0xF) << 8
        | eps << 12
        | u32::from(t.control) << 14
        | u32::from(t.max_packet & 0x7FF) << 16
        | u32::from(t.control && t.speed != USB_SPEED_HIGH) << 27
        | rl << 28
}

/// QH word 2: one transaction a microframe; an interrupt endpoint is
/// visited in microframe 0 (its complete-splits in 2 to 4 behind a
/// translator); the translator's hub and port.
fn capabilities(t: &Target) -> u32 {
    let (smask, cmask) = match (t.periodic, t.tt.is_some()) {
        (false, _) => (0, 0),
        (true, false) => (0x01, 0),
        (true, true) => (0x01, 0x1C),
    };
    let (hub, port) = t.tt.unwrap_or((0, 0));
    smask | cmask << 8 | u32::from(hub & 0x7F) << 16 | u32::from(port & 0x7F) << 23 | 1 << 30
}

/// The device got its address (or its endpoint 0 its max packet size):
/// the idle QH says so.
pub fn pipe_retarget(p: &Pipe, t: &Target) {
    p.set_qh_word(QH_CHAR, characteristics(t));
    p.set_qh_word(QH_CAPS, capabilities(t));
}

/// Link `p` in: after the async head, or after the periodic anchor.
fn link(c: &Controller, p: &Pipe) {
    let mut linked = c.schedule.lock();
    if linked.n < MAX_LINKED {
        let n = linked.n;
        linked.qhs[n] = (p.bus as u32, p.va as usize);
        linked.n = n + 1;
    }
    let before = if p.periodic { unsafe { c.heads.va.add(512).cast::<u32>() } } else { c.heads.qh() };
    let next = unsafe { core::ptr::read_volatile(before.add(QH_LINK)) };
    p.set_qh_word(QH_LINK, next);
    sync_to(c, p.va, PAGE);
    wmb();
    unsafe { core::ptr::write_volatile(before.add(QH_LINK), p.bus as u32 | LINK_QH) };
    sync_to(c, c.heads.va, PAGE);
}

/// Unlink `p` and wait until the controller holds no copy of it: the
/// async advance doorbell (EHCI 4.8.2), or two frames on the periodic
/// schedule.
pub fn unlink(c: &Controller, p: &Pipe) {
    {
        let mut linked = c.schedule.lock();
        if let Some(i) = linked.qhs[..linked.n].iter().position(|q| q.0 == p.bus as u32) {
            let n = linked.n;
            linked.qhs[i] = linked.qhs[n - 1];
            linked.n = n - 1;
        }
        let first = if p.periodic { unsafe { c.heads.va.add(512).cast::<u32>() } } else { c.heads.qh() };
        let target = p.bus as u32 | LINK_QH;
        let mut at = first;
        // Walk the chain: a QH's link names the next by bus address.
        for _ in 0..=MAX_LINKED {
            let next = unsafe { core::ptr::read_volatile(at.add(QH_LINK)) };
            if next & LINK_TERMINATE != 0 || next == (c.heads.bus as u32 | LINK_QH) {
                break;
            }
            if next == target {
                unsafe { core::ptr::write_volatile(at.add(QH_LINK), p.qh_word(QH_LINK)) };
                sync_to(c, at.cast(), 64);
                break;
            }
            match linked.va(next & !0x1F) {
                Some(va) => at = va,
                None => break,
            }
        }
    }
    wmb();
    if p.periodic {
        sleep_ms(2);
        return;
    }
    c.iaa.store(false, Ordering::Release);
    c.write(USBCMD, c.read(USBCMD) | CMD_IAAD);
    for _ in 0..100 {
        if !c.irq_on.load(Ordering::Relaxed) {
            process(c);
        }
        if c.iaa.load(Ordering::Acquire) {
            return;
        }
        sleep_ms(1);
    }
}

/// Forget `p`: unlinked, its pages back to the pool.
pub fn pipe_free(c: &Controller, p: &Pipe) {
    unlink(c, p);
    free_page(c.dma, p.bus, p.va);
    if !p.data.is_null() {
        free_page(c.dma, p.data_bus, p.data);
    }
}

/// Put the qTDs written (from the first) on the idle QH `p`: the
/// controller starts on them at its next pass. The overlay keeps its
/// toggle (`keep_toggle`) or starts at DATA0.
pub fn start(c: &Controller, p: &Pipe, keep_toggle: bool) {
    sync_to(c, p.va, PAGE);
    wmb();
    let dt = if keep_toggle { p.qh_word(QH_TOKEN) & TOKEN_DT } else { 0 };
    p.set_qh_word(QH_ALT, LINK_TERMINATE);
    p.set_qh_word(QH_TOKEN, dt);
    wmb();
    p.set_qh_word(QH_NEXT, p.qtd_bus(0));
    sync_to(c, p.va, 128);
    wmb();
}

/// Back to DATA0 (`clear_halt`), the halt cleared; the QH must be idle.
pub fn reset_toggle(p: &Pipe) {
    p.set_qh_word(QH_NEXT, LINK_TERMINATE);
    p.set_qh_word(QH_TOKEN, 0);
    wmb();
}

/// Where the `n` qTDs on `p` stand: `None` while the last is active and
/// none halted, else the result: the bytes the qTDs `data` names moved
/// (each asked `asked[i]`), or the error.
pub fn outcome(c: &Controller, p: &Pipe, n: usize, data: &[usize], asked: &[usize]) -> Option<Result<usize, i32>> {
    sync_from(c, p.va, PAGE);
    for i in 0..n {
        let t = p.token(i);
        if t & TOKEN_HALTED != 0 {
            return Some(Err(if t & (TOKEN_BABBLE | TOKEN_BUFFER_ERR | TOKEN_XACT_ERR) != 0 { USB_EIO } else { USB_ESTALL }));
        }
    }
    // Short IN data ends a control transfer's data stage early and goes on
    // at its status qTD (the alternate next): the last qTD decides.
    if p.token(n - 1) & TOKEN_ACTIVE != 0 {
        return None;
    }
    let mut moved = 0;
    for (k, &i) in data.iter().enumerate() {
        let t = p.token(i);
        if t & TOKEN_ACTIVE != 0 {
            break;
        }
        let left = ((t >> 16) & 0x7FFF) as usize;
        moved += asked[k].saturating_sub(left);
    }
    Some(Ok(moved))
}

/// Wait for the `n` qTDs on `p` until `deadline` (ns): their outcome, or
/// [`USB_ETIMEDOUT`] with the pipe taken off and back so nothing of it
/// runs on; [`USB_EGONE`] when `gone` is raised meanwhile.
pub fn wait(c: &Controller, p: &Pipe, n: usize, data: &[usize], asked: &[usize], deadline: u64, gone: &AtomicBool) -> Result<usize, i32> {
    loop {
        let seq = api().wait_seq();
        if !c.irq_on.load(Ordering::Relaxed) {
            process(c);
        }
        if let Some(r) = outcome(c, p, n, data, asked) {
            return r;
        }
        let now = api().monotonic_ns();
        if gone.load(Ordering::Acquire) || now >= deadline {
            cancel(c, p);
            return Err(if gone.load(Ordering::Acquire) { USB_EGONE } else { USB_ETIMEDOUT });
        }
        let step = if c.irq_on.load(Ordering::Relaxed) { 10_000_000 } else { 1_000_000 };
        api().block_until(c.key(), seq, (now + step).min(deadline));
    }
}

/// Stop what is on `p`: out of the schedule (so the controller drops its
/// copy), its qTDs and overlay idle, back in.
pub fn cancel(c: &Controller, p: &Pipe) {
    unlink(c, p);
    for i in 0..MAX_QTDS {
        unsafe { core::ptr::write_volatile(p.qtd(i).add(2), 0) };
    }
    let dt = p.qh_word(QH_TOKEN) & TOKEN_DT;
    p.set_qh_word(QH_NEXT, LINK_TERMINATE);
    p.set_qh_word(QH_ALT, LINK_TERMINATE);
    p.set_qh_word(QH_TOKEN, dt);
    link(c, p);
}

/// Whether a stall or error left `p` halted (`clear_halt` undoes it).
pub fn halted(p: &Pipe) -> bool {
    p.qh_word(QH_TOKEN) & TOKEN_HALTED != 0
}

/// The QH's toggle cleared of a halt, the queue empty: after an error the
/// next transfer starts clean.
pub fn unhalt(p: &Pipe) {
    let dt = p.qh_word(QH_TOKEN) & TOKEN_DT;
    p.set_qh_word(QH_NEXT, LINK_TERMINATE);
    p.set_qh_word(QH_TOKEN, dt);
    wmb();
}
