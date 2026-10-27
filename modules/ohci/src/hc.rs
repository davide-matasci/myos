//! The OHCI controller (Open Host Controller Interface 1.0a): its
//! memory-mapped registers, its schedule and the transfers on it.
//!
//! The controller walks lists of endpoint descriptors (ED), each with a
//! queue of transfer descriptors (TD) from its head pointer to its tail
//! pointer: control and bulk EDs in two lists of their own (`HcControlHeadED`,
//! `HcBulkHeadED`), interrupt and isochronous ones from the 32 entries of
//! the interrupt table in the HCCA, the block it shares with the module.
//! Every list starts at an anchor ED (skipped: `K`), so an endpoint's ED is
//! linked in after its list's anchor and out again by its predecessor.
//! Every table entry leads to the interrupt anchor: an interrupt endpoint
//! is asked every frame. The isochronous anchor follows the last
//! interrupt ED.
//!
//! An ED's queue ends in a dummy TD the controller does not run: a
//! transfer writes its TDs from the dummy on (a ring of them in the ED's
//! page), a new dummy after them, then moves the tail pointer to it. The
//! controller retires each TD it is done with, moving the head pointer
//! along: the queue is empty again when head and tail meet; an error halts
//! the ED (`H`) at the TD after the failing one. Bulk and interrupt data
//! toggles are the ED's (`toggleCarry`); a control transfer's TDs carry
//! theirs.
//!
//! An isochronous OUT endpoint's packets go in isochronous TDs, up to eight
//! frames each, queued on its ED a few frames ahead of the controller and
//! counted gone once the head pointer passed them (`isoch_reap`).
//!
//! Every structure the controller reads is below 4 GiB (`DmaDevice::below`).

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering, compiler_fence};

use myos_abi::{
    DMA_FROM_DEVICE, DMA_TO_DEVICE, Lock, SleepLock, USB_EBUSY, USB_EGONE, USB_EIO, USB_ESTALL, USB_ETIMEDOUT,
    USB_SPEED_FULL, USB_SPEED_LOW,
};

use crate::{API, api, sleep_ms};

pub const MAX_CTRL: usize = 8;
/// Root ports a controller has at most (`HcRhDescriptorA.NDP`).
pub const MAX_PORTS: u8 = 15;
const PAGE: usize = 4096;
/// A pipe's page: its ED, then a ring of TDs.
const TD_AT: usize = 32;
const TD_STRIDE: usize = 16;
pub const TD_RING: usize = 64;
/// A bulk transfer's piece: one TD over two pages at most.
pub const BOUNCE_PAGES: usize = 2;
/// Isochronous packets on an endpoint at most (`USB_ISOCH_MAX`).
pub const ISO_PACKETS: usize = 64;

const HC_REVISION: usize = 0x00;
const HC_CONTROL: usize = 0x04;
const HC_COMMAND_STATUS: usize = 0x08;
const HC_INTERRUPT_STATUS: usize = 0x0C;
const HC_INTERRUPT_ENABLE: usize = 0x10;
const HC_INTERRUPT_DISABLE: usize = 0x14;
const HC_HCCA: usize = 0x18;
const HC_CONTROL_HEAD_ED: usize = 0x20;
const HC_CONTROL_CURRENT_ED: usize = 0x24;
const HC_BULK_HEAD_ED: usize = 0x28;
const HC_BULK_CURRENT_ED: usize = 0x2C;
const HC_FM_INTERVAL: usize = 0x34;
const HC_FM_NUMBER: usize = 0x3C;
const HC_PERIODIC_START: usize = 0x40;
const HC_LS_THRESHOLD: usize = 0x44;
const HC_RH_DESCRIPTOR_A: usize = 0x48;
const HC_RH_STATUS: usize = 0x50;
const HC_RH_PORT_STATUS: usize = 0x54;

const CTL_CBSR_4: u32 = 3;
const CTL_PLE: u32 = 1 << 2;
const CTL_IE: u32 = 1 << 3;
const CTL_CLE: u32 = 1 << 4;
const CTL_BLE: u32 = 1 << 5;
const CTL_HCFS: u32 = 3 << 6;
const HCFS_RESET: u32 = 0;
const HCFS_RESUME: u32 = 1 << 6;
const HCFS_OPERATIONAL: u32 = 2 << 6;
const CTL_IR: u32 = 1 << 8;

