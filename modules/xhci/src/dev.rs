//! The devices, the controller's side (`UsbHcdOps`, `docs/usb.md`): a
//! slot and its contexts for each device the core enumerates, the
//! endpoints' transfer rings, and the transfers. The core calls in one
//! call per device at a time, from its thread or a class driver's caller;
//! `device_abort` is the exception, which touches only the completion
//! records.
//!
//! A device lives in a [`Slot`] behind a `SleepLock`, held for a call. What
//! the interrupt handler touches is outside the lock: the endpoints'
//! completion records (`Pending`, atomics).

use core::sync::atomic::Ordering;

use myos_abi::{
    SleepLock, USB_ASYNC_BULK_MAX, USB_EGONE, USB_EIO, USB_EPENDING, USB_ETIMEDOUT, USB_SPEED_FULL, USB_SPEED_HIGH,
    USB_SPEED_LOW, USB_SPEED_SUPER, UsbDevicePath, UsbEndpoint,
};
use xhci::context::{
    Device32Byte, Device64Byte, DeviceHandler, EndpointType, Input32Byte, Input64Byte, InputHandler,
};

use crate::dma;
use crate::hc::{self, Controller, Endpoint, MAX_DCI, MAX_DEVICES, Pending, Ring};

/// A control transfer's data stage at most (the descriptors).
const CONTROL_MAX: usize = 1024;

pub struct Device {
    pub slot: u8,
    pub path: UsbDevicePath,
    /// The xHCI port speed id (slot context).
    pub psi: u8,
    /// The transaction translator for a low/full-speed device behind a
    /// high-speed hub: that hub's slot and the port the device is on.
    pub tt: Option<(u8, u8)>,
    /// A page: the input context at 0, control-transfer bounce after it.
    pub input: *mut u8,
    pub input_phys: u64,
    /// A page: the output device context at 0, interrupt bounce slots after it.
    pub out: *mut u8,
    pub out_phys: u64,
    pub eps: [Option<Endpoint>; MAX_DCI],
}

// SAFETY: the pointers are the device's own DMA pages (contexts, rings,
// bounce slots), used only by whoever holds its slot's lock.
unsafe impl Send for Device {}

/// A device table entry. The index + 1 is the handle the core knows the
/// device by, and the xHCI slot's `slot_owner` entry names it for the
/// event handler.
pub struct Slot {
    /// The endpoints' completion records, by DCI: the handler fills them
    /// with no lock held.
    pub pending: [Pending; MAX_DCI],
    dev: SleepLock<Option<Device>>,
}

impl Slot {
    const EMPTY: Self = Self { pending: [const { Pending::new() }; MAX_DCI], dev: SleepLock::new(&crate::API, None) };
}

static DEVICES: [[Slot; MAX_DEVICES]; hc::MAX_CTRL] = [const { [const { Slot::EMPTY }; MAX_DEVICES] }; hc::MAX_CTRL];

const INPUT_CTX_BYTES: usize = 33 * 64;
const OUT_CTX_BYTES: usize = 32 * 64;
const CONTROL_BOUNCE: usize = INPUT_CTX_BYTES;
/// Interrupt bounce slot of DCI `d`: 64 bytes each, after the output context.
fn int_bounce(d: usize) -> usize {
    OUT_CTX_BYTES + d * 64
}

/// The table entry `index` of controller `ctrl`.
fn slot_at(ctrl: u8, index: usize) -> Option<&'static Slot> {
    DEVICES.get(usize::from(ctrl))?.get(index)
}

/// The completion record of endpoint `dci` of the device entry `owner`
/// (`slot_owner` value: table index + 1) names, for the event handler.
pub fn pending_at(ctrl: u8, owner: u8, dci: usize) -> Option<&'static Pending> {
    if owner == 0 {
        return None;
    }
    slot_at(ctrl, usize::from(owner) - 1)?.pending.get(dci)
}

/// The table entry the handle `hdev` (table index + 1) names.
fn slot_of(ctrl: u8, hdev: u32) -> Option<&'static Slot> {
    if hdev == 0 {
        return None;
    }
    slot_at(ctrl, hdev as usize - 1)
}

/// Run `f` on the device `hdev` of `c` with its slot's lock held:
/// `USB_EGONE` when there is no such device.
pub fn with_device<R>(c: &Controller, hdev: u32, f: impl FnOnce(&Slot, &mut Device) -> R) -> Result<R, i32> {
    let s = slot_of(c.index, hdev).ok_or(USB_EGONE)?;
    let mut dev = s.dev.lock();
    let dev = dev.as_mut().ok_or(USB_EGONE)?;
    Ok(f(s, dev))
}

