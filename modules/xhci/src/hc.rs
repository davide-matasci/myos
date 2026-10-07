//! The host controller: its registers (the `xhci` crate's layouts), the
//! command and event rings, the interrupter, and the transfer rings of the
//! devices' endpoints with their completion records.
//!
//! Two contexts touch a controller, through `&Controller`. Task context
//! (the USB thread, a class driver's caller) enqueues commands and
//! transfers under locks and waits for their completion records. The
//! interrupt handler (or, without an interrupt, whoever waits) consumes
//! the event ring: it fills the records, flags port changes and wakes the
//! waiters. The handler takes no lock and allocates nothing; the two meet
//! only through atomics.

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::num::NonZeroUsize;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, AtomicU8, Ordering};

use myos_abi::{SleepLock, USB_EGONE, USB_EIO, USB_ESTALL, USB_ETIMEDOUT, UsbCompletion};
use xhci::accessor::Mapper;
use xhci::ring::trb::{Link, command, event, transfer};

use crate::{api, dma};

/// TRBs per ring: one 4 KiB segment, the last TRB a Link back to the start.
pub const TRBS: usize = 256;
const TRB_BYTES: usize = 16;
/// Controllers this module drives.
pub const MAX_CTRL: usize = 2;
/// Device slots used per controller (`devices` table size).
pub const MAX_DEVICES: usize = 32;
/// Endpoint contexts per device (DCI 1..=31).
pub const MAX_DCI: usize = 32;
/// Bytes of the bulk bounce buffer (16 contiguous pages; a page when the
/// kernel cannot give more).
const BOUNCE_PAGES: usize = 16;

/// The registers are reached through the BAR the kernel mapped: the
/// "physical" base the crate asks about is already a virtual address.
#[derive(Clone)]
pub struct Ident;

impl Mapper for Ident {
    unsafe fn map(&mut self, phys_base: usize, _bytes: usize) -> NonZeroUsize {
        NonZeroUsize::new(phys_base).expect("xhci: null register base")
    }

    fn unmap(&mut self, _virt_base: usize, _bytes: usize) {}
}

pub type Regs = xhci::Registers<Ident>;

/// A producer ring (command or transfer): TRBs, the enqueue index and the
/// producer cycle state.
pub struct Ring {
    pub trbs: *mut [u32; 4],
    pub phys: u64,
    pub enq: usize,
    pub cycle: bool,
    /// The last TRB pushed had its chain bit set: a TD in progress, which
    /// the Link TRB must chain through if it comes next.
    last_chain: bool,
}

// SAFETY: the TRBs are the ring's own page.
unsafe impl Send for Ring {}

/// The slot after `i`, skipping the Link TRB.
pub fn next_slot(i: usize) -> usize {
    if i + 1 >= TRBS - 1 { 0 } else { i + 1 }
}

impl Ring {
    pub fn new() -> Option<Ring> {
        let (phys, va) = dma::page()?;
        let mut r = Ring { trbs: va as *mut [u32; 4], phys, enq: 0, cycle: true, last_chain: false };
        r.reset();
        Some(r)
    }

    /// Back to an empty ring: zeroed, the Link TRB in the last slot.
    pub fn reset(&mut self) {
        unsafe {
            core::ptr::write_bytes(self.trbs as *mut u8, 0, TRBS * TRB_BYTES);
        }
        let mut link = Link::new();
        link.set_ring_segment_pointer(self.phys).set_toggle_cycle();
        let raw = link.into_raw();
        unsafe {
            core::ptr::write_volatile(self.trbs.add(TRBS - 1), raw);
        }
        self.enq = 0;
        self.cycle = true;
        self.last_chain = false;
        dma::clean(self.trbs as *mut u8, TRBS * TRB_BYTES);
    }

    /// The physical address of slot `i`.
    pub fn slot_phys(&self, i: usize) -> u64 {
        self.phys + (i * TRB_BYTES) as u64
    }

    /// The slot index of the TRB at `phys`, if on the ring at `base`.
    pub fn index_in(base: u64, phys: u64) -> Option<usize> {
        if phys < base || phys >= base + (TRBS * TRB_BYTES) as u64 {
            return None;
        }
        Some(((phys - base) / TRB_BYTES as u64) as usize)
    }

    /// Where the next TRB goes, with the cycle state (`Set TR Dequeue`).
    pub fn enqueue_pointer(&self) -> u64 {
        self.slot_phys(self.enq) | u64::from(self.cycle)
    }