const CMD_HCR: u32 = 1 << 0;
const CMD_CLF: u32 = 1 << 1;
const CMD_BLF: u32 = 1 << 2;
const CMD_OCR: u32 = 1 << 3;

const INTR_WDH: u32 = 1 << 1;
const INTR_UE: u32 = 1 << 4;
const INTR_RHSC: u32 = 1 << 6;
const INTR_MIE: u32 = 1 << 31;
/// Every status bit (the ownership change's too).
const INTR_ALL: u32 = 0x7F | 1 << 30;

/// The frame interval of a 12 MHz full-speed frame (11999 bit times) and
/// the largest packet that fits one (`FSMPS`).
const FM_INTERVAL: u32 = 0x2EDF;
const FSMPS: u32 = (6 * (FM_INTERVAL - 210)) / 7;
const FIT: u32 = 1 << 31;

const RHA_NDP: u32 = 0xFF;
const RHA_PSM: u32 = 1 << 8;
const RHA_NPS: u32 = 1 << 9;
const RHS_LPSC: u32 = 1 << 16;

const PORT_CCS: u32 = 1 << 0;
const PORT_PES: u32 = 1 << 1;
const PORT_PRS: u32 = 1 << 4;
const PORT_PPS: u32 = 1 << 8;
const PORT_LSDA: u32 = 1 << 9;
const PORT_CSC: u32 = 1 << 16;
const PORT_PRSC: u32 = 1 << 20;
/// The change bits (write 1 to clear).
const PORT_CHANGES: u32 = 0x1F << 16;

// ED: dword 0, the head pointer's low bits.
const ED_SKIP: u32 = 1 << 14;
const ED_LOW_SPEED: u32 = 1 << 13;
const ED_ISO: u32 = 1 << 15;
const ED_DIR_OUT: u32 = 1 << 11;
const ED_HALTED: u32 = 1;
const ED_CARRY: u32 = 2;

// General TD dword 0.
const TD_ROUNDING: u32 = 1 << 18;
const TD_NO_IRQ: u32 = 7 << 21;
const TD_TOGGLE_DATA0: u32 = 2 << 24;
const TD_TOGGLE_DATA1: u32 = 3 << 24;
const TD_CC_SHIFT: u32 = 28;
const CC_NO_ERROR: u32 = 0;
const CC_STALL: u32 = 4;
const CC_NOT_ACCESSED: u32 = 0xF;

pub const DP_SETUP: u32 = 0;
pub const DP_OUT: u32 = 1;
pub const DP_IN: u32 = 2;

/// A transfer's endpoint.
#[derive(Clone, Copy)]
pub struct Target {
    pub address: u8,
    pub endpoint: u8,
    pub speed: u8,
    pub max_packet: u16,
}

/// The list an ED is on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Control = 0,
    Bulk = 1,
    Interrupt = 2,
    Isochronous = 3,
}

/// An endpoint's ED and its TD ring (one page), and a data page for a
/// control or queued endpoint's transfers.
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

/// One TD of a transfer: its direction (`DP_*`), its toggle (`None`: the
/// ED's), its buffer. A short IN packet ends it without an error.
#[derive(Clone, Copy)]
pub struct Td {
    pub pid: u32,
    pub toggle: Option<bool>,
    pub buf: u64,
    pub len: usize,
}

/// The TDs a transfer put on a pipe: from ring slot `first`, `n` of them,
/// their lengths (for the bytes moved).
#[derive(Clone, Copy)]
pub struct Queued {
    pub first: usize,
    pub n: usize,
    pub lens: [u16; 3],
    pub bufs: [u32; 3],
}

impl Pipe {
    fn td_bus(&self, i: usize) -> u32 {
        (self.bus + (TD_AT + (i % TD_RING) * TD_STRIDE) as u64) as u32
    }

    fn td(&self, i: usize) -> *mut u8 {
        unsafe { self.va.add(TD_AT + (i % TD_RING) * TD_STRIDE) }
    }

    /// The ring slot of TD bus address `bus`.
    fn slot_of(&self, bus: u32) -> usize {
        (u64::from(bus & !0xF) - self.bus) as usize / TD_STRIDE - TD_AT / TD_STRIDE
    }