/// The device `hdev`'s field `f`, if it is there: a copy out from under the
/// lock.
fn peek<R>(ctrl: u8, hdev: u32, f: impl FnOnce(&Device) -> R) -> Option<R> {
    let s = slot_of(ctrl, hdev)?;
    let dev = s.dev.lock();
    dev.as_ref().map(f)
}

impl Device {
    pub fn endpoint_mut(&mut self, dci: usize) -> Option<&mut Endpoint> {
        self.eps.get_mut(dci)?.as_mut()
    }

    /// The endpoint with `address` (direction bit included).
    pub fn endpoint_by_address(&mut self, address: u8) -> Option<&mut Endpoint> {
        self.eps.iter_mut().flatten().find(|e| e.address == address)
    }

    fn input_ctx(&mut self, csz64: bool) -> &mut dyn InputHandler {
        unsafe {
            if csz64 {
                &mut *(self.input as *mut Input64Byte)
            } else {
                &mut *(self.input as *mut Input32Byte)
            }
        }
    }

    fn out_ctx(&self, csz64: bool) -> &dyn DeviceHandler {
        unsafe {
            if csz64 {
                &*(self.out as *const Device64Byte)
            } else {
                &*(self.out as *const Device32Byte)
            }
        }
    }

    fn clear_input(&mut self) {
        unsafe {
            core::ptr::write_bytes(self.input, 0, INPUT_CTX_BYTES);
        }
    }

    /// The slot context's fields every input context repeats.
    fn fill_slot_context(&mut self, csz64: bool, context_entries: u8) {
        let (route, psi, root_port, tt) = (self.path.route, self.psi, self.path.root_port, self.tt);
        let input = self.input_ctx(csz64);
        input.control_mut().set_add_context_flag(0);
        let s = input.device_mut().slot_mut();
        s.set_route_string(route);
        s.set_speed(psi);
        s.set_context_entries(context_entries);
        s.set_root_hub_port_number(root_port);
        if let Some((hub_slot, port)) = tt {
            s.set_parent_hub_slot_id(hub_slot);
            s.set_parent_port_number(port);
        }
    }
}

/// `bEndpointAddress` to the device context index.
pub fn dci_of(address: u8) -> usize {
    let n = usize::from(address & 0xF);
    if n == 0 { 1 } else { n * 2 + usize::from(address >> 7) }
}

pub fn speed_from_psi(psi: u8) -> u8 {
    match psi {
        1 => USB_SPEED_FULL,
        2 => USB_SPEED_LOW,
        3 => USB_SPEED_HIGH,
        _ => USB_SPEED_SUPER,
    }
}

pub fn psi_from_speed(speed: u8) -> u8 {
    match speed {
        USB_SPEED_LOW => 2,
        USB_SPEED_FULL => 1,
        USB_SPEED_HIGH => 3,
        _ => 4,
    }
}

fn ep0_max_packet(speed: u8) -> u16 {
    match speed {
        USB_SPEED_LOW | USB_SPEED_FULL => 8,
        USB_SPEED_HIGH => 64,
        _ => 512,
    }
}

/// The endpoint context interval (the 2^n microframe service interval)
/// from a descriptor's `bInterval`.
fn ep_interval(speed: u8, kind: u8, b_interval: u8) -> u8 {
    match kind {
        3 | 1 => {
            if speed == USB_SPEED_HIGH || speed == USB_SPEED_SUPER {
                b_interval.clamp(1, 16) - 1
            } else {
                // Full/low speed: bInterval frames; 2^(n-3) frames.
                let frames = u32::from(b_interval.max(1));
                (frames.ilog2() as u8 + 3).clamp(3, 10)
            }
        }
        _ => 0,
    }
}

fn new_endpoint(dma: i32, dev: &mut Device, address: u8, kind: u8, max_packet: u16) -> Option<Endpoint> {
    let dci = dci_of(address);
    let ring = Ring::new(dma)?;
    Some(Endpoint {
        ring,
        address,
        kind,
        max_packet,
        bounce: unsafe { dev.out.add(int_bounce(dci)) },
        bounce_phys: dev.out_phys + int_bounce(dci) as u64,
        bounce_len: 64,
        own_bounce: false,
        queued: false,
        user: core::ptr::null_mut(),
        user_len: 0,
    })
}