    /// Write `trb` at the enqueue slot with the producer cycle bit, and
    /// advance. Reaching the Link TRB hands it the cycle bit (and the chain
    /// bit when a TD continues past it) and toggles the cycle. The slot's
    /// index.
    pub fn push(&mut self, mut trb: [u32; 4]) -> usize {
        if self.enq == TRBS - 1 {
            let mut link = unsafe { core::ptr::read_volatile(self.trbs.add(TRBS - 1)) };
            link[3] = (link[3] & !(1 | (1 << 4))) | u32::from(self.cycle) | (u32::from(self.last_chain) << 4);
            unsafe {
                core::ptr::write_volatile(self.trbs.add(TRBS - 1), link);
            }
            dma::clean(unsafe { self.trbs.add(TRBS - 1) } as *mut u8, TRB_BYTES);
            self.enq = 0;
            self.cycle = !self.cycle;
        }
        let i = self.enq;
        trb[3] = (trb[3] & !1) | u32::from(self.cycle);
        self.last_chain = trb[3] & (1 << 4) != 0;
        unsafe {
            core::ptr::write_volatile(self.trbs.add(i), trb);
        }
        dma::clean(unsafe { self.trbs.add(i) } as *mut u8, TRB_BYTES);
        self.enq += 1;
        i
    }

    /// The transfer length of TRB `i` of the ring at `trbs`.
    pub fn trb_len_at(trbs: *mut [u32; 4], i: usize) -> u32 {
        // SAFETY: a ring is one page of `TRBS` TRBs and `i` is below that.
        let t = unsafe { core::ptr::read_volatile(trbs.add(i)) };
        t[2] & 0x1_FFFF
    }
}

/// One transfer descriptor in flight on an endpoint, filled in by the event
/// handler. `td_start..td_start + td_trbs` are its TRBs (Link excluded),
/// `total` the bytes they carry. Atomics only: the handler reads and
/// writes it with no lock held, and it lives in the device's slot
/// (`usb::Slot`), outside the lock a transfer holds.
pub struct Pending {
    pub active: AtomicBool,
    pub done: AtomicBool,
    pub code: AtomicU8,
    pub transferred: AtomicU32,
    /// Bytes moved up to a short packet on a TRB before the last one
    /// (`u32::MAX`: none); the event for the last TRB completes the TD.
    pub short: AtomicU32,
    pub td_start: AtomicU32,
    pub td_trbs: AtomicU32,
    pub total: AtomicU32,
    /// An asynchronous transfer (`interrupt_start`): the USB thread runs
    /// the endpoint's callback when `done`.
    pub has_callback: AtomicBool,
    /// The endpoint's ring (base and TRBs), for the handler to find the
    /// TD's TRBs; null while the endpoint does not exist.
    pub ring_phys: AtomicU64,
    pub ring_trbs: AtomicPtr<[u32; 4]>,
}

impl Pending {
    pub const fn new() -> Self {
        Pending {
            active: AtomicBool::new(false),
            done: AtomicBool::new(false),
            code: AtomicU8::new(0),
            transferred: AtomicU32::new(0),
            short: AtomicU32::new(u32::MAX),
            td_start: AtomicU32::new(0),
            td_trbs: AtomicU32::new(0),
            total: AtomicU32::new(0),
            has_callback: AtomicBool::new(false),
            ring_phys: AtomicU64::new(0),
            ring_trbs: AtomicPtr::new(core::ptr::null_mut()),
        }
    }

    /// The endpoint's ring, for the handler.
    pub fn set_ring(&self, ring: &Ring) {
        self.ring_phys.store(ring.phys, Ordering::Relaxed);
        self.ring_trbs.store(ring.trbs, Ordering::Release);
    }

    /// No endpoint any more.
    pub fn clear_ring(&self) {
        self.ring_trbs.store(core::ptr::null_mut(), Ordering::Release);
        self.ring_phys.store(0, Ordering::Relaxed);
    }

    pub fn arm(&self, td_start: usize, td_trbs: usize, total: usize) {
        self.td_start.store(td_start as u32, Ordering::Relaxed);
        self.td_trbs.store(td_trbs as u32, Ordering::Relaxed);
        self.total.store(total as u32, Ordering::Relaxed);
        self.transferred.store(0, Ordering::Relaxed);
        self.short.store(u32::MAX, Ordering::Relaxed);
        self.code.store(0, Ordering::Relaxed);
        self.done.store(false, Ordering::Relaxed);
        self.active.store(true, Ordering::Release);
    }

    /// What waiters block on and the event handler wakes.
    pub fn key(&self) -> usize {
        self.done.as_ptr() as usize
    }