    fn head(&self) -> u32 {
        rd(self.va, 8)
    }

    fn tail(&self) -> u32 {
        rd(self.va, 4)
    }

    /// The ED's endpoint: function address, endpoint, speed, packet size;
    /// the direction from the TDs (control, bulk, interrupt) or OUT
    /// (isochronous). Its skip bit kept.
    pub fn set_target(&self, t: &Target) {
        let skip = rd(self.va, 0) & ED_SKIP;
        let mut v = u32::from(t.address & 0x7F) | u32::from(t.endpoint & 0xF) << 7 | u32::from(t.max_packet & 0x7FF) << 16 | skip;
        if t.speed == USB_SPEED_LOW {
            v |= ED_LOW_SPEED;
        }
        if self.kind == Kind::Isochronous {
            v |= ED_ISO | ED_DIR_OUT;
        }
        wr(self.va, 0, v);
    }
}

/// An isochronous OUT endpoint's packets: its ED (a pipe), a ring of
/// isochronous TDs (32 bytes each, in a page of their own) with a data
/// region each, used in turn; the frames each covers.
pub struct Iso {
    pub pipe: Pipe,
    pub tds_bus: u64,
    pub tds: *mut u8,
    pub data_bus: u64,
    pub data: *mut u8,
    pub data_pages: usize,
    /// A packet's room, and the packets of a TD (its region's).
    pub slot: usize,
    pub per_td: usize,
    pub ring: usize,
    /// The oldest TD queued, and the dummy (the tail).
    pub oldest: usize,
    pub tail_slot: usize,
    /// Per TD: its packets (0: free).
    pub packets: [u8; ISO_RING],
    /// The frame after the last one queued.
    pub frame: u16,
    pub in_flight: usize,
    /// Completed, not reaped yet.
    pub done: u32,
}

pub const ISO_RING: usize = 34;

unsafe impl Send for Iso {}

pub struct Bounce {
    pub bus: u64,
    pub va: *mut u8,
}

unsafe impl Send for Bounce {}

pub struct Controller {
    mmio: usize,
    /// The PCI function (a rescan skips it).
    pub pci: (u8, u8, u8),
    pub dma: i32,
    pub ports: u8,
    /// The HCCA (256 bytes) and the four anchor EDs, in one page.
    hcca_bus: u64,
    hcca: *mut u8,
    pub bounce: SleepLock<Bounce>,
    /// Schedule edits one at a time (an unlink waits for a frame); the EDs
    /// linked, `(bus, va)`, for walking the lists.
    schedule: SleepLock<Linked>,
    pub irq_on: AtomicBool,
    pub core_index: AtomicU32,
    pub irqs: AtomicU32,
    pub events: AtomicU32,
}

unsafe impl Sync for Controller {}
unsafe impl Send for Controller {}

pub const MAX_LINKED: usize = 128;

pub struct Linked {
    eds: [(u32, usize); MAX_LINKED],
    n: usize,
}

/// The anchors' place in the HCCA page: after the HCCA's 256 bytes, one
/// per list (`Kind`).
const ANCHOR_AT: usize = 256;

impl Controller {
    pub fn key(&self) -> usize {
        self as *const Self as usize
    }

