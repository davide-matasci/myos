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
//! An isochronous OUT endpoint's packets go in the frame list itself, a
//! few frames ahead of the controller, in front of the anchor QH: in
//! isochronous transfer descriptors (iTD) for a high-speed endpoint, up to
//! eight packets each (one per microframe of its interval), and in split
//! ones (siTD) for a full-speed endpoint behind a high-speed hub's
//! transaction translator, one packet each. They come from the
//! controller's pool of descriptor slots and are taken out of the frame
//! list once done or once their frame has passed (`isoch_reap`).
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
const FRINDEX: usize = 0x0C;
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
    /// The periodic frame list (1024 entries).
    frames: *mut u32,
    /// Frames ahead of the controller an isochronous descriptor goes in at
    /// the earliest (HCCPARAMS's isochronous scheduling threshold, and one
    /// more).
    iso_ahead: u16,
    /// The isochronous descriptors' slots, made with the first endpoint.
    iso_pool: SleepLock<Option<IsoPool>>,
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
        frames: frames_va.cast(),
        // IST (bits 7:4): with bit 7, the controller may hold a whole frame
        // (bits 6:4 of them) ahead; else up to 7 microframes.
        iso_ahead: if hcc & (1 << 7) != 0 { ((hcc >> 4) & 7) as u16 + 2 } else { 2 },
        iso_pool: SleepLock::new(&API, None),
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


// ---- Isochronous ------------------------------------------------------------

/// The pool's descriptor slots: 128 bytes each (an iTD is 92 with the
/// 64-bit words, an siTD 36), 32-byte aligned as links need; a link's bus
/// address finds its slot by the offset.
const ISO_SLOT: usize = 128;
const ISO_POOL_PAGES: usize = 8;
const ISO_SLOTS: usize = ISO_POOL_PAGES * PAGE / ISO_SLOT;
/// Isochronous packets on an endpoint at most (`USB_ISOCH_MAX`).
pub const ISO_PACKETS: usize = 64;
/// Descriptors of an endpoint's ring at most: a packet each, and the one
/// being filled.
const ISO_RING: usize = ISO_PACKETS + 2;
/// Frames between an endpoint's descriptors at most (a longer interval is
/// served this often), and how far ahead of the controller the last one
/// may be: within half the frame list, so a frame behind it is a frame
/// passed (`iso_reap`).
const ISO_FRAME_STEP_MAX: usize = 4;
const ISO_AHEAD_MAX: u16 = 400;

// Link pointer types (bits 2:1), and their mask.
const LINK_TYPE_MASK: u32 = 3 << 1;
const LINK_TYPE_ITD: u32 = 0;
const LINK_TYPE_SITD: u32 = 2 << 1;
// iTD transaction words, siTD transfer state.
const ITD_ACTIVE: u32 = 1 << 31;
const ITD_IOC: u32 = 1 << 15;
const SITD_ACTIVE: u32 = 1 << 7;
const SITD_IOC: u32 = 1 << 31;
/// A full-speed OUT start-split carries 188 bytes at most.
const SPLIT_BYTES: usize = 188;

/// The controller's isochronous descriptor slots (one contiguous block).
pub struct IsoPool {
    bus: u64,
    va: *mut u8,
    used: [u64; ISO_SLOTS / 64],
}

unsafe impl Send for IsoPool {}

impl IsoPool {
    fn slot_va(&self, slot: u16) -> *mut u32 {
        unsafe { self.va.add(usize::from(slot) * ISO_SLOT).cast() }
    }

    fn slot_bus(&self, slot: u16) -> u32 {
        (self.bus + (usize::from(slot) * ISO_SLOT) as u64) as u32
    }

    /// The descriptor a link names, if it is one of the pool's.
    fn va_of(&self, link: u32) -> Option<*mut u32> {
        let at = u64::from(link & !0x1F).checked_sub(self.bus)? as usize;
        (at < ISO_SLOTS * ISO_SLOT).then(|| unsafe { self.va.add(at).cast() })
    }
}

/// An isochronous OUT endpoint, as its descriptors name it: a high-speed
/// one (iTDs) with its transactions per microframe, or a full-speed one
/// behind the translator of hub `tt` (siTDs); its service interval
/// (`bInterval`, an exponent).
#[derive(Clone, Copy)]
pub struct IsoTarget {
    pub address: u8,
    pub endpoint: u8,
    /// `wMaxPacketSize` as the descriptor has it (high speed: the
    /// additional transactions in bits 12:11).
    pub max_packet: u16,
    pub interval: u8,
    pub tt: Option<(u8, u8)>,
}