    /// The completion as a transfer result: bytes, or a `USB_E*` error.
    pub fn result(&self) -> i32 {
        match self.code.load(Ordering::Acquire) {
            1 | 13 => self.transferred.load(Ordering::Relaxed) as i32, // Success, Short Packet
            6 => USB_ESTALL,
            0xFF => USB_EGONE,
            _ => USB_EIO,
        }
    }
}

/// An endpoint of an addressed device: its transfer ring, and what the
/// one TD in flight on it (`Pending`, in the device's slot) has to hand
/// back. Reached through the device's lock.
pub struct Endpoint {
    pub ring: Ring,
    pub address: u8,
    /// Transfer type (`bmAttributes & 3`): 0 control, 2 bulk, 3 interrupt.
    pub kind: u8,
    pub max_packet: u16,
    /// A bounce slot for small transfers (control data, interrupt reports).
    pub bounce: *mut u8,
    pub bounce_phys: u64,
    pub bounce_len: usize,
    /// An asynchronous transfer (`interrupt_start`): its callback and the
    /// caller's buffer, which gets the bytes when the TD is done.
    pub callback: Option<(UsbCompletion, *mut c_void)>,
    pub user: *mut u8,
    pub user_len: usize,
}

/// The command ring's one command in flight.
pub struct CmdWait {
    pub done: AtomicBool,
    pub code: AtomicU8,
    pub slot: AtomicU8,
}

/// The event ring and the consumer's place on it.
struct EventRing {
    trbs: *mut [u32; 4],
    phys: u64,
    deq: usize,
    cycle: bool,
}

/// The bulk bounce buffer.
pub struct Bounce {
    pub va: *mut u8,
    pub phys: u64,
    pub len: usize,
}

// SAFETY: the buffer is the controller's own pages.
unsafe impl Send for Bounce {}

/// A controller and what hangs from it, reached as `&Controller` from
/// every context: each part is an atomic, behind a lock, or written by
/// one context alone (the USB thread for the DCBAA, `init` for the rest).
/// The devices are in `crate::usb`'s table, indexed by the table slot
/// (not the xHCI slot id).
pub struct Controller {
    pub mmio_base: usize,
    /// The first port's `PORTSC` (operational base + 0x400).
    pub portsc_base: usize,
    pub index: u8,
    pub ports: u8,
    pub slots: u8,
    pub csz64: bool,
    /// The device context base address array: the USB thread alone
    /// writes it (`attach`, `detach_id`).
    pub dcbaa: *mut u64,
    /// The command ring, held from a command to its completion.
    pub cmd: SleepLock<Ring>,
    pub cmd_wait: CmdWait,
    /// The event ring: whoever holds `evt_busy` is its one consumer (the
    /// interrupt handler, or a waiter polling); the other skips its turn.
    evt: UnsafeCell<EventRing>,
    evt_busy: AtomicBool,
    /// The interrupt is routed (`pci_irq_enable` succeeded).
    pub irq_on: AtomicBool,
    /// Events handled, for the boot log.
    pub events: AtomicU32,
    /// Root ports whose status changed (bit = port index), for the thread.
    pub port_change: AtomicU64,
    /// An asynchronous transfer completed: the thread runs its callback.
    pub async_done: AtomicBool,
    /// The bulk bounce buffer, held for a transfer.
    pub bounce: SleepLock<Bounce>,
    /// Which xHCI slot ids are in use: `slot_owner[slot] = device table
    /// index + 1` (the thread writes, the event handler reads).
    pub slot_owner: [AtomicU8; 256],
    /// Root port protocol major version (2 or 3), from the supported
    /// protocol capabilities; 0 unknown.
    pub port_major: [u8; 64],
}

// SAFETY: see the struct: nothing in it is written through a shared
// reference but atomics, the locked parts, and the event ring under
// `evt_busy`; the raw pointers are the controller's own DMA pages.
unsafe impl Sync for Controller {}
unsafe impl Send for Controller {}

impl Controller {
    /// The registers as the `xhci` crate reaches them, made per use (a
    /// few capability reads): the crate wants `&mut` for a write, and the
    /// contexts that write (the handler, the thread, a transfer) must not
    /// share one.
    pub fn regs(&self) -> Regs {
        // SAFETY: the BAR the kernel mapped for this controller.
        unsafe { Regs::new(self.mmio_base, Ident) }
    }
}

