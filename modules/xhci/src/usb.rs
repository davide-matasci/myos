//! The devices: enumeration (slot, address, descriptors, endpoints),
//! their interfaces offered to the class drivers, hubs' children with their
//! route strings, and detach. All of it runs on the USB thread
//! (`crate::thread_main`), except the transfers class drivers start.

use core::ffi::c_void;
use core::sync::atomic::Ordering;

use myos_abi::{
    USB_EGONE, USB_EIO, USB_ETIMEDOUT, USB_LABEL_MAX, USB_MAX_ENDPOINTS, USB_SPEED_FULL,
    USB_SPEED_HIGH, USB_SPEED_LOW, USB_SPEED_SUPER, UsbDeviceInfo, UsbDriverOps, UsbEndpoint, UsbInterfaceInfo,
};
use xhci::context::{
    Device32Byte, Device64Byte, DeviceHandler, EndpointType, Input32Byte, Input64Byte,
    InputHandler,
};

use crate::hc::{self, Controller, Endpoint, MAX_DCI, MAX_DEVICES, Pending, Ring};
use crate::{api, dma};

/// Interfaces per device the host describes (the rest are ignored).
pub const MAX_INTERFACES: usize = 8;
/// Class drivers that may register.
pub const MAX_DRIVERS: usize = 8;
/// A control transfer's data stage at most (the descriptors).
const CONTROL_MAX: usize = 1024;
/// Hub nesting (route string tiers).
const MAX_DEPTH: u8 = 5;

pub struct Interface {
    pub info: UsbInterfaceInfo,
    /// Index into `DRIVERS` of the driver that took it.
    pub driver: Option<usize>,
    /// The driver's name for what it made of it (`interface_label`).
    pub label: [u8; USB_LABEL_MAX],
    pub label_len: u8,
}

pub struct Device {
    pub slot: u8,
    pub info: UsbDeviceInfo,
    /// The xHCI port speed id (slot context).
    pub psi: u8,
    pub root_port: u8,
    pub route: u32,
    /// The transaction translator for a low/full-speed device behind a
    /// high-speed hub: that hub's slot and the port the device is on.
    pub tt: Option<(u8, u8)>,
    pub hub_ports: u8,
    pub hub_tt_think: u8,
    pub hub_multi_tt: bool,
    /// A page: the input context at 0, control-transfer bounce after it.
    pub input: *mut u8,
    pub input_phys: u64,
    /// A page: the output device context at 0, interrupt bounce slots after it.
    pub out: *mut u8,
    pub out_phys: u64,
    pub eps: [Option<Endpoint>; MAX_DCI],
    pub interfaces: [Option<Interface>; MAX_INTERFACES],
    pub gone: bool,
}

/// The enumeration step that failed last, for the boot log.
pub static mut STEP: &str = "";

fn step(s: &'static str) {
    unsafe {
        *core::ptr::addr_of_mut!(STEP) = s;
    }
}

pub static mut DEVICES: [[Option<Device>; MAX_DEVICES]; hc::MAX_CTRL] =
    [const { [const { None }; MAX_DEVICES] }; hc::MAX_CTRL];
pub static mut DRIVERS: [Option<&'static UsbDriverOps>; MAX_DRIVERS] = [None; MAX_DRIVERS];

const INPUT_CTX_BYTES: usize = 33 * 64;
const OUT_CTX_BYTES: usize = 32 * 64;
const CONTROL_BOUNCE: usize = INPUT_CTX_BYTES;
/// Interrupt bounce slot of DCI `d`: 64 bytes each, after the output context.
fn int_bounce(d: usize) -> usize {
    OUT_CTX_BYTES + d * 64
}

/// The device entry `owner` (`slot_owner` value: table index + 1) of
/// controller `ctrl` names, for the event handler.
pub fn device_at(ctrl: u8, owner: u8) -> Option<&'static mut Device> {
    if owner == 0 {
        return None;
    }
    unsafe { (*core::ptr::addr_of_mut!(DEVICES)).get_mut(usize::from(ctrl))?.get_mut(usize::from(owner) - 1)?.as_mut() }
}

fn table(ctrl: u8) -> &'static mut [Option<Device>; MAX_DEVICES] {
    unsafe { &mut (*core::ptr::addr_of_mut!(DEVICES))[usize::from(ctrl)] }
}