/// `ep` into the device, its ring known to its completion record (the
/// handler finds the TD's TRBs by it).
fn install_endpoint(s: &Slot, dev: &mut Device, ep: Endpoint) {
    let dci = dci_of(ep.address);
    s.pending[dci].set_ring(&ep.ring);
    dev.eps[dci] = Some(ep);
}

/// A control transfer on `dev`'s endpoint 0 (the core's `control`): bytes
/// moved in the data stage, or an error.
pub fn control(c: &Controller, s: &Slot, dev: &mut Device, request_type: u8, request: u8, value: u16, index: u16, data: *mut u8, len: u16) -> i32 {
    if usize::from(len) > CONTROL_MAX {
        return USB_EIO;
    }
    let slot = dev.slot;
    let bounce = unsafe { dev.input.add(CONTROL_BOUNCE) };
    let bounce_phys = dev.input_phys + CONTROL_BOUNCE as u64;
    let Some(ep) = dev.endpoint_mut(1) else {
        return USB_EGONE;
    };
    let p = &s.pending[1];
    if len != 0 && request_type & 0x80 == 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(data, bounce, usize::from(len));
        }
    }
    dma::clean(c.dma, bounce, usize::from(len).max(1));
    hc::enqueue_control(ep, p, request_type, request, value, index, bounce_phys, len);
    hc::ring_doorbell(c, slot, 1);
    if !hc::wait(c, &p.done, 1000) {
        p.active.store(false, Ordering::Release);
        abort_endpoint(c, slot, ep);
        return USB_ETIMEDOUT;
    }
    p.active.store(false, Ordering::Release);
    let r = p.result();
    if r > 0 && request_type & 0x80 != 0 {
        dma::invalidate(c.dma, bounce, usize::from(len));
        unsafe {
            core::ptr::copy_nonoverlapping(bounce, data, (r as usize).min(usize::from(len)));
        }
    }
    r
}

/// A transfer that did not complete: stop the endpoint and move its
/// dequeue pointer past the TD, so the ring is usable again.
fn abort_endpoint(c: &Controller, slot: u8, ep: &mut Endpoint) {
    let dci = dci_of(ep.address) as u8;
    let _ = hc::command(c, hc::trb_stop_endpoint(slot, dci));
    let _ = hc::command(c, hc::trb_set_tr_dequeue(slot, dci, ep.ring.enqueue_pointer()));
}

/// A bulk transfer (the core's `bulk`): through the controller's bounce
/// buffer, in pieces of its size.
pub fn bulk(c: &Controller, s: &Slot, dev: &mut Device, address: u8, data: *mut u8, len: usize, timeout_ms: u32) -> i32 {
    let slot = dev.slot;
    let to_host = address & 0x80 != 0;
    let Some(ep) = dev.endpoint_by_address(address) else {
        return USB_EIO;
    };
    let p = &s.pending[dci_of(address)];
    let bounce = c.bounce.lock();
    let mut done_bytes = 0usize;
    while done_bytes < len {
        let chunk = (len - done_bytes).min(bounce.len);
        if !to_host {
            unsafe {
                core::ptr::copy_nonoverlapping(data.add(done_bytes), bounce.va, chunk);
            }
        }
        dma::clean(c.dma, bounce.va, chunk);
        if let Err(e) = hc::enqueue_normal(ep, p, bounce.phys, chunk) {
            return e;
        }
        hc::ring_doorbell(c, slot, dci_of(address) as u8);
        if !hc::wait(c, &p.done, timeout_ms.max(1)) {
            p.active.store(false, Ordering::Release);
            abort_endpoint(c, slot, ep);
            return USB_ETIMEDOUT;
        }
        p.active.store(false, Ordering::Release);
        let r = p.result();
        if r < 0 {
            return r;
        }
        let moved = (r as usize).min(chunk);
        if to_host {
            dma::invalidate(c.dma, bounce.va, moved);
            unsafe {
                core::ptr::copy_nonoverlapping(bounce.va, data.add(done_bytes), moved);
            }
        }
        done_bytes += moved;
        if moved < chunk {
            break; // a short packet ends the transfer
        }
    }
    done_bytes as i32
}