    fn read(&self, reg: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.mmio + reg) as *const u32) }
    }

    fn write(&self, reg: usize, v: u32) {
        unsafe { core::ptr::write_volatile((self.mmio + reg) as *mut u32, v) }
    }

    fn anchor(&self, k: Kind) -> *mut u8 {
        unsafe { self.hcca.add(ANCHOR_AT + k as usize * 16) }
    }

    fn anchor_bus(&self, k: Kind) -> u32 {
        (self.hcca_bus + (ANCHOR_AT + k as usize * 16) as u64) as u32
    }

    pub fn frame_number(&self) -> u16 {
        self.read(HC_FM_NUMBER) as u16
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

/// Take the controller from the firmware (from SMM through an ownership
/// change request, or from a BIOS driver by a reset), reset it, and start
/// it on the anchors with its root ports powered.
pub fn init(mmio: usize, pci: (u8, u8, u8), dma: i32) -> Result<Controller, &'static str> {
    let read = |reg: usize| unsafe { core::ptr::read_volatile((mmio + reg) as *const u32) };
    let write = |reg: usize, v: u32| unsafe { core::ptr::write_volatile((mmio + reg) as *mut u32, v) };
    if read(HC_REVISION) & 0xFF != 0x10 {
        return Err("not OHCI 1.0");
    }
    if read(HC_CONTROL) & CTL_IR != 0 {
        // SMM owns it: ask, and wait for it to let go (the interrupt
        // routing bit clears).
        write(HC_INTERRUPT_ENABLE, 1 << 30);
        write(HC_COMMAND_STATUS, CMD_OCR);
        let mut released = false;
        for _ in 0..100 {
            if read(HC_CONTROL) & CTL_IR == 0 {
                released = true;
                break;
            }
            sleep_ms(10);
        }
        if !released {
            return Err("SMM kept the controller");
        }
    } else if read(HC_CONTROL) & CTL_HCFS != HCFS_RESET && read(HC_CONTROL) & CTL_HCFS != HCFS_OPERATIONAL {
        // A BIOS driver left it suspended: resume before the reset.
        write(HC_CONTROL, HCFS_RESUME);
        sleep_ms(20);
    }
    let (hcca_bus, hcca) = pages(dma, 1).ok_or("no DMA page")?;
    let (bounce_bus, bounce_va) = pages(dma, BOUNCE_PAGES).ok_or("no DMA pages")?;
    let ports = (read(HC_RH_DESCRIPTOR_A) & RHA_NDP).min(u32::from(MAX_PORTS)) as u8;
    let c = Controller {
        mmio,
        pci,
        dma,
        ports,
        hcca_bus,
        hcca,
        bounce: SleepLock::new(&API, Bounce { bus: bounce_bus, va: bounce_va }),
        schedule: SleepLock::new(&API, Linked { eds: [(0, 0); MAX_LINKED], n: 0 }),
        irq_on: AtomicBool::new(false),
        core_index: AtomicU32::new(u32::MAX),
        irqs: AtomicU32::new(0),
        events: AtomicU32::new(0),
    };
    // The anchors, skipped; the interrupt one leads to the isochronous one.
    for k in [Kind::Control, Kind::Bulk, Kind::Interrupt, Kind::Isochronous] {
        let a = c.anchor(k);
        wr(a, 0, ED_SKIP);
        wr(a, 12, if k == Kind::Interrupt { c.anchor_bus(Kind::Isochronous) } else { 0 });
    }
    for i in 0..32 {
        wr(hcca, i * 4, c.anchor_bus(Kind::Interrupt));
    }
    sync_to(&c, hcca, PAGE);
    write(HC_INTERRUPT_DISABLE, INTR_MIE | INTR_ALL);
    // The software reset (10 us at most) leaves it suspended: it must be
    // made operational within 2 ms, with the frame interval restored.
    let fit = read(HC_FM_INTERVAL) & FIT;
    write(HC_COMMAND_STATUS, CMD_HCR);
    let mut reset = false;
    for _ in 0..100_000 {
        if read(HC_COMMAND_STATUS) & CMD_HCR == 0 {
            reset = true;
            break;
        }
        core::hint::spin_loop();
    }
    if !reset {
        return Err("reset timed out");
    }
    c.write(HC_FM_INTERVAL, (fit ^ FIT) | FSMPS << 16 | FM_INTERVAL);
    c.write(HC_PERIODIC_START, FM_INTERVAL * 9 / 10);
    c.write(HC_LS_THRESHOLD, 0x628);
    c.write(HC_HCCA, hcca_bus as u32);
    c.write(HC_CONTROL_HEAD_ED, c.anchor_bus(Kind::Control));
    c.write(HC_BULK_HEAD_ED, c.anchor_bus(Kind::Bulk));
    c.write(HC_INTERRUPT_STATUS, INTR_ALL);
    c.write(HC_CONTROL, HCFS_OPERATIONAL | CTL_CBSR_4 | CTL_PLE | CTL_IE | CTL_CLE | CTL_BLE);
    // Port power: none to switch, all at once, or port by port.
    let rha = c.read(HC_RH_DESCRIPTOR_A);
    if rha & RHA_NPS == 0 {
        if rha & RHA_PSM == 0 {
            c.write(HC_RH_STATUS, RHS_LPSC);
        } else {
            for p in 0..ports {
                c.write(HC_RH_PORT_STATUS + 4 * usize::from(p), PORT_PPS);
            }
        }
        sleep_ms(u64::from(rha >> 24) * 2 + 20);
    }
    Ok(c)
}