/// A device id: controller in the high byte, table index + 1 below.
pub fn id_of(ctrl: u8, index: usize) -> u32 {
    (u32::from(ctrl) << 8) | (index as u32 + 1)
}

pub fn lookup(id: u32) -> Option<(&'static mut Controller, &'static mut Device)> {
    let ctrl = (id >> 8) as u8;
    let index = (id & 0xFF) as usize;
    if index == 0 {
        return None;
    }
    let c = crate::controller(ctrl)?;
    let d = table(ctrl).get_mut(index - 1)?.as_mut()?;
    if d.gone {
        return None;
    }
    Some((c, d))
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

fn new_endpoint(dev: &mut Device, address: u8, kind: u8, max_packet: u16) -> Option<Endpoint> {
    let dci = dci_of(address);
    let ring = Ring::new()?;
    Some(Endpoint {
        ring,
        pending: Pending::new(),
        lock: crate::sync::Spin::new(),
        address,
        kind,
        max_packet,
        bounce: unsafe { dev.out.add(int_bounce(dci)) },
        bounce_phys: dev.out_phys + int_bounce(dci) as u64,
        bounce_len: 64,
    })
}

/// A control transfer on `dev`'s endpoint 0 (the host's `control`): bytes
/// moved in the data stage, or an error.
pub fn control(c: &mut Controller, dev: &mut Device, request_type: u8, request: u8, value: u16, index: u16, data: *mut u8, len: u16) -> i32 {
    if usize::from(len) > CONTROL_MAX {
        return USB_EIO;
    }
    let slot = dev.slot;
    let bounce = unsafe { dev.input.add(CONTROL_BOUNCE) };
    let bounce_phys = dev.input_phys + CONTROL_BOUNCE as u64;
    let Some(ep) = dev.endpoint_mut(1) else {
        return USB_EGONE;
    };
    let _held = unsafe { &*(&ep.lock as *const crate::sync::Spin) }.lock();
    if len != 0 && request_type & 0x80 == 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(data, bounce, usize::from(len));
        }
    }
    dma::clean(bounce, usize::from(len).max(1));
    hc::enqueue_control(ep, request_type, request, value, index, bounce_phys, len);
    hc::ring_doorbell(c, slot, 1);
    let done = unsafe { &*(&ep.pending.done as *const core::sync::atomic::AtomicBool) };
    if !hc::wait(c, done, 1000) {
        ep.pending.active.store(false, Ordering::Release);
        abort_endpoint(c, slot, ep);
        return USB_ETIMEDOUT;
    }
    ep.pending.active.store(false, Ordering::Release);
    let r = ep.pending.result();
    if r > 0 && request_type & 0x80 != 0 {
        dma::invalidate(bounce, usize::from(len));
        unsafe {
            core::ptr::copy_nonoverlapping(bounce, data, (r as usize).min(usize::from(len)));
        }
    }
    r
}

/// A transfer that did not complete: stop the endpoint and move its
/// dequeue pointer past the TD, so the ring is usable again.
fn abort_endpoint(c: &mut Controller, slot: u8, ep: &mut Endpoint) {
    let dci = dci_of(ep.address) as u8;
    let _ = hc::command(c, hc::trb_stop_endpoint(slot, dci));
    let _ = hc::command(c, hc::trb_set_tr_dequeue(slot, dci, ep.ring.enqueue_pointer()));
}

/// A bulk transfer (the host's `bulk`): through the controller's bounce
/// buffer, in pieces of its size.
pub fn bulk(c: &mut Controller, dev: &mut Device, address: u8, data: *mut u8, len: usize, timeout_ms: u32) -> i32 {
    let slot = dev.slot;
    let to_host = address & 0x80 != 0;
    let Some(ep) = dev.endpoint_by_address(address) else {
        return USB_EIO;
    };
    let _held = unsafe { &*(&ep.lock as *const crate::sync::Spin) }.lock();
    let _bounce = unsafe { &*(&c.bounce_lock as *const crate::sync::Spin) }.lock();
    let mut done_bytes = 0usize;
    while done_bytes < len {
        let chunk = (len - done_bytes).min(c.bounce_len);
        if !to_host {
            unsafe {
                core::ptr::copy_nonoverlapping(data.add(done_bytes), c.bounce, chunk);
            }
        }
        dma::clean(c.bounce, chunk);
        if let Err(e) = hc::enqueue_normal(ep, c.bounce_phys, chunk) {
            return e;
        }
        hc::ring_doorbell(c, slot, dci_of(address) as u8);
        let done = unsafe { &*(&ep.pending.done as *const core::sync::atomic::AtomicBool) };
        if !hc::wait(c, done, timeout_ms.max(1)) {
            ep.pending.active.store(false, Ordering::Release);
            abort_endpoint(c, slot, ep);
            return USB_ETIMEDOUT;
        }
        ep.pending.active.store(false, Ordering::Release);
        let r = ep.pending.result();
        if r < 0 {
            return r;
        }
        let moved = (r as usize).min(chunk);
        if to_host {
            dma::invalidate(c.bounce, moved);
            unsafe {
                core::ptr::copy_nonoverlapping(c.bounce, data.add(done_bytes), moved);
            }
        }
        done_bytes += moved;
        if moved < chunk {
            break; // a short packet ends the transfer
        }
    }
    done_bytes as i32
}