/// An isochronous OUT endpoint's ring of descriptors (pool slots), each
/// with a data region of its own; the frame each is in.
pub struct Iso {
    t: IsoTarget,
    /// Packets a descriptor carries, the microframes between them (an iTD),
    /// the frames between descriptors.
    per_desc: usize,
    uframe_step: usize,
    frame_step: u16,
    /// A packet's room, and the largest one.
    slot: usize,
    packet_max: usize,
    ring: usize,
    slots: [u16; ISO_RING],
    data_bus: u64,
    data: *mut u8,
    data_pages: usize,
    /// Per descriptor: its frame (`u16::MAX`: free), and its packets.
    frame: [u16; ISO_RING],
    packets: [u8; ISO_RING],
    next: usize,
    /// The frame (list index) the next descriptor goes in.
    tail: u16,
    pub in_flight: usize,
    /// Completed, not reaped yet.
    pub done: u32,
}

unsafe impl Send for Iso {}

impl Iso {
    fn split(&self) -> bool {
        self.t.tt.is_some()
    }

    fn region(&self) -> usize {
        self.per_desc * self.slot
    }
}

/// The frame (list index) the controller is in.
fn frame_now(c: &Controller) -> u16 {
    ((c.read(FRINDEX) >> 3) & 0x3FF) as u16
}

/// An isochronous OUT endpoint `t`'s ring: its descriptors' slots and
/// data. The pool is made with the first.
pub fn iso_new(c: &Controller, t: &IsoTarget) -> Option<Iso> {
    let mps = usize::from(t.max_packet & 0x7FF).max(1);
    let interval = 1usize << (t.interval.clamp(1, 16) - 1);
    let (per_desc, uframe_step, frame_step, packet_max) = if t.tt.is_some() {
        // Full speed: a packet a frame of its interval, 1023 bytes at most.
        (1, 8, interval.min(ISO_FRAME_STEP_MAX) as u16, mps.min(1023))
    } else {
        // High speed: its interval in microframes; up to three
        // transactions a microframe.
        let mult = usize::from((t.max_packet >> 11) & 3) + 1;
        let packet_max = (mps * mult).min(myos_abi::USB_ISOCH_PACKET_MAX);
        if interval <= 8 {
            (8 / interval, interval, 1, packet_max)
        } else {
            (1, 8, (interval / 8).min(ISO_FRAME_STEP_MAX) as u16, packet_max)
        }
    };
    let slot = packet_max.max(16).next_power_of_two();
    let ring = (ISO_PACKETS.div_ceil(per_desc) + 2).min(ISO_RING);
    let data_pages = (ring * per_desc * slot).div_ceil(PAGE);
    let mut slots = [0u16; ISO_RING];
    {
        let mut pool = c.iso_pool.lock();
        if pool.is_none() {
            let (bus, va) = pages(c.dma, ISO_POOL_PAGES)?;
            *pool = Some(IsoPool { bus, va, used: [0; ISO_SLOTS / 64] });
        }
        let pool = pool.as_mut()?;
        let free = (0..ISO_SLOTS).filter(|&s| pool.used[s / 64] & (1 << (s % 64)) == 0).count();
        if free < ring {
            return None;
        }
        let mut n = 0;
        for s in 0..ISO_SLOTS {
            if n < ring && pool.used[s / 64] & (1 << (s % 64)) == 0 {
                pool.used[s / 64] |= 1 << (s % 64);
                slots[n] = s as u16;
                n += 1;
            }
        }
    }
    let Some((data_bus, data)) = pages(c.dma, data_pages) else {
        if let Some(pool) = c.iso_pool.lock().as_mut() {
            for &s in &slots[..ring] {
                pool.used[usize::from(s) / 64] &= !(1 << (s % 64));
            }
        }
        return None;
    };
    Some(Iso {
        t: *t,
        per_desc,
        uframe_step,
        frame_step,
        slot,
        packet_max,
        ring,
        slots,
        data_bus,
        data,
        data_pages,
        frame: [u16::MAX; ISO_RING],
        packets: [0; ISO_RING],
        next: 0,
        tail: 0,
        in_flight: 0,
        done: 0,
    })
}