/// `USBSTS` / `IMAN` acknowledgement and the event ring: what the interrupt
/// handler does. Also run by waiters when the controller has no interrupt.
pub fn process_events(c: &Controller) {
    if c.evt_busy.swap(true, Ordering::AcqRel) {
        return;
    }
    let mut regs = c.regs();
    let sts = regs.operational.usbsts.read_volatile();
    if sts.event_interrupt() {
        regs.operational.usbsts.update_volatile(|s| {
            s.clear_event_interrupt();
        });
    }
    if sts.port_change_detect() {
        regs.operational.usbsts.update_volatile(|s| {
            s.clear_port_change_detect();
        });
    }
    let mut ir = regs.interrupter_register_set.interrupter_mut(0);
    let iman = ir.iman.read_volatile();
    if iman.interrupt_pending() {
        ir.iman.update_volatile(|m| {
            m.clear_interrupt_pending();
        });
    }
    // SAFETY: `evt_busy` is ours until the store below: no other consumer.
    let ev = unsafe { &mut *c.evt.get() };
    let mut any = false;
    loop {
        dma::invalidate(unsafe { ev.trbs.add(ev.deq) } as *mut u8, TRB_BYTES);
        let raw = unsafe { core::ptr::read_volatile(ev.trbs.add(ev.deq)) };
        if (raw[3] & 1 != 0) != ev.cycle {
            break;
        }
        any = true;
        c.events.fetch_add(1, Ordering::Relaxed);
        handle_event(c, raw);
        ev.deq += 1;
        if ev.deq == TRBS {
            ev.deq = 0;
            ev.cycle = !ev.cycle;
        }
    }
    if any || iman.interrupt_pending() {
        // The dequeue pointer, clearing Event Handler Busy (write 1).
        let p = ev.phys + (ev.deq * TRB_BYTES) as u64;
        regs.interrupter_register_set.interrupter_mut(0).erdp.update_volatile(|r| {
            r.set_event_ring_dequeue_pointer(p);
            r.clear_event_handler_busy();
        });
    }
    c.evt_busy.store(false, Ordering::Release);
}

fn handle_event(c: &Controller, raw: [u32; 4]) {
    match event::Allowed::try_from(raw) {
        Ok(event::Allowed::CommandCompletion(e)) => {
            let code = e.completion_code().map_or(0xFE, |c| c as u8);
            c.cmd_wait.slot.store(e.slot_id(), Ordering::Relaxed);
            c.cmd_wait.code.store(code, Ordering::Relaxed);
            c.cmd_wait.done.store(true, Ordering::Release);
            api().wake(c.cmd_wait.done.as_ptr() as usize);
        }
        Ok(event::Allowed::TransferEvent(e)) => {
            let code = e.completion_code().map_or(0xFE, |c| c as u8);
            let slot = e.slot_id() as usize;
            let dci = e.endpoint_id() as usize;
            let residual = e.trb_transfer_length();
            let owner = c.slot_owner.get(slot).map(|o| o.load(Ordering::Acquire));
            let Some(p) = owner.and_then(|o| crate::usb::pending_at(c.index, o, dci)) else {
                return;
            };
            if !p.active.load(Ordering::Acquire) || p.done.load(Ordering::Acquire) {
                return;
            }
            let trbs = p.ring_trbs.load(Ordering::Acquire);
            if trbs.is_null() {
                return;
            }
            let ring_phys = p.ring_phys.load(Ordering::Relaxed);
            // Bytes moved: the TD's total less this TRB's residual and the
            // TRBs after it the controller skipped (a short packet ends a TD).
            let total = p.total.load(Ordering::Relaxed);
            let start = p.td_start.load(Ordering::Relaxed) as usize;
            let n = p.td_trbs.load(Ordering::Relaxed) as usize;
            // The TD's slots, from `start`, skipping the Link TRB: which
            // one the event names, and the bytes of the ones after it.
            let idx = Ring::index_in(ring_phys, e.trb_pointer());
            let (mut k, mut slot) = (n - 1, start);
            for pos in 0..n {
                if Some(slot) == idx {
                    k = pos;
                    break;
                }
                slot = next_slot(slot);
            }
            let mut moved = total.saturating_sub(residual);
            let mut after = next_slot(slot);
            for _ in k + 1..n {
                moved = moved.saturating_sub(Ring::trb_len_at(trbs, after));
                after = next_slot(after);
            }
            // A short packet before the last TRB: the controller skips to
            // the last TRB (the Status Stage of a control TD) and reports
            // that one too; the TD completes there with the bytes from here.
            if k + 1 < n {
                if code == 13 {
                    p.short.store(moved, Ordering::Relaxed);
                    return;
                }
            } else {
                let short = p.short.load(Ordering::Relaxed);
                if short != u32::MAX {
                    moved = short;
                }
            }
            p.transferred.store(moved, Ordering::Relaxed);
            p.code.store(code, Ordering::Relaxed);
            p.done.store(true, Ordering::Release);
            if p.has_callback.load(Ordering::Acquire) {
                c.async_done.store(true, Ordering::Release);
                api().wake(crate::THREAD_KEY.as_ptr() as usize);
            } else {
                api().wake(p.key());
            }
        }
        Ok(event::Allowed::PortStatusChange(e)) => {
            let port = e.port_id();
            if port >= 1 {
                c.port_change.fetch_or(1 << (port - 1).min(63), Ordering::Release);
                api().wake(crate::THREAD_KEY.as_ptr() as usize);
            }
        }
        _ => {}
    }
}