/// Queue an interrupt IN transfer (the host's `interrupt_start`).
pub fn interrupt_start(c: &mut Controller, dev: &mut Device, address: u8, data: *mut u8, len: usize, done: myos_abi::UsbCompletion, ctx: *mut c_void) -> i32 {
    let slot = dev.slot;
    let Some(ep) = dev.endpoint_by_address(address) else {
        return USB_EIO;
    };
    if len > ep.bounce_len {
        return USB_EIO;
    }
    let _held = unsafe { &*(&ep.lock as *const crate::sync::Spin) }.lock();
    if ep.pending.active.load(Ordering::Acquire) {
        return USB_EIO;
    }
    ep.pending.callback = Some((done, ctx));
    ep.pending.user = data;
    ep.pending.user_len = len;
    if let Err(e) = hc::enqueue_normal(ep, ep.bounce_phys, len) {
        return e;
    }
    hc::ring_doorbell(c, slot, dci_of(address) as u8);
    0
}

/// Completed asynchronous transfers: copy their data out and run their
/// callbacks (the USB thread).
pub fn run_async_completions(c: &mut Controller) {
    let ctrl = c.index;
    for i in 0..MAX_DEVICES {
        let Some(dev) = table(ctrl)[i].as_mut() else {
            continue;
        };
        for ep in dev.eps.iter_mut().flatten() {
            let p = &mut ep.pending;
            if p.callback.is_none() || !p.done.load(Ordering::Acquire) || !p.active.load(Ordering::Acquire) {
                continue;
            }
            let r = p.result();
            if r > 0 {
                dma::invalidate(ep.bounce, p.user_len);
                unsafe {
                    core::ptr::copy_nonoverlapping(ep.bounce, p.user, (r as usize).min(p.user_len));
                }
            }
            let (f, ctx) = p.callback.take().unwrap();
            p.active.store(false, Ordering::Release);
            unsafe { f(ctx, r) };
        }
    }
}

/// Recover a stalled endpoint (the host's `clear_halt`).
pub fn clear_halt(c: &mut Controller, dev: &mut Device, address: u8) -> i32 {
    let slot = dev.slot;
    let Some(ep) = dev.endpoint_by_address(address) else {
        return USB_EIO;
    };
    let dci = dci_of(address) as u8;
    let _ = hc::command(c, hc::trb_reset_endpoint(slot, dci));
    ep.ring.reset();
    let _ = hc::command(c, hc::trb_set_tr_dequeue(slot, dci, ep.ring.enqueue_pointer()));
    // CLEAR_FEATURE(ENDPOINT_HALT) on the endpoint.
    let r = control(c, dev, 0x02, 1, 0, u16::from(address), core::ptr::null_mut(), 0);
    if r < 0 { r } else { 0 }
}