/// Queue `lens` packets from `data`, one a service interval after the last
/// one queued (from [`Controller::iso_ahead`] frames ahead of the
/// controller when it has none): an iTD per frame for up to eight, an siTD
/// per packet. `USB_EBUSY` when the ring has no room.
pub fn iso_queue(c: &Controller, iso: &mut Iso, data: &[u8], lens: &[u16]) -> i32 {
    iso_reap(c, iso, false);
    let descs = lens.len().div_ceil(iso.per_desc);
    let busy = iso.frame[..iso.ring].iter().filter(|&&f| f != u16::MAX).count();
    if iso.in_flight + lens.len() > ISO_PACKETS || busy + descs > iso.ring {
        return myos_abi::USB_EBUSY;
    }
    let now = frame_now(c);
    let ahead = iso.tail.wrapping_sub(now) & 0x3FF;
    if iso.in_flight == 0 || ahead < c.iso_ahead || ahead > 512 {
        iso.tail = (now + c.iso_ahead) & 0x3FF;
    }
    if (iso.tail.wrapping_sub(now) & 0x3FF) + descs as u16 * iso.frame_step > ISO_AHEAD_MAX {
        return myos_abi::USB_EBUSY;
    }
    let pool = c.iso_pool.lock();
    let Some(pool) = pool.as_ref() else {
        return USB_EIO;
    };
    let mut at = 0usize;
    for (n, chunk) in lens.chunks(iso.per_desc).enumerate() {
        let last = n + 1 == descs;
        let i = iso.next;
        iso.next = (iso.next + 1) % iso.ring;
        let region_bus = iso.data_bus + (i * iso.region()) as u64;
        let region = unsafe { iso.data.add(i * iso.region()) };
        let mut lens_k = [0usize; 8];
        for (k, &len) in chunk.iter().enumerate() {
            let len = usize::from(len).min(iso.packet_max);
            unsafe { core::ptr::copy_nonoverlapping(data[at..].as_ptr(), region.add(k * iso.slot), len) };
            at += usize::from(chunk[k]);
            lens_k[k] = len;
        }
        sync_to(c, region, iso.region());
        let slot = iso.slots[i];
        let d = pool.slot_va(slot);
        let w = |k: usize, v: u32| unsafe { core::ptr::write_volatile(d.add(k), v) };
        for k in 0..ISO_SLOT / 4 {
            w(k, 0);
        }
        let page0 = (region_bus & !0xFFF) as u32;
        let link_type = if iso.split() {
            let (hub, port) = iso.t.tt.unwrap_or((0, 0));
            let len = lens_k[0];
            // The start-splits: one a microframe from microframe 0, 188 bytes
            // each; no complete-split for an OUT.
            let starts = len.div_ceil(SPLIT_BYTES).max(1) as u32;
            w(1, u32::from(iso.t.address & 0x7F) | u32::from(iso.t.endpoint & 0xF) << 8 | u32::from(hub & 0x7F) << 16 | u32::from(port & 0x7F) << 24);
            w(2, (1 << starts) - 1);
            w(3, SITD_ACTIVE | (len as u32 & 0x3FF) << 16 | if last { SITD_IOC } else { 0 });
            w(4, region_bus as u32);
            // Page 1, the transaction position (all, or the first of
            // several) and the start-split count.
            w(5, (page0 + PAGE as u32) | if starts > 1 { 1 << 3 } else { 0 } | starts);
            w(6, LINK_TERMINATE);
            LINK_TYPE_SITD
        } else {
            for (k, &len) in lens_k[..chunk.len()].iter().enumerate() {
                let off = (region_bus & 0xFFF) as usize + k * iso.slot;
                let ioc = last && k + 1 == chunk.len();
                w(1 + k * iso.uframe_step, ITD_ACTIVE | (len as u32 & 0xFFF) << 16 | if ioc { ITD_IOC } else { 0 } | ((off >> 12) as u32) << 12 | (off & 0xFFF) as u32);
            }
            let mult = u32::from((iso.t.max_packet >> 11) & 3) + 1;
            for p in 0..7u32 {
                let page = page0 + p * PAGE as u32;
                w(
                    9 + p as usize,
                    page | match p {
                        0 => u32::from(iso.t.endpoint & 0xF) << 8 | u32::from(iso.t.address & 0x7F),
                        1 => u32::from(iso.t.max_packet & 0x7FF),
                        2 => mult,
                        _ => 0,
                    },
                );
            }
            LINK_TYPE_ITD
        };
        // In front of the frame's entry (other descriptors, then the anchor).
        let frame = iso.tail;
        iso.tail = (iso.tail + iso.frame_step) & 0x3FF;
        let entry = unsafe { c.frames.add(usize::from(frame)) };
        w(0, unsafe { core::ptr::read_volatile(entry) });
        sync_to(c, d.cast(), ISO_SLOT);
        wmb();
        unsafe { core::ptr::write_volatile(entry, pool.slot_bus(slot) | link_type) };
        sync_to(c, entry.cast(), 4);
        iso.frame[i] = frame;
        iso.packets[i] = chunk.len() as u8;
        iso.in_flight += chunk.len();
    }
    0
}