/// Wait for `done` (a command or a TD), `timeout_ms` at most: false on
/// timeout. The interrupt handler wakes the waiter; the waiter also looks
/// at the event ring itself every few milliseconds, so a controller whose
/// interrupt does not reach us (none, or routed elsewhere) still works.
pub fn wait(c: &Controller, done: &AtomicBool, timeout_ms: u32) -> bool {
    let deadline = api().monotonic_ns() + u64::from(timeout_ms) * 1_000_000;
    loop {
        if done.load(Ordering::Acquire) {
            return true;
        }
        let seq = api().wait_seq();
        process_events(c);
        if done.load(Ordering::Acquire) {
            return true;
        }
        let now = api().monotonic_ns();
        if now >= deadline {
            return false;
        }
        api().block_until(done.as_ptr() as usize, seq, deadline.min(now + 2_000_000));
    }
}

/// Run `trb` on the command ring: the completion code and the slot id of
/// the event (the command lock is held for the duration).
pub fn command(c: &Controller, trb: [u32; 4]) -> Result<(u8, u8), i32> {
    let mut cmd = c.cmd.lock();
    c.cmd_wait.done.store(false, Ordering::Relaxed);
    cmd.push(trb);
    dma::wmb();
    c.regs().doorbell.update_volatile_at(0, |d| {
        d.set_doorbell_target(0);
    });
    let ok = wait(c, &c.cmd_wait.done, 2000);
    if !ok {
        return Err(USB_ETIMEDOUT);
    }
    let code = c.cmd_wait.code.load(Ordering::Relaxed);
    let slot = c.cmd_wait.slot.load(Ordering::Relaxed);
    if code == 1 { Ok((code, slot)) } else { Err(-(i32::from(code)) - 1000) }
}

/// Ring the doorbell of `dci` on `slot`.
pub fn ring_doorbell(c: &Controller, slot: u8, dci: u8) {
    dma::wmb();
    c.regs().doorbell.update_volatile_at(usize::from(slot), |d| {
        d.set_doorbell_target(dci);
    });
}

/// Enqueue a TD of Normal TRBs moving `len` bytes at `phys` (chained, every
/// TRB below 64 KiB and inside one 64 KiB region, the last with IOC, all
/// with ISP) and arm `pending`, the endpoint's record. The caller holds
/// the device's lock and rings the doorbell.
pub fn enqueue_normal(ep: &mut Endpoint, pending: &Pending, phys: u64, len: usize) -> Result<(), i32> {
    let mut chunks: [(u64, usize); 8] = [(0, 0); 8];
    let mut n = 0;
    let (mut p, mut left) = (phys, len);
    loop {
        if n == chunks.len() {
            return Err(USB_EIO);
        }
        let boundary = (p | 0xFFFF) + 1;
        let take = left.min((boundary - p) as usize).min(0x1_0000);
        chunks[n] = (p, take);
        n += 1;
        p += take as u64;
        left -= take;
        if left == 0 {
            break;
        }
    }
    let start = if ep.ring.enq == TRBS - 1 { 0 } else { ep.ring.enq };
    for (i, &(p, take)) in chunks[..n].iter().enumerate() {
        let mut t = transfer::Normal::new();
        t.set_data_buffer_pointer(p)
            .set_trb_transfer_length(take as u32)
            .set_td_size((n - 1 - i).min(31) as u8)
            .set_interrupt_on_short_packet();
        if i + 1 < n {
            t.set_chain_bit();
        } else {
            t.set_interrupt_on_completion();
        }
        ep.ring.push(t.into_raw());
    }
    pending.arm(start, n, len);
    Ok(())
}