/// Enumerate the device on `root_port` (1-based) of controller `c`, or on
/// `port` of hub `parent`, at `speed`: the new device's id.
pub fn attach(c: &mut Controller, parent: Option<(u32, u8)>, root_port: u8, speed: u8) -> Result<u32, i32> {
    let ctrl = c.index;
    let (depth, route, tt, parent_id, hub_port, root_port) = match parent {
        None => (0u8, 0u32, None, 0u32, 0u8, root_port),
        Some((hub_id, port)) => {
            let (_, hub) = lookup(hub_id).ok_or(USB_EGONE)?;
            if hub.info.depth >= MAX_DEPTH || port == 0 || port > 15 {
                return Err(USB_EIO);
            }
            let route = hub.route | (u32::from(port) << (4 * hub.info.depth));
            let tt = if hub.info.speed == USB_SPEED_HIGH && speed != USB_SPEED_HIGH {
                Some((hub.slot, port))
            } else {
                hub.tt
            };
            (hub.info.depth + 1, route, tt, hub_id, port, hub.root_port)
        }
    };
    let index = table(ctrl).iter().position(|d| d.is_none()).ok_or(USB_EIO)?;
    step("enable slot");
    let (_, slot) = hc::command(c, hc::trb_enable_slot())?;
    if slot == 0 {
        return Err(USB_EIO);
    }
    let (input_phys, input) = dma::page().ok_or(USB_EIO)?;
    let (out_phys, out) = dma::page().ok_or(USB_EIO)?;
    unsafe {
        core::ptr::write_bytes(input, 0, 4096);
        core::ptr::write_bytes(out, 0, 4096);
    }
    let mut dev = Device {
        slot,
        info: UsbDeviceInfo {
            id: id_of(ctrl, index),
            parent: parent_id,
            speed,
            port: if parent.is_some() { hub_port } else { root_port },
            depth,
            ..UsbDeviceInfo::default()
        },
        psi: psi_from_speed(speed),
        root_port,
        route,
        tt,
        hub_ports: 0,
        hub_tt_think: 0,
        hub_multi_tt: false,
        input,
        input_phys,
        out,
        out_phys,
        eps: [const { None }; MAX_DCI],
        interfaces: [const { None }; MAX_INTERFACES],
        gone: false,
    };
    let ep0 = new_endpoint(&mut dev, 0, 0, ep0_max_packet(speed)).ok_or(USB_EIO)?;
    dev.eps[1] = Some(ep0);
    unsafe {
        core::ptr::write_volatile(c.dcbaa.add(usize::from(slot)), out_phys);
    }
    dma::clean(c.dcbaa as *mut u8, 4096);
    c.slot_owner[usize::from(slot)] = index as u8 + 1;
    table(ctrl)[index] = Some(dev);
    let dev = table(ctrl)[index].as_mut().unwrap();

    match enumerate(c, dev) {
        Ok(()) => {
            crate::proc_update();
            Ok(dev.info.id)
        }
        Err(e) => {
            let id = dev.info.id;
            detach_id(c, id);
            Err(e)
        }
    }
}