/// The descriptors done (inactive) or whose frame has passed (missed:
/// gone too), every one when `all`: out of the frame list, their packets
/// added to `iso.done`.
pub fn iso_reap(c: &Controller, iso: &mut Iso, all: bool) {
    let now = frame_now(c);
    let pool = c.iso_pool.lock();
    let Some(pool) = pool.as_ref() else {
        return;
    };
    for i in 0..iso.ring {
        let frame = iso.frame[i];
        if frame == u16::MAX {
            continue;
        }
        let slot = iso.slots[i];
        let d = pool.slot_va(slot);
        sync_from(c, d.cast(), ISO_SLOT);
        let r = |k: usize| unsafe { core::ptr::read_volatile(d.add(k)) };
        let active = if iso.split() {
            r(3) & SITD_ACTIVE != 0
        } else {
            (0..8).any(|u| r(1 + u) & ITD_ACTIVE != 0)
        };
        let passed = now != frame && now.wrapping_sub(frame) & 0x3FF < 512;
        if active && !passed && !all {
            continue;
        }
        unlink_iso(c, pool, frame, pool.slot_bus(slot), r(0));
        iso.frame[i] = u16::MAX;
        iso.in_flight -= usize::from(iso.packets[i]);
        iso.done += u32::from(iso.packets[i]);
    }
}

/// Take descriptor `bus` (whose link is `next`) out of frame `frame`'s
/// chain: the entry, or the descriptor before it (another endpoint's).
fn unlink_iso(c: &Controller, pool: &IsoPool, frame: u16, bus: u32, next: u32) {
    let entry = unsafe { c.frames.add(usize::from(frame)) };
    let mut link = unsafe { core::ptr::read_volatile(entry) };
    if link & !0x1F == bus {
        unsafe { core::ptr::write_volatile(entry, next) };
        sync_to(c, entry.cast(), 4);
        return;
    }
    for _ in 0..ISO_SLOTS {
        if link & LINK_TERMINATE != 0 || !matches!(link & LINK_TYPE_MASK, LINK_TYPE_ITD | LINK_TYPE_SITD) {
            return;
        }
        let Some(prev) = pool.va_of(link) else {
            return;
        };
        link = unsafe { core::ptr::read_volatile(prev) };
        if link & !0x1F == bus {
            unsafe { core::ptr::write_volatile(prev, next) };
            sync_to(c, prev.cast(), 4);
            return;
        }
    }
}

/// The endpoint goes: what is still queued stops (inactive), out of the
/// frame list once the controller is past it; its slots and data free.
pub fn iso_free(c: &Controller, mut iso: Iso) {
    {
        let pool = c.iso_pool.lock();
        if let Some(pool) = pool.as_ref() {
            for i in 0..iso.ring {
                if iso.frame[i] == u16::MAX {
                    continue;
                }
                let d = pool.slot_va(iso.slots[i]);
                if iso.split() {
                    unsafe { core::ptr::write_volatile(d.add(3), 0) };
                } else {
                    for u in 0..8 {
                        unsafe { core::ptr::write_volatile(d.add(1 + u), 0) };
                    }
                }
                sync_to(c, d.cast(), ISO_SLOT);
            }
        }
    }
    // A frame for the controller to finish the one it is in.
    sleep_ms(2);
    iso_reap(c, &mut iso, true);
    sleep_ms(2);
    if let Some(pool) = c.iso_pool.lock().as_mut() {
        for &s in &iso.slots[..iso.ring] {
            pool.used[usize::from(s) / 64] &= !(1 << (s % 64));
        }
    }
    for k in 0..iso.data_pages {
        free_page(c.dma, iso.data_bus + (k * PAGE) as u64, unsafe { iso.data.add(k * PAGE) });
    }
}