/// A control transfer's TD: Setup, an optional Data stage of `len` bytes at
/// `phys` (IN when `to_host`), Status. Arms `pending`, the endpoint's record.
pub fn enqueue_control(
    ep: &mut Endpoint,
    pending: &Pending,
    request_type: u8,
    request: u8,
    value: u16,
    index: u16,
    phys: u64,
    len: u16,
) {
    let to_host = request_type & 0x80 != 0;
    let start = if ep.ring.enq == TRBS - 1 { 0 } else { ep.ring.enq };
    let mut setup = transfer::SetupStage::new();
    setup
        .set_request_type(request_type)
        .set_request(request)
        .set_value(value)
        .set_index(index)
        .set_length(len)
        .set_transfer_type(if len == 0 {
            transfer::TransferType::No
        } else if to_host {
            transfer::TransferType::In
        } else {
            transfer::TransferType::Out
        });
    ep.ring.push(setup.into_raw());
    let mut n = 2;
    if len != 0 {
        let mut data = transfer::DataStage::new();
        data.set_data_buffer_pointer(phys)
            .set_trb_transfer_length(u32::from(len))
            .set_direction(if to_host { transfer::Direction::In } else { transfer::Direction::Out })
            .set_interrupt_on_short_packet();
        ep.ring.push(data.into_raw());
        n = 3;
    }
    let mut status = transfer::StatusStage::new();
    // The status stage runs against the data stage's direction (IN when
    // there was no data).
    if len == 0 || !to_host {
        status.set_direction();
    }
    status.set_interrupt_on_completion();
    ep.ring.push(status.into_raw());
    // Bytes counted: the data stage only (the setup TRB's 8 are immediate
    // and the event for the status stage reports 0 residual).
    pending.arm(start, n, usize::from(len));
}