/// Queue an interrupt or bulk IN transfer (the core's `transfer_start`):
/// its completion wakes the core, which asks [`transfer_take`]. A bulk
/// endpoint gets a page of its own to bounce through the first time.
pub fn transfer_start(c: &Controller, s: &Slot, dev: &mut Device, address: u8, data: *mut u8, len: usize) -> i32 {
    let slot = dev.slot;
    let Some(ep) = dev.endpoint_by_address(address) else {
        return USB_EIO;
    };
    if address & 0x80 == 0 || !matches!(ep.kind, 2 | 3) {
        return USB_EIO;
    }
    if ep.kind == 2 && !ep.own_bounce {
        let Some((phys, va)) = dma::page(c.dma) else {
            return USB_EIO;
        };
        (ep.bounce, ep.bounce_phys, ep.bounce_len, ep.own_bounce) = (va, phys, USB_ASYNC_BULK_MAX, true);
    }
    if len > ep.bounce_len {
        return USB_EIO;
    }
    let p = &s.pending[dci_of(address)];
    if p.active.load(Ordering::Acquire) {
        return USB_EIO;
    }
    ep.queued = true;
    ep.user = data;
    ep.user_len = len;
    // Known to the handler before the doorbell: a completion wakes the
    // core's thread, not a waiter.
    p.has_callback.store(true, Ordering::Release);
    dma::clean(c.dma, ep.bounce, len);
    if let Err(e) = hc::enqueue_normal(ep, p, ep.bounce_phys, len) {
        p.has_callback.store(false, Ordering::Release);
        ep.queued = false;
        return e;
    }
    hc::ring_doorbell(c, slot, dci_of(address) as u8);
    0
}

/// The queued transfer on `address` (the core's `transfer_take`):
/// `USB_EPENDING` while it runs; else its result, its data copied out and
/// its record released.
pub fn transfer_take(c: &Controller, s: &Slot, dev: &mut Device, address: u8) -> i32 {
    let Some(ep) = dev.endpoint_by_address(address) else {
        return USB_EIO;
    };
    if !ep.queued {
        return USB_EIO;
    }
    let p = &s.pending[dci_of(address)];
    if !p.done.load(Ordering::Acquire) {
        return USB_EPENDING;
    }
    let r = p.result();
    if r > 0 {
        dma::invalidate(c.dma, ep.bounce, ep.user_len);
        unsafe {
            core::ptr::copy_nonoverlapping(ep.bounce, ep.user, (r as usize).min(ep.user_len));
        }
    }
    ep.queued = false;
    p.has_callback.store(false, Ordering::Release);
    p.active.store(false, Ordering::Release);
    r
}

/// The controller's side of recovering a stalled endpoint (the core's
/// `clear_halt`; the core sends the CLEAR_FEATURE).
pub fn clear_halt(c: &Controller, dev: &mut Device, address: u8) -> i32 {
    let slot = dev.slot;
    let Some(ep) = dev.endpoint_by_address(address) else {
        return USB_EIO;
    };
    let dci = dci_of(address) as u8;
    let _ = hc::command(c, hc::trb_reset_endpoint(slot, dci));
    ep.ring.reset();
    let _ = hc::command(c, hc::trb_set_tr_dequeue(slot, dci, ep.ring.enqueue_pointer()));
    0
}

/// A device at `path` (the core's `device_new`): a slot enabled for it,
/// its contexts and endpoint 0's ring; the handle (table index + 1).
pub fn device_new(c: &Controller, path: &UsbDevicePath) -> Result<u32, i32> {
    let ctrl = c.index;
    let tt = if path.tt_hub != 0 {
        Some((peek(ctrl, path.tt_hub, |hub| hub.slot).ok_or(USB_EGONE)?, path.tt_port))
    } else {
        None
    };
    // A free entry.
    let (index, s, mut entry) = (0..MAX_DEVICES)
        .find_map(|i| {
            let s = slot_at(ctrl, i)?;
            let entry = s.dev.lock();
            entry.is_none().then_some((i, s, entry))
        })
        .ok_or(USB_EIO)?;
    let (_, slot) = hc::command(c, hc::trb_enable_slot())?;
    if slot == 0 {
        return Err(USB_EIO);
    }
    // Zeroed and written back by `dma::page`: the output context is the
    // controller's to write from here on, and must not be dirtied again.
    let (input_phys, input) = dma::page(c.dma).ok_or(USB_EIO)?;
    let (out_phys, out) = dma::page(c.dma).ok_or(USB_EIO)?;
    let mut dev = Device {
        slot,
        path: *path,
        psi: psi_from_speed(path.speed),
        tt,
        input,
        input_phys,
        out,
        out_phys,
        eps: [const { None }; MAX_DCI],
    };
    let ep0 = new_endpoint(c.dma, &mut dev, 0, 0, ep0_max_packet(path.speed)).ok_or(USB_EIO)?;
    install_endpoint(s, &mut dev, ep0);
    unsafe {
        core::ptr::write_volatile(c.dcbaa.add(usize::from(slot)), out_phys);
    }
    dma::clean(c.dma, c.dcbaa as *mut u8, 4096);
    c.slot_owner[usize::from(slot)].store(index as u8 + 1, Ordering::Release);
    *entry = Some(dev);
    Ok(index as u32 + 1)
}