pub fn enable_interrupts(c: &Controller) {
    c.write(HC_INTERRUPT_ENABLE, INTR_MIE | INTR_WDH | INTR_RHSC | INTR_UE);
}

/// The controller's status, acknowledged: a TD retired (or the done
/// queue written) wakes its waiters and the core; a root port change goes
/// to the core. The root hub's interrupt is masked until the core read
/// the changed ports ([`port_status`]): it stays raised while a change
/// bit is set.
pub fn process(c: &Controller) {
    let sts = c.read(HC_INTERRUPT_STATUS) & INTR_ALL;
    if sts == 0 {
        return;
    }
    c.write(HC_INTERRUPT_STATUS, sts);
    c.events.fetch_add(1, Ordering::Relaxed);
    if sts & INTR_RHSC != 0 {
        let mut changed = false;
        for p in 0..c.ports {
            if c.read(HC_RH_PORT_STATUS + 4 * usize::from(p)) & PORT_CHANGES != 0 {
                if !changed && c.irq_on.load(Ordering::Relaxed) {
                    c.write(HC_INTERRUPT_DISABLE, INTR_RHSC);
                }
                changed = true;
                crate::core_port_change(c, p);
            }
        }
    }
    api().wake(c.key());
    crate::core_wake(c);
}

// ---- Ports ------------------------------------------------------------------

pub struct PortState {
    pub connected: bool,
    pub enabled: bool,
    pub speed: u8,
}

/// Root port `port`, its change bits acknowledged.
pub fn port_status(c: &Controller, port: u8) -> PortState {
    let reg = HC_RH_PORT_STATUS + 4 * usize::from(port);
    let v = c.read(reg);
    c.write(reg, v & PORT_CHANGES);
    if c.irq_on.load(Ordering::Relaxed) {
        c.write(HC_INTERRUPT_ENABLE, INTR_RHSC);
    }
    PortState {
        connected: v & PORT_CCS != 0,
        enabled: v & PORT_PES != 0,
        speed: if v & PORT_LSDA != 0 { USB_SPEED_LOW } else { USB_SPEED_FULL },
    }
}

/// Reset root port `port` (the root hub times it, 10 ms); the port is
/// enabled when it is done.
pub fn port_reset(c: &Controller, port: u8) -> bool {
    let reg = HC_RH_PORT_STATUS + 4 * usize::from(port);
    if c.read(reg) & PORT_CCS == 0 {
        return false;
    }
    c.write(reg, PORT_PRS);
    for _ in 0..50 {
        sleep_ms(2);
        let v = c.read(reg);
        if v & PORT_PRS == 0 {
            c.write(reg, PORT_PRSC | PORT_CSC);
            sleep_ms(10);
            return c.read(reg) & PORT_PES != 0;
        }
    }
    false
}

// ---- Pipes ------------------------------------------------------------------

/// A new endpoint's ED (and a data page when `data`), its queue empty (the
/// dummy TD in slot 0, or at `queue`), linked in after its list's anchor.
pub fn pipe_new(c: &Controller, kind: Kind, t: &Target, data: bool, queue: Option<u32>) -> Option<Pipe> {
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
    p.set_target(t);
    let dummy = queue.unwrap_or(p.td_bus(0));
    wr(va, 4, dummy);
    wr(va, 8, dummy);
    link(c, &p);
    Some(p)
}

fn link(c: &Controller, p: &Pipe) {
    let mut linked = c.schedule.lock();
    if linked.n < MAX_LINKED {
        let n = linked.n;
        linked.eds[n] = (p.bus as u32, p.va as usize);
        linked.n = n + 1;
    }
    let anchor = c.anchor(p.kind);
    wr(p.va, 12, rd(anchor, 12));
    sync_to(c, p.va, PAGE);
    wmb();
    wr(anchor, 12, p.bus as u32);
    sync_to(c, anchor, 16);
}