/// Address the device, read its descriptors, configure its endpoints and
/// offer its interfaces.
fn enumerate(c: &mut Controller, dev: &mut Device) -> Result<(), i32> {
    let csz64 = c.csz64;
    let slot = dev.slot;
    // Input context: slot + endpoint 0.
    dev.clear_input();
    {
        let ep0_ring = dev.eps[1].as_ref().unwrap().ring.phys;
        let (psi, route, root_port, tt, speed) = (dev.psi, dev.route, dev.root_port, dev.tt, dev.info.speed);
        let input = dev.input_ctx(csz64);
        input.control_mut().set_add_context_flag(0);
        input.control_mut().set_add_context_flag(1);
        let s = input.device_mut().slot_mut();
        s.set_route_string(route);
        s.set_speed(psi);
        s.set_context_entries(1);
        s.set_root_hub_port_number(root_port);
        if let Some((hub_slot, port)) = tt {
            s.set_parent_hub_slot_id(hub_slot);
            s.set_parent_port_number(port);
        }
        let e = input.device_mut().endpoint_mut(1);
        e.set_endpoint_type(EndpointType::Control);
        e.set_max_packet_size(ep0_max_packet(speed));
        e.set_error_count(3);
        e.set_tr_dequeue_pointer(ep0_ring);
        e.set_dequeue_cycle_state();
        e.set_average_trb_length(8);
    }
    dma::clean(dev.input, INPUT_CTX_BYTES);
    step("address device");
    hc::command(c, hc::trb_address_device(dev.input_phys, slot))?;
    step("device descriptor");

    // The device descriptor: 8 bytes first, for the control endpoint's
    // real max packet size (full speed may use 8, 16, 32 or 64).
    let mut desc = [0u8; 18];
    let n = control(c, dev, 0x80, 6, 0x0100, 0, desc.as_mut_ptr(), 8);
    if n < 8 {
        return Err(if n < 0 { n } else { USB_EIO });
    }
    let mps0 = match dev.info.speed {
        USB_SPEED_SUPER => 1u16 << desc[7].min(9),
        _ => u16::from(desc[7]),
    };
    if mps0 != ep0_max_packet(dev.info.speed) && mps0 >= 8 {
        dev.clear_input();
        {
            let input = dev.input_ctx(csz64);
            input.control_mut().set_add_context_flag(1);
            let e = input.device_mut().endpoint_mut(1);
            e.set_endpoint_type(EndpointType::Control);
            e.set_max_packet_size(mps0);
            e.set_error_count(3);
        }
        dma::clean(dev.input, INPUT_CTX_BYTES);
        step("evaluate context");
        hc::command(c, hc::trb_evaluate_context(dev.input_phys, slot))?;
        step("device descriptor");
        if let Some(ep0) = dev.eps[1].as_mut() {
            ep0.max_packet = mps0;
        }
    }
    let n = control(c, dev, 0x80, 6, 0x0100, 0, desc.as_mut_ptr(), 18);
    if n < 18 {
        return Err(if n < 0 { n } else { USB_EIO });
    }
    dev.info.vendor = u16::from_le_bytes([desc[8], desc[9]]);
    dev.info.product = u16::from_le_bytes([desc[10], desc[11]]);
    dev.info.class = desc[4];
    dev.info.subclass = desc[5];
    dev.info.protocol = desc[6];

    // The first configuration.
    step("configuration descriptor");
    let mut cfg = [0u8; CONTROL_MAX];
    let n = control(c, dev, 0x80, 6, 0x0200, 0, cfg.as_mut_ptr(), 9);
    if n < 9 {
        return Err(if n < 0 { n } else { USB_EIO });
    }
    let total = usize::from(u16::from_le_bytes([cfg[2], cfg[3]])).min(CONTROL_MAX);
    let n = control(c, dev, 0x80, 6, 0x0200, 0, cfg.as_mut_ptr(), total as u16);
    if n < total as i32 {
        return Err(if n < 0 { n } else { USB_EIO });
    }
    dev.info.config = cfg[5];
    parse_config(dev, &cfg[..total]);

    // Endpoint contexts for every endpoint of every interface.
    dev.clear_input();
    let mut max_dci = 1u8;
    let speed = dev.info.speed;
    let mut new_eps: [Option<Endpoint>; MAX_DCI] = [const { None }; MAX_DCI];
    for i in 0..MAX_INTERFACES {
        let Some(intf) = dev.interfaces[i].as_ref() else {
            continue;
        };
        let info = intf.info;
        for e in &info.endpoints[..usize::from(info.n_endpoints)] {
            let dci = dci_of(e.address);
            if dci >= MAX_DCI || new_eps[dci].is_some() {
                continue;
            }
            let kind = e.attributes & 3;
            if kind == 1 {
                continue; // isochronous: not driven
            }
            new_eps[dci] = Some(new_endpoint(dev, e.address, kind, e.max_packet & 0x7FF).ok_or(USB_EIO)?);
            max_dci = max_dci.max(dci as u8);
        }
    }
    let mut intervals = [0u8; MAX_DCI];
    for dci in 2..MAX_DCI {
        if let Some(ep) = new_eps[dci].as_ref() {
            intervals[dci] = interval_of(dev, ep.address, speed, ep.kind);
        }
    }
    {
        let (route, psi, root_port, tt) = (dev.route, dev.psi, dev.root_port, dev.tt);
        let input = dev.input_ctx(csz64);
        input.control_mut().set_add_context_flag(0);
        let s = input.device_mut().slot_mut();
        s.set_route_string(route);
        s.set_speed(psi);
        s.set_context_entries(max_dci);
        s.set_root_hub_port_number(root_port);
        if let Some((hub_slot, port)) = tt {
            s.set_parent_hub_slot_id(hub_slot);
            s.set_parent_port_number(port);
        }
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
            dev.eps[dci] = Some(ep);
        }
    }
    if max_dci > 1 {
        dma::clean(dev.input, INPUT_CTX_BYTES);
        step("configure endpoint");
        hc::command(c, hc::trb_configure_endpoint(dev.input_phys, slot))?;
    }
    // SET_CONFIGURATION.
    step("set configuration");
    let config = dev.info.config;
    let r = control(c, dev, 0x00, 9, u16::from(config), 0, core::ptr::null_mut(), 0);
    if r < 0 {
        return Err(r);
    }
    offer(c, dev);
    Ok(())
}