/// Address the device (the core's `device_address`): the input context
/// with the slot and endpoint 0, then Address Device (the controller sends
/// the SET_ADDRESS).
pub fn device_address(c: &Controller, dev: &mut Device) -> Result<(), i32> {
    let csz64 = c.csz64;
    let slot = dev.slot;
    dev.clear_input();
    dev.fill_slot_context(csz64, 1);
    {
        let ep0_ring = dev.eps[1].as_ref().ok_or(USB_EIO)?.ring.phys;
        let speed = dev.path.speed;
        let input = dev.input_ctx(csz64);
        input.control_mut().set_add_context_flag(1);
        let e = input.device_mut().endpoint_mut(1);
        e.set_endpoint_type(EndpointType::Control);
        e.set_max_packet_size(ep0_max_packet(speed));
        e.set_error_count(3);
        e.set_tr_dequeue_pointer(ep0_ring);
        e.set_dequeue_cycle_state();
        e.set_average_trb_length(8);
    }
    dma::clean(c.dma, dev.input, INPUT_CTX_BYTES);
    hc::command(c, hc::trb_address_device(dev.input_phys, slot))?;
    Ok(())
}

/// Endpoint 0 takes `mps0` bytes a packet (the core's `device_ep0`):
/// Evaluate Context.
pub fn device_ep0(c: &Controller, dev: &mut Device, mps0: u16) -> Result<(), i32> {
    let csz64 = c.csz64;
    let slot = dev.slot;
    dev.clear_input();
    {
        let input = dev.input_ctx(csz64);
        input.control_mut().set_add_context_flag(1);
        let e = input.device_mut().endpoint_mut(1);
        e.set_endpoint_type(EndpointType::Control);
        e.set_max_packet_size(mps0);
        e.set_error_count(3);
    }
    dma::clean(c.dma, dev.input, INPUT_CTX_BYTES);
    hc::command(c, hc::trb_evaluate_context(dev.input_phys, slot))?;
    if let Some(ep0) = dev.eps[1].as_mut() {
        ep0.max_packet = mps0;
    }
    Ok(())
}

/// The endpoints of the configuration about to be set (the core's
/// `device_configure`): a ring and a context each, Configure Endpoint.
pub fn device_configure(c: &Controller, s: &Slot, dev: &mut Device, eps: &[UsbEndpoint]) -> Result<(), i32> {
    let csz64 = c.csz64;
    let slot = dev.slot;
    let speed = dev.path.speed;
    let mut max_dci = 1u8;
    let mut new_eps: [Option<Endpoint>; MAX_DCI] = [const { None }; MAX_DCI];
    let mut intervals = [0u8; MAX_DCI];
    for e in eps {
        let dci = dci_of(e.address);
        if dci < 2 || dci >= MAX_DCI || new_eps[dci].is_some() {
            continue;
        }
        let kind = e.attributes & 3;
        if kind == 1 {
            continue; // isochronous: not driven
        }
        new_eps[dci] = Some(new_endpoint(c.dma, dev, e.address, kind, e.max_packet & 0x7FF).ok_or(USB_EIO)?);
        intervals[dci] = ep_interval(speed, kind, e.interval);
        max_dci = max_dci.max(dci as u8);
    }
    dev.clear_input();
    dev.fill_slot_context(csz64, max_dci);
    {
        let input = dev.input_ctx(csz64);
        for dci in 2..MAX_DCI {
            let Some(ep) = new_eps[dci].as_ref() else {
                continue;
            };
            input.control_mut().set_add_context_flag(dci);
            let e = input.device_mut().endpoint_mut(dci);
            let to_host = ep.address & 0x80 != 0;
            e.set_endpoint_type(match (ep.kind, to_host) {
                (2, true) => EndpointType::BulkIn,
                (2, false) => EndpointType::BulkOut,
                (3, true) => EndpointType::InterruptIn,
                (3, false) => EndpointType::InterruptOut,
                (_, true) => EndpointType::IsochIn,
                (_, false) => EndpointType::IsochOut,
            });
            e.set_max_packet_size(ep.max_packet);
            e.set_max_burst_size(0);
            e.set_error_count(3);
            e.set_tr_dequeue_pointer(ep.ring.phys);
            e.set_dequeue_cycle_state();
            e.set_interval(intervals[dci]);
            if ep.kind == 3 {
                e.set_max_endpoint_service_time_interval_payload_low(ep.max_packet);
                e.set_average_trb_length(ep.max_packet);
            } else {
                e.set_average_trb_length(3072);
            }
        }
    }
    for dci in 2..MAX_DCI {
        if let Some(ep) = new_eps[dci].take() {
            install_endpoint(s, dev, ep);
        }
    }
    if max_dci > 1 {
        dma::clean(c.dma, dev.input, INPUT_CTX_BYTES);
        hc::command(c, hc::trb_configure_endpoint(dev.input_phys, slot))?;
    }
    Ok(())
}