/// Unlink `p` from its list. A periodic list is walked afresh every
/// frame; the control and bulk lists go on from the ED the controller was
/// at, so with the list stopped for a frame that ED is moved past `p`.
fn unlink(c: &Controller, p: &Pipe) {
    let mut linked = c.schedule.lock();
    if let Some(i) = linked.eds[..linked.n].iter().position(|e| e.0 == p.bus as u32) {
        let n = linked.n;
        linked.eds[i] = linked.eds[n - 1];
        linked.n = n - 1;
    }
    let target = p.bus as u32;
    let mut at = c.anchor(p.kind);
    for _ in 0..=MAX_LINKED {
        let next = rd(at, 12) & !0xF;
        if next == target {
            wr(at, 12, rd(p.va, 12));
            sync_to(c, at, 16);
            break;
        }
        match linked.eds[..linked.n].iter().find(|e| e.0 == next) {
            Some(e) => at = e.1 as *mut u8,
            None => break,
        }
    }
    wmb();
    let (enable, current) = match p.kind {
        Kind::Control => (CTL_CLE, HC_CONTROL_CURRENT_ED),
        Kind::Bulk => (CTL_BLE, HC_BULK_CURRENT_ED),
        _ => {
            sleep_ms(2);
            return;
        }
    };
    c.write(HC_CONTROL, c.read(HC_CONTROL) & !enable);
    sleep_ms(2);
    if c.read(current) == target {
        c.write(current, rd(p.va, 12));
    }
    c.write(HC_CONTROL, c.read(HC_CONTROL) | enable);
}

pub fn pipe_free(c: &Controller, p: &Pipe) {
    unlink(c, p);
    free_page(c.dma, p.bus, p.va);
    if !p.data.is_null() {
        free_page(c.dma, p.data_bus, p.data);
    }
}

/// Queue `tds` on the idle pipe `p` from its dummy on, the last one
/// interrupting: what [`outcome`] needs.
pub fn start(c: &Controller, p: &Pipe, tds: &[Td]) -> Queued {
    let first = p.slot_of(p.tail());
    let n = tds.len();
    let mut q = Queued { first, n, lens: [0; 3], bufs: [0; 3] };
    for (k, t) in tds.iter().enumerate() {
        let td = p.td(first + k);
        let toggle = match t.toggle {
            None => 0,
            Some(false) => TD_TOGGLE_DATA0,
            Some(true) => TD_TOGGLE_DATA1,
        };
        let irq = if k + 1 == n { 0 } else { TD_NO_IRQ };
        let rounding = if t.pid == DP_IN { TD_ROUNDING } else { 0 };
        wr(td, 0, CC_NOT_ACCESSED << TD_CC_SHIFT | toggle | irq | t.pid << 19 | rounding);
        let (cbp, be) = if t.len == 0 { (0, 0) } else { (t.buf as u32, (t.buf + t.len as u64 - 1) as u32) };
        wr(td, 4, cbp);
        wr(td, 8, p.td_bus(first + k + 1));
        wr(td, 12, be);
        if k < 3 {
            q.lens[k] = t.len as u16;
            q.bufs[k] = cbp;
        }
    }
    // The new dummy.
    let dummy = p.td(first + n);
    for off in [0, 4, 8, 12] {
        wr(dummy, off, 0);
    }
    sync_to(c, p.va, PAGE);
    wmb();
    wr(p.va, 4, p.td_bus(first + n));
    sync_to(c, p.va, 16);
    wmb();
    match p.kind {
        Kind::Control => c.write(HC_COMMAND_STATUS, CMD_CLF),
        Kind::Bulk => c.write(HC_COMMAND_STATUS, CMD_BLF),
        _ => {}
    }
    q
}

/// The queue empty again, a halt cleared, the toggle kept (or reset to
/// DATA0 when `reset_toggle`): after an error, a timeout or a cancel. A
/// running queue is skipped for a frame first.
pub fn clear(c: &Controller, p: &Pipe, reset_toggle: bool) {
    sync_from(c, p.va, 16);
    let halted = p.head() & ED_HALTED != 0;
    if !halted && p.head() & !0xF != p.tail() & !0xF {
        wr(p.va, 0, rd(p.va, 0) | ED_SKIP);
        sync_to(c, p.va, 16);
        sleep_ms(2);
    }
    let carry = if reset_toggle { 0 } else { p.head() & ED_CARRY };
    wr(p.va, 8, p.tail() & !0xF | carry);
    wr(p.va, 0, rd(p.va, 0) & !ED_SKIP);
    sync_to(c, p.va, 16);
    wmb();
}