fn interval_of(dev: &Device, address: u8, speed: u8, kind: u8) -> u8 {
    for intf in dev.interfaces.iter().flatten() {
        for e in &intf.info.endpoints[..usize::from(intf.info.n_endpoints)] {
            if e.address == address {
                return ep_interval(speed, kind, e.interval);
            }
        }
    }
    0
}

/// The interfaces and endpoints of a configuration descriptor (alternate
/// setting 0 only).
fn parse_config(dev: &mut Device, cfg: &[u8]) {
    let mut i = 0usize;
    let mut cur: Option<usize> = None;
    let mut n_if = 0u8;
    while i + 2 <= cfg.len() {
        let len = usize::from(cfg[i]);
        if len < 2 || i + len > cfg.len() {
            break;
        }
        let d = &cfg[i..i + len];
        match d[1] {
            4 if len >= 9 => {
                cur = None;
                if d[3] != 0 {
                    // another alternate setting: skip its endpoints
                } else if let Some(slot) = dev.interfaces.iter().position(|x| x.is_none()) {
                    dev.interfaces[slot] = Some(Interface {
                        info: UsbInterfaceInfo {
                            number: d[2],
                            class: d[5],
                            subclass: d[6],
                            protocol: d[7],
                            n_endpoints: 0,
                            endpoints: [UsbEndpoint::default(); USB_MAX_ENDPOINTS],
                        },
                        driver: None,
                        label: [0; USB_LABEL_MAX],
                        label_len: 0,
                    });
                    cur = Some(slot);
                    n_if += 1;
                }
            }
            5 if len >= 7 => {
                if let Some(slot) = cur {
                    let intf = dev.interfaces[slot].as_mut().unwrap();
                    let n = usize::from(intf.info.n_endpoints);
                    if n < USB_MAX_ENDPOINTS {
                        intf.info.endpoints[n] = UsbEndpoint {
                            address: d[2],
                            attributes: d[3],
                            max_packet: u16::from_le_bytes([d[4], d[5]]),
                            interval: d[6],
                        };
                        intf.info.n_endpoints += 1;
                    }
                }
            }
            _ => {}
        }
        i += len;
    }
    dev.info.n_interfaces = n_if;
}

/// Offer every unclaimed interface of `dev` to the drivers.
pub fn offer(c: &mut Controller, dev: &mut Device) {
    let _ = c;
    for i in 0..MAX_INTERFACES {
        let Some(intf) = dev.interfaces[i].as_ref() else {
            continue;
        };
        if intf.driver.is_some() {
            continue;
        }
        let info = intf.info;
        let dinfo = dev.info;
        for (k, drv) in unsafe { (*core::ptr::addr_of!(DRIVERS)).iter().enumerate() } {
            let Some(drv) = drv else {
                continue;
            };
            if unsafe { (drv.probe)(&dinfo, &info) } == 0 {
                if let Some(intf) = dev.interfaces[i].as_mut() {
                    intf.driver = Some(k);
                }
                break;
            }
        }
    }
}

/// Every unclaimed interface of every device, to the drivers (a driver
/// registered after the devices were enumerated).
pub fn offer_all(c: &mut Controller) {
    let ctrl = c.index;
    for i in 0..MAX_DEVICES {
        if let Some(dev) = table(ctrl)[i].as_mut() {
            if !dev.gone {
                offer(c, dev);
            }
        }
    }
    crate::proc_update();
}

/// The device on `port` of `parent` (0: a root port).
pub fn find(ctrl: u8, parent: u32, port: u8) -> Option<u32> {
    table(ctrl)
        .iter()
        .flatten()
        .find(|d| d.info.parent == parent && d.info.port == port && !d.gone)
        .map(|d| d.info.id)
}