/// Bring the controller at `mmio` up: BIOS handoff, reset, the device
/// context array (with scratchpads), command and event rings, the
/// interrupter, run. `None` with a reason when it refuses.
pub fn init(mmio: usize, index: u8) -> Result<Controller, &'static str> {
    let mut regs = unsafe { Regs::new(mmio, Ident) };
    let caplength = regs.capability.caplength.read_volatile().get();
    let hcs1 = regs.capability.hcsparams1.read_volatile();
    let hcs2 = regs.capability.hcsparams2.read_volatile();
    let hcc1 = regs.capability.hccparams1.read_volatile();
    let ports = hcs1.number_of_ports();
    let slots = hcs1.number_of_device_slots();
    let csz64 = hcc1.context_size();
    if ports == 0 || slots == 0 {
        return Err("no ports or slots");
    }

    // Extended capabilities: take the controller from the BIOS, learn
    // which ports are USB 3.
    let mut port_major = [0u8; 64];
    if let Some(mut list) = unsafe { xhci::extended_capabilities::List::new(mmio, hcc1, Ident) } {
        for cap in &mut list {
            match cap {
                Ok(xhci::ExtendedCapability::UsbLegacySupport(mut l)) => {
                    l.usblegsup.update_volatile(|s| {
                        s.set_hc_os_owned_semaphore();
                    });
                    let mut spins = 0u32;
                    while l.usblegsup.read_volatile().hc_bios_owned_semaphore() && spins < 1_000_000 {
                        spins += 1;
                        core::hint::spin_loop();
                    }
                    // No SMIs on anything from here on.
                    l.usblegctlsts.update_volatile(|s| {
                        s.clear_usb_smi_enable();
                        s.clear_smi_on_os_ownership_enable();
                        s.clear_smi_on_pci_command_enable();
                        s.clear_smi_on_bar_enable();
                        s.clear_smi_on_os_ownership_change();
                        s.clear_smi_on_pci_command();
                        s.clear_smi_on_bar();
                    });
                }
                Ok(xhci::ExtendedCapability::XhciSupportedProtocol(p)) => {
                    let h = p.header.read_volatile();
                    let first = usize::from(h.compatible_port_offset());
                    let count = usize::from(h.compatible_port_count());
                    for i in first..first + count {
                        if i >= 1 && i - 1 < port_major.len() {
                            port_major[i - 1] = h.major_revision();
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Stop and reset.
    regs.operational.usbcmd.update_volatile(|c| {
        c.clear_run_stop();
    });
    if !spin_until(|| regs.operational.usbsts.read_volatile().hc_halted()) {
        return Err("does not halt");
    }
    regs.operational.usbcmd.update_volatile(|c| {
        c.set_host_controller_reset();
    });
    if !spin_until(|| {
        !regs.operational.usbcmd.read_volatile().host_controller_reset()
            && !regs.operational.usbsts.read_volatile().controller_not_ready()
    }) {
        return Err("does not reset");
    }

    // The device context base address array (slot 0: the scratchpad array).
    let (dcbaa_phys, dcbaa) = dma::page().ok_or("no memory")?;
    unsafe {
        core::ptr::write_bytes(dcbaa, 0, 4096);
    }
    let dcbaa = dcbaa as *mut u64;
    let scratch = hcs2.max_scratchpad_buffers() as usize;
    if scratch > 0 {
        if scratch > 512 {
            return Err("too many scratchpads");
        }
        let (arr_phys, arr) = dma::page().ok_or("no memory")?;
        let arr = arr as *mut u64;
        for i in 0..scratch {
            let (p, va) = dma::page().ok_or("no memory")?;
            unsafe {
                core::ptr::write_bytes(va, 0, 4096);
                core::ptr::write_volatile(arr.add(i), p);
            }
        }
        dma::clean(arr as *mut u8, 4096);
        unsafe {
            core::ptr::write_volatile(dcbaa, arr_phys);
        }
    }
    dma::clean(dcbaa as *mut u8, 4096);
    regs.operational.config.update_volatile(|c| {
        c.set_max_device_slots_enabled(slots);
    });
    regs.operational.dcbaap.update_volatile(|d| {
        d.set(dcbaa_phys);
    });

    // The command ring.
    let cmd = Ring::new().ok_or("no memory")?;
    regs.operational.crcr.update_volatile(|r| {
        r.set_command_ring_pointer(cmd.phys);
        r.set_ring_cycle_state();
    });

    // The event ring: one segment, one interrupter.
    let (evt_phys, evt) = dma::page().ok_or("no memory")?;
    unsafe {
        core::ptr::write_bytes(evt, 0, 4096);
    }
    dma::clean(evt, 4096);
    let (erst_phys, erst) = dma::page().ok_or("no memory")?;
    unsafe {
        core::ptr::write_bytes(erst, 0, 4096);
        core::ptr::write_volatile(erst as *mut u64, evt_phys);
        core::ptr::write_volatile((erst as *mut u64).add(1), TRBS as u64);
    }
    dma::clean(erst, 4096);
    {
        let mut ir = regs.interrupter_register_set.interrupter_mut(0);
        ir.erstsz.update_volatile(|s| {
            s.set(1);
        });
        ir.erdp.update_volatile(|r| {
            r.set_event_ring_dequeue_pointer(evt_phys);
        });
        ir.erstba.update_volatile(|r| {
            r.set(evt_phys_erst(erst_phys));
        });
        ir.imod.update_volatile(|m| {
            m.set_interrupt_moderation_interval(500);
        });
        ir.iman.update_volatile(|m| {
            m.set_interrupt_enable();
        });
    }

    regs.operational.usbcmd.update_volatile(|c| {
        c.set_interrupter_enable();
        c.set_run_stop();
    });
    if !spin_until(|| !regs.operational.usbsts.read_volatile().hc_halted()) {
        return Err("does not run");
    }

    // The bulk bounce buffer.
    let (bounce, bounce_phys, bounce_len) = match dma::pages(BOUNCE_PAGES) {
        Some((p, va)) => (va, p, BOUNCE_PAGES * 4096),
        None => {
            let (p, va) = dma::page().ok_or("no memory")?;
            (va, p, 4096)
        }
    };

    Ok(Controller {
        mmio_base: mmio,
        portsc_base: mmio + usize::from(caplength) + 0x400,
        index,
        ports,
        slots,
        csz64,
        dcbaa,
        cmd: SleepLock::new(&crate::API, cmd),
        cmd_wait: CmdWait { done: AtomicBool::new(false), code: AtomicU8::new(0), slot: AtomicU8::new(0) },
        evt: UnsafeCell::new(EventRing { trbs: evt as *mut [u32; 4], phys: evt_phys, deq: 0, cycle: true }),
        evt_busy: AtomicBool::new(false),
        irq_on: AtomicBool::new(false),
        events: AtomicU32::new(0),
        port_change: AtomicU64::new(0),
        async_done: AtomicBool::new(false),
        bounce: SleepLock::new(&crate::API, Bounce { va: bounce, phys: bounce_phys, len: bounce_len }),
        slot_owner: [const { AtomicU8::new(0) }; 256],
        port_major,
    })
}

fn evt_phys_erst(erst_phys: u64) -> u64 {
    erst_phys
}

fn spin_until(mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..50_000_000u32 {
        if f() {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// A root port's `PORTSC`. Read raw and written with care: the enabled bit
/// and the change bits are write-1-to-clear, so a value read back must not
/// carry them (writing PED back disables the port).
pub struct PortStatus {
    pub connected: bool,
    pub enabled: bool,
    pub speed: u8,
}

const PORTSC_CCS: u32 = 1 << 0;
const PORTSC_PED: u32 = 1 << 1;
const PORTSC_PR: u32 = 1 << 4;
const PORTSC_PP: u32 = 1 << 9;
const PORTSC_PRC: u32 = 1 << 21;
/// CSC, PEC, WRC, OCC, PRC, PLC, CEC.
const PORTSC_CHANGES: u32 = 0x7F << 17;
/// What a write may carry over from a read.
const PORTSC_KEEP: u32 = !(PORTSC_PED | PORTSC_CHANGES | PORTSC_PR | (1 << 31));

fn portsc_ptr(c: &Controller, port: usize) -> *mut u32 {
    (c.portsc_base + port * 0x10) as *mut u32
}

fn portsc_read(c: &Controller, port: usize) -> u32 {
    unsafe { core::ptr::read_volatile(portsc_ptr(c, port)) }
}

fn portsc_write(c: &Controller, port: usize, value: u32) {
    unsafe { core::ptr::write_volatile(portsc_ptr(c, port), value) }
}

pub fn port_status(c: &Controller, port: usize) -> PortStatus {
    let s = portsc_read(c, port);
    PortStatus {
        connected: s & PORTSC_CCS != 0,
        enabled: s & PORTSC_PED != 0,
        speed: ((s >> 10) & 0xF) as u8,
    }
}

/// Acknowledge every change bit of `port`.
pub fn port_ack(c: &Controller, port: usize) {
    let s = portsc_read(c, port);
    portsc_write(c, port, (s & PORTSC_KEEP) | (s & PORTSC_CHANGES));
}

/// Reset `port` (USB 2: required before a device answers; USB 3 ports
/// enable themselves, a reset is harmless): true once enabled.
pub fn port_reset(c: &Controller, port: usize) -> bool {
    let s = portsc_read(c, port);
    portsc_write(c, port, (s & PORTSC_KEEP) | PORTSC_PP | PORTSC_PR);
    for _ in 0..100 {
        crate::sleep_ms(5);
        let s = portsc_read(c, port);
        if s & PORTSC_PRC != 0 || (s & PORTSC_PR == 0 && s & PORTSC_PED != 0) {
            portsc_write(c, port, (s & PORTSC_KEEP) | PORTSC_PRC);
            return portsc_read(c, port) & PORTSC_PED != 0;
        }
    }
    false
}

/// The `command::*` TRBs, raw, with the controller's slot and context.
pub fn trb_enable_slot() -> [u32; 4] {
    command::EnableSlot::new().into_raw()
}

pub fn trb_disable_slot(slot: u8) -> [u32; 4] {
    let mut t = command::DisableSlot::new();
    t.set_slot_id(slot);
    t.into_raw()
}

pub fn trb_address_device(input: u64, slot: u8) -> [u32; 4] {
    let mut t = command::AddressDevice::new();
    t.set_input_context_pointer(input).set_slot_id(slot);
    t.into_raw()
}

pub fn trb_configure_endpoint(input: u64, slot: u8) -> [u32; 4] {
    let mut t = command::ConfigureEndpoint::new();
    t.set_input_context_pointer(input).set_slot_id(slot);
    t.into_raw()
}

pub fn trb_evaluate_context(input: u64, slot: u8) -> [u32; 4] {
    let mut t = command::EvaluateContext::new();
    t.set_input_context_pointer(input).set_slot_id(slot);
    t.into_raw()
}

pub fn trb_reset_endpoint(slot: u8, dci: u8) -> [u32; 4] {
    let mut t = command::ResetEndpoint::new();
    t.set_slot_id(slot).set_endpoint_id(dci);
    t.into_raw()
}

pub fn trb_stop_endpoint(slot: u8, dci: u8) -> [u32; 4] {
    let mut t = command::StopEndpoint::new();
    t.set_slot_id(slot).set_endpoint_id(dci);
    t.into_raw()
}

pub fn trb_set_tr_dequeue(slot: u8, dci: u8, pointer: u64) -> [u32; 4] {
    let mut t = command::SetTrDequeuePointer::new();
    t.set_new_tr_dequeue_pointer(pointer & !0xF).set_slot_id(slot).set_endpoint_id(dci);
    if pointer & 1 != 0 {
        t.set_dequeue_cycle_state();
    }
    t.into_raw()
}