/// The device is going (the core's `device_abort`): its transfers in
/// flight fail now, waking their waiters, so whoever holds its lock lets
/// go of it.
pub fn device_abort(c: &Controller, hdev: u32) {
    let Some(s) = slot_of(c.index, hdev) else {
        return;
    };
    for p in &s.pending {
        if p.active.load(Ordering::Acquire) {
            p.code.store(0xFF, Ordering::Relaxed);
            p.done.store(true, Ordering::Release);
            crate::api().wake(p.key());
        }
    }
}

/// Forget the device (the core's `device_free`): its slot disabled, its
/// memory back to the pool.
pub fn device_free(c: &Controller, hdev: u32) {
    let Some(s) = slot_of(c.index, hdev) else {
        return;
    };
    let mut entry = s.dev.lock();
    let Some(dev) = entry.take() else {
        return;
    };
    let slot = dev.slot;
    let _ = hc::command(c, hc::trb_disable_slot(slot));
    unsafe {
        core::ptr::write_volatile(c.dcbaa.add(usize::from(slot)), 0);
    }
    dma::clean(c.dma, c.dcbaa as *mut u8, 4096);
    c.slot_owner[usize::from(slot)].store(0, Ordering::Release);
    // The records, clean for the next device in the entry, whatever a
    // transfer that slipped in after `device_abort` left in them.
    for p in &s.pending {
        p.reset();
    }
    for ep in dev.eps.into_iter().flatten() {
        dma::free(c.dma, ep.ring.phys, ep.ring.trbs as *mut u8);
        if ep.own_bounce {
            dma::free(c.dma, ep.bounce_phys, ep.bounce);
        }
    }
    dma::free(c.dma, dev.input_phys, dev.input);
    dma::free(c.dma, dev.out_phys, dev.out);
}

/// A hub's slot context learns its port count (the core's `device_hub`).
pub fn device_hub(c: &Controller, dev: &mut Device, ports: u8, tt_think: u8, multi_tt: bool) -> i32 {
    let csz64 = c.csz64;
    let slot = dev.slot;
    dma::invalidate(c.dma, dev.out, OUT_CTX_BYTES);
    dev.clear_input();
    let high_speed = dev.path.speed == USB_SPEED_HIGH;
    {
        let out_slot: [u32; 8] = {
            let o = dev.out_ctx(csz64);
            let w = o.slot().as_ref();
            let mut a = [0u32; 8];
            a[..w.len().min(8)].copy_from_slice(&w[..w.len().min(8)]);
            a
        };
        let input = dev.input_ctx(csz64);
        input.control_mut().set_add_context_flag(0);
        let s = input.device_mut().slot_mut();
        let w = s.as_mut();
        let n = w.len().min(8);
        w[..n].copy_from_slice(&out_slot[..n]);
        // Context Entries reads back 0 on some controllers: at least EP0.
        if s.context_entries() == 0 {
            s.set_context_entries(1);
        }
        s.set_hub();
        s.set_number_of_ports(ports);
        if high_speed {
            s.set_tt_think_time(tt_think & 3);
            if multi_tt {
                s.set_multi_tt();
            }
        }
        s.set_slot_state(xhci::context::SlotState::DisabledEnabled);
        s.set_usb_device_address(0);
    }
    dma::clean(c.dma, dev.input, INPUT_CTX_BYTES);
    match hc::command(c, hc::trb_configure_endpoint(dev.input_phys, slot)) {
        Ok(_) => 0,
        Err(e) => e,
    }
}