/// Tear `id` down: its children first (a hub), its drivers told, its
/// transfers failed, its slot disabled, its memory back to the pool.
pub fn detach_id(c: &mut Controller, id: u32) {
    let ctrl = c.index;
    let index = (id & 0xFF) as usize;
    if index == 0 {
        return;
    }
    // Children.
    loop {
        let child = table(ctrl)
            .iter()
            .flatten()
            .find(|d| d.info.parent == id && !d.gone)
            .map(|d| d.info.id);
        match child {
            Some(cid) => detach_id(c, cid),
            None => break,
        }
    }
    let Some(dev) = table(ctrl)[index - 1].as_mut() else {
        return;
    };
    dev.gone = true;
    // Waiters and asynchronous transfers fail now.
    for ep in dev.eps.iter_mut().flatten() {
        if ep.pending.active.load(Ordering::Acquire) {
            ep.pending.code.store(0xFF, Ordering::Relaxed);
            ep.pending.done.store(true, Ordering::Release);
            unsafe { (api().wake)(ep.pending.key()) };
        }
    }
    for i in 0..MAX_INTERFACES {
        let Some(intf) = dev.interfaces[i].as_ref() else {
            continue;
        };
        if let Some(k) = intf.driver {
            if let Some(drv) = unsafe { (*core::ptr::addr_of!(DRIVERS))[k] } {
                unsafe { (drv.disconnect)(id, intf.info.number) };
            }
        }
    }
    let slot = dev.slot;
    let _ = hc::command(c, hc::trb_disable_slot(slot));
    unsafe {
        core::ptr::write_volatile(c.dcbaa.add(usize::from(slot)), 0);
    }
    dma::clean(c.dcbaa as *mut u8, 4096);
    c.slot_owner[usize::from(slot)] = 0;
    let dev = table(ctrl)[index - 1].take().unwrap();
    for ep in dev.eps.into_iter().flatten() {
        dma::free(ep.ring.phys, ep.ring.trbs as *mut u8);
    }
    dma::free(dev.input_phys, dev.input);
    dma::free(dev.out_phys, dev.out);
    crate::proc_update();
}

/// A hub's slot context learns its port count (the host's `hub_configure`).
pub fn hub_configure(c: &mut Controller, dev: &mut Device, ports: u8, tt_think: u8, multi_tt: bool) -> i32 {
    let csz64 = c.csz64;
    let slot = dev.slot;
    dev.hub_ports = ports;
    dev.hub_tt_think = tt_think;
    dev.hub_multi_tt = multi_tt;
    dma::invalidate(dev.out, OUT_CTX_BYTES);
    dev.clear_input();
    let high_speed = dev.info.speed == USB_SPEED_HIGH;
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
    dma::clean(dev.input, INPUT_CTX_BYTES);
    match hc::command(c, hc::trb_configure_endpoint(dev.input_phys, slot)) {
        Ok(_) => 0,
        Err(e) => e,
    }
}

/// `/proc/usb`: one line per device.
pub fn proc_text(out: &mut [u8]) -> usize {
    let mut w = crate::Writer { buf: out, len: 0 };
    for ctrl in 0..hc::MAX_CTRL as u8 {
        let Some(c) = crate::controller(ctrl) else {
            continue;
        };
        w.str("controller ");
        w.dec(u32::from(ctrl));
        w.str(" ports ");
        w.dec(u32::from(c.ports));
        w.str(if c.irq_on { " irq on" } else { " irq off" });
        w.str(" events ");
        w.dec(c.events.load(Ordering::Relaxed));
        w.str(" irqs ");
        w.dec(crate::irqs());
        w.str("\n");
        for dev in table(ctrl).iter().flatten() {
            let d = &dev.info;
            w.hex(d.id);
            w.str(" parent ");
            w.hex(d.parent);
            w.str(" port ");
            w.dec(u32::from(d.port));
            w.str(" ");
            w.str(match d.speed {
                USB_SPEED_LOW => "low",
                USB_SPEED_FULL => "full",
                USB_SPEED_HIGH => "high",
                _ => "super",
            });
            w.str(" ");
            w.hex4(d.vendor);
            w.str(":");
            w.hex4(d.product);
            w.str(" class ");
            w.hex2(d.class);
            for intf in dev.interfaces.iter().flatten() {
                w.str(" if");
                w.dec(u32::from(intf.info.number));
                w.str("=");
                w.hex2(intf.info.class);
                w.str("/");
                w.hex2(intf.info.subclass);
                w.str("/");
                w.hex2(intf.info.protocol);
                if let Some(k) = intf.driver {
                    if let Some(drv) = unsafe { (*core::ptr::addr_of!(DRIVERS))[k] } {
                        w.str(":");
                        w.bytes(unsafe { core::slice::from_raw_parts(drv.name.ptr, drv.name.len) });
                    }
                }
                if intf.label_len != 0 {
                    w.str(" ");
                    w.bytes(&intf.label[..usize::from(intf.label_len)]);
                }
            }
            if dev.hub_ports != 0 {
                w.str(" hub ");
                w.dec(u32::from(dev.hub_ports));
            }
            if dev.gone {
                w.str(" gone");
            }
            w.str("\n");
        }
    }
    w.len
}