/// What the TDs `q` on `p` came to: `None` while they run; else the bytes
/// the data TDs moved (a control transfer's setup TD, its first, not
/// counted when `control`), or the error.
pub fn outcome(c: &Controller, p: &Pipe, q: &Queued, control: bool) -> Option<Result<usize, i32>> {
    sync_from(c, p.va, PAGE);
    let head = p.head();
    let halted = head & ED_HALTED != 0;
    if !halted && head & !0xF != p.tail() & !0xF {
        return None;
    }
    let mut moved = 0usize;
    for k in 0..q.n {
        let td = p.td(q.first + k);
        let cc = rd(td, 0) >> TD_CC_SHIFT;
        if cc == CC_NOT_ACCESSED {
            // Not reached: an earlier TD failed.
            break;
        }
        if cc != CC_NO_ERROR {
            return Some(Err(if cc == CC_STALL { USB_ESTALL } else { USB_EIO }));
        }
        if control && k == 0 {
            continue;
        }
        let len = usize::from(q.lens[k.min(2)]);
        let cbp = rd(td, 4);
        moved += if cbp == 0 { len } else { (cbp - q.bufs[k.min(2)]) as usize };
    }
    if halted {
        return Some(Err(USB_EIO));
    }
    Some(Ok(moved))
}

/// Wait for the TDs `q` on `p` until `deadline`: their outcome, or a
/// timeout (or [`USB_EGONE`] when `gone` is raised) with the queue
/// emptied. A failed transfer's queue is emptied too.
pub fn wait(c: &Controller, p: &Pipe, q: &Queued, control: bool, deadline: u64, gone: &AtomicBool) -> Result<usize, i32> {
    loop {
        let seq = api().wait_seq();
        if !c.irq_on.load(Ordering::Relaxed) {
            process(c);
        }
        if let Some(r) = outcome(c, p, q, control) {
            if r.is_err() {
                clear(c, p, false);
            }
            return r;
        }
        let now = api().monotonic_ns();
        if gone.load(Ordering::Acquire) || now >= deadline {
            clear(c, p, false);
            return Err(if gone.load(Ordering::Acquire) { USB_EGONE } else { USB_ETIMEDOUT });
        }
        let step = if c.irq_on.load(Ordering::Relaxed) { 10_000_000 } else { 1_000_000 };
        api().block_until(c.key(), seq, (now + step).min(deadline));
    }
}

// ---- Isochronous ------------------------------------------------------------

/// An isochronous OUT endpoint `t`'s ED, linked in, and its ring for
/// packets of up to `max_packet` bytes.
pub fn iso_new(c: &Controller, t: &Target) -> Option<Iso> {
    let slot = usize::from(t.max_packet.max(16)).next_power_of_two().min(1024);
    let per_td = (PAGE / slot).min(8);
    // Room for every packet the core may queue, and the dummy.
    let ring = (ISO_PACKETS.div_ceil(per_td) + 2).min(ISO_RING);
    let data_pages = (ring * per_td * slot).div_ceil(PAGE);
    let (tds_bus, tds) = pages(c.dma, 1)?;
    let Some((data_bus, data)) = pages(c.dma, data_pages) else {
        free_page(c.dma, tds_bus, tds);
        return None;
    };
    // Its queue in the ring: the dummy in TD 0.
    let Some(pipe) = pipe_new(c, Kind::Isochronous, t, false, Some(tds_bus as u32)) else {
        free_page(c.dma, tds_bus, tds);
        return None;
    };
    Some(Iso {
        pipe,
        tds_bus,
        tds,
        data_bus,
        data,
        data_pages,
        slot,
        per_td,
        ring,
        oldest: 0,
        tail_slot: 0,
        packets: [0; ISO_RING],
        frame: 0,
        in_flight: 0,
        done: 0,
    })
}

fn iso_td(iso: &Iso, i: usize) -> *mut u8 {
    unsafe { iso.tds.add(i * 32) }
}

fn iso_td_bus(iso: &Iso, i: usize) -> u32 {
    (iso.tds_bus + (i * 32) as u64) as u32
}

/// Queue `lens` packets from `data` on the endpoint, one a frame after
/// the last one queued (at least two frames ahead of the controller), in
/// isochronous TDs of up to `per_td` frames: `USB_EBUSY` when the ring has
/// no room. A zero-length packet is not sent (counted gone).
pub fn iso_queue(c: &Controller, iso: &mut Iso, data: &[u8], lens: &[u16], max_packet: u16) -> i32 {
    iso_reap(c, iso);
    if iso.in_flight + lens.len() > ISO_PACKETS {
        return USB_EBUSY;
    }
    let tds_needed = lens.len().div_ceil(iso.per_td);
    let busy = iso.packets.iter().take(iso.ring).filter(|&&n| n != 0).count();
    if busy + tds_needed + 1 > iso.ring {
        return USB_EBUSY;
    }
    let now = c.frame_number();
    let ahead = iso.frame.wrapping_sub(now);
    if iso.in_flight == 0 || ahead < 2 || ahead > 1000 {
        iso.frame = now.wrapping_add(2);
    }
    let mut at = 0usize;
    for chunk in lens.chunks(iso.per_td) {
        let i = iso.tail_slot;
        let next = (i + 1) % iso.ring;
        let region = i * iso.per_td * iso.slot;
        let page_bus = iso.data_bus + region as u64;
        let buf = unsafe { iso.data.add(region) };
        let td = iso_td(iso, i);
        let mut offsets = [0u16; 8];
        let mut used = 0usize;
        let mut sent = 0usize;
        for &len in chunk {
            let len = usize::from(len).min(usize::from(max_packet)).min(iso.slot);
            if len == 0 {
                iso.done += 1;
                continue;
            }
            unsafe { core::ptr::copy_nonoverlapping(data[at..].as_ptr(), buf.add(used), len) };
            at += len;
            // Its offset in the page (the region never crosses one), the
            // condition code "not accessed".
            offsets[sent] = 0xE000 | ((page_bus as usize + used) & 0xFFF) as u16;
            used += len;
            sent += 1;
        }
        if sent == 0 {
            continue;
        }
        sync_to(c, buf, used);
        let frame = iso.frame;
        iso.frame = iso.frame.wrapping_add(sent as u16);
        wr(td, 0, CC_NOT_ACCESSED << TD_CC_SHIFT | ((sent as u32 - 1) << 24) | u32::from(frame));
        wr(td, 4, (page_bus as u32) & !0xFFF);
        wr(td, 8, iso_td_bus(iso, next));
        wr(td, 12, (page_bus + used as u64 - 1) as u32);
        for k in 0..4 {
            wr(td, 16 + k * 4, u32::from(offsets[2 * k]) | u32::from(offsets[2 * k + 1]) << 16);
        }
        let dummy = iso_td(iso, next);
        for off in (0..32).step_by(4) {
            wr(dummy, off, 0);
        }
        sync_to(c, iso.tds, PAGE);
        wmb();
        wr(iso.pipe.va, 4, iso_td_bus(iso, next));
        sync_to(c, iso.pipe.va, 16);
        iso.packets[i] = sent as u8;
        iso.in_flight += sent;
        iso.tail_slot = next;
    }
    0
}

/// The packets that went since the last look: the TDs the head pointer
/// passed (sent, or expired unsent), added to `iso.done`.
pub fn iso_reap(c: &Controller, iso: &mut Iso) {
    sync_from(c, iso.pipe.va, 16);
    let head = iso.pipe.head() & !0xF;
    while iso.oldest != iso.tail_slot && iso_td_bus(iso, iso.oldest) != head {
        let n = usize::from(iso.packets[iso.oldest]);
        iso.packets[iso.oldest] = 0;
        iso.in_flight -= n;
        iso.done += n as u32;
        iso.oldest = (iso.oldest + 1) % iso.ring;
    }
}

/// The endpoint goes: its ED out of the schedule, its pages free.
pub fn iso_free(c: &Controller, iso: Iso) {
    unlink(c, &iso.pipe);
    free_page(c.dma, iso.pipe.bus, iso.pipe.va);
    free_page(c.dma, iso.tds_bus, iso.tds);
    for k in 0..iso.data_pages {
        free_page(c.dma, iso.data_bus + (k * PAGE) as u64, unsafe { iso.data.add(k * PAGE) });
    }
}
