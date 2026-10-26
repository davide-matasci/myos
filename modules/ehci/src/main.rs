//! EHCI: a USB host controller driver module (`docs/usb.md`) for USB 2.0
//! controllers on PCI (class 0C/03, interface 0x20).
//!
//! Brings each controller up (`hc`) and registers it with the USB core
//! (the `usb` module's [`USB_CORE_SERVICE`]), which owns the devices and
//! their enumeration: this module does the controller's part through the
//! [`UsbHcdOps`] it hands the core. An EHCI talks to high-speed devices
//! only: a full- or low-speed device on one of its ports is handed to the
//! port's companion controller (UHCI or OHCI, the `uhci` module), and one
//! behind a high-speed hub is reached through the hub's transaction
//! translator (split transactions, the QH's hub and port). Isochronous
//! transfers are not driven (issue #622).

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

mod hc;

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use myos_abi::{
    ABI_VERSION, ApiCell, DmaDevice, KernelApi, Lock, StrRef, USB_CORE_SERVICE, USB_CORE_VERSION, USB_EGONE, USB_EIO,
    USB_EPENDING, USB_HCD_VERSION, USB_SPEED_HIGH, UsbCoreOps, UsbDevicePath, UsbEndpoint, UsbHcdOps,
    UsbPortStatus, status_fail, status_ok,
};

use hc::{Controller, PID_IN, PID_OUT, PID_SETUP, Pipe, Target};

/// A control transfer's data stage at most (the descriptors), in the
/// endpoint's data page after the setup packet.
const CONTROL_MAX: usize = 1024;
const CONTROL_DATA: usize = 64;
/// How long a control transfer may take.
const CONTROL_TIMEOUT_MS: u64 = 2000;
/// Devices one controller drives at once.
const MAX_DEVICES: usize = 32;
/// Endpoints of a device besides endpoint 0.
const MAX_ENDPOINTS: usize = 15;

pub static API: ApiCell = ApiCell::new();
static CORE: AtomicPtr<UsbCoreOps> = AtomicPtr::new(core::ptr::null_mut());
static CONTROLLERS: [Lock<Option<&'static Controller>>; hc::MAX_CTRL] = [const { Lock::new(None) }; hc::MAX_CTRL];
/// The controllers' memory: set once each, never freed.
static mut CELLS: [Option<Controller>; hc::MAX_CTRL] = [const { None }; hc::MAX_CTRL];

pub fn api() -> &'static KernelApi {
    API.get()
}

fn core() -> &'static UsbCoreOps {
    // SAFETY: the core's service table, which outlives this module.
    unsafe { CORE.load(Ordering::Acquire).as_ref() }.expect("ehci: usb core")
}

fn controller(ctrl: usize) -> Option<&'static Controller> {
    *CONTROLLERS.get(ctrl)?.lock()
}

pub fn sleep_ms(ms: u64) {
    let until = api().monotonic_ns() + ms * 1_000_000;
    api().task_sleep_until(until);
}

/// A completion the core's thread takes (a queued transfer).
pub fn core_wake(c: &Controller) {
    let index = c.core_index.load(Ordering::Acquire);
    if index != u32::MAX {
        core().wake(index);
    }
}

/// Root port `port` changed.
pub fn core_port_change(c: &Controller, port: u8) {
    let index = c.core_index.load(Ordering::Acquire);
    if index != u32::MAX {
        core().port_change(index, port);
    }
}

/// A small text writer for the log and `describe`.
pub struct Writer<'a> {
    pub buf: &'a mut [u8],
    pub len: usize,
}

impl Writer<'_> {
    pub fn str(&mut self, s: &str) {
        let b = s.as_bytes();
        let n = b.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&b[..n]);
        self.len += n;
    }
    pub fn dec(&mut self, mut v: u32) {
        let mut t = [0u8; 10];
        let mut n = 0;
        loop {
            t[n] = b'0' + (v % 10) as u8;
            n += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        t[..n].reverse();
        let mut s = [0u8; 10];
        s[..n].copy_from_slice(&t[..n]);
        self.str(core::str::from_utf8(&s[..n]).unwrap_or(""));
    }
}

// ---- The devices -----------------------------------------------------------

#[derive(Clone, Copy)]
struct Endpoint {
    address: u8,
    /// Transfer type (`bmAttributes & 3`): 2 bulk, 3 interrupt.
    kind: u8,
    max_packet: u16,
    pipe: Pipe,
    /// A queued transfer (`transfer_start`): the core's buffer and length.
    queued: Option<(usize, usize)>,
}

#[derive(Clone, Copy)]
struct Device {
    /// The address on the bus: 0 until `device_address`.
    address: u8,
    speed: u8,
    /// Below high speed behind a high-speed hub: that hub's address and
    /// port (split transactions).
    tt: Option<(u8, u8)>,
    ep0_max_packet: u16,
    ep0: Pipe,
    eps: [Option<Endpoint>; MAX_ENDPOINTS],
}

struct Slot {
    /// Raised by `device_abort`: a transfer finding it set gives up.
    gone: AtomicBool,
    dev: Lock<Option<Device>>,
}

static DEVICES: [[Slot; MAX_DEVICES]; hc::MAX_CTRL] =
    [const { [const { Slot { gone: AtomicBool::new(false), dev: Lock::new(None) } }; MAX_DEVICES] }; hc::MAX_CTRL];

fn slot(ctrl: usize, hdev: u32) -> Option<&'static Slot> {
    if hdev == 0 {
        return None;
    }
    DEVICES.get(ctrl)?.get(hdev as usize - 1)
}

fn get(ctrl: usize, hdev: u32) -> Result<Device, i32> {
    let s = slot(ctrl, hdev).ok_or(USB_EGONE)?;
    if s.gone.load(Ordering::Acquire) {
        return Err(USB_EGONE);
    }
    s.dev.lock().ok_or(USB_EGONE)
}

fn update<R>(ctrl: usize, hdev: u32, f: impl FnOnce(&mut Device) -> R) -> Result<R, i32> {
    let s = slot(ctrl, hdev).ok_or(USB_EGONE)?;
    let mut d = s.dev.lock();
    let d = d.as_mut().ok_or(USB_EGONE)?;
    Ok(f(d))
}

fn ep0_target(d: &Device) -> Target {
    Target {
        address: d.address,
        endpoint: 0,
        speed: d.speed,
        max_packet: d.ep0_max_packet,
        control: true,
        tt: d.tt,
        periodic: false,
    }
}

fn endpoint_target(d: &Device, e: &UsbEndpoint) -> Target {
    Target {
        address: d.address,
        endpoint: e.address & 0xF,
        speed: d.speed,
        max_packet: e.max_packet & 0x7FF,
        control: false,
        tt: d.tt,
        periodic: e.attributes & 3 == 3,
    }
}

/// The endpoints of `eps` this controller drives (bulk and interrupt),
/// each with its QH, into `d`.
fn add_endpoints(c: &Controller, d: &mut Device, eps: &[UsbEndpoint]) -> Result<(), i32> {
    for e in eps.iter().filter(|e| matches!(e.attributes & 3, 2 | 3)) {
        if d.eps.iter().flatten().any(|x| x.address == e.address) {
            continue;
        }
        let t = endpoint_target(d, e);
        let free = d.eps.iter().position(|x| x.is_none()).ok_or(USB_EIO)?;
        // An IN endpoint's queued transfers land in a page of its own.
        let pipe = hc::pipe_new(c, &t, e.address & 0x80 != 0).ok_or(USB_EIO)?;
        d.eps[free] = Some(Endpoint { address: e.address, kind: e.attributes & 3, max_packet: t.max_packet, pipe, queued: None });
    }
    Ok(())
}

// ---- The controller driver's table ---------------------------------------------

static HCD_OPS: UsbHcdOps = UsbHcdOps {
    version: USB_HCD_VERSION,
    name: StrRef { ptr: b"ehci".as_ptr(), len: 4 },
    ports,
    port_status,
    port_reset,
    poll,
    describe,
    device_new,
    device_address,
    device_ep0,
    device_configure,
    device_hub,
    device_abort,
    device_free,
    control,
    bulk,
    transfer_start,
    transfer_take,
    clear_halt,
    endpoints_change,
    isoch_queue,
    isoch_reap,
};

unsafe extern "C" fn ports(ctrl: usize) -> u8 {
    controller(ctrl).map_or(0, |c| c.ports)
}

unsafe extern "C" fn port_status(ctrl: usize, port: u8, out: *mut UsbPortStatus) -> i32 {
    let Some(c) = controller(ctrl) else {
        return USB_EGONE;
    };
    if out.is_null() || port >= c.ports {
        return USB_EIO;
    }
    let s = hc::port_status(c, port);
    unsafe {
        *out = UsbPortStatus { connected: u8::from(s.connected), enabled: u8::from(s.enabled), speed: s.speed };
    }
    0
}

unsafe extern "C" fn port_reset(ctrl: usize, port: u8) -> i32 {
    let Some(c) = controller(ctrl) else {
        return USB_EGONE;
    };
    if port >= c.ports {
        return USB_EIO;
    }
    if hc::port_reset(c, port) { 0 } else { USB_EIO }
}

/// Every pass of the core's thread: without an interrupt the controller's
/// status is taken here. Nonzero then, so the core comes back.
unsafe extern "C" fn poll(ctrl: usize) -> i32 {
    let Some(c) = controller(ctrl) else {
        return 0;
    };
    let polled = !c.irq_on.load(Ordering::Relaxed);
    if polled {
        hc::process(c);
    }
    i32::from(polled)
}

unsafe extern "C" fn describe(ctrl: usize, buf: *mut u8, len: usize) -> usize {
    let Some(c) = controller(ctrl) else {
        return 0;
    };
    if buf.is_null() {
        return 0;
    }
    let buf = unsafe { core::slice::from_raw_parts_mut(buf, len) };
    let mut w = Writer { buf, len: 0 };
    w.str("ehci ");
    w.str(if c.irq_on.load(Ordering::Relaxed) { "irq on" } else { "irq off" });
    w.str(" events ");
    w.dec(c.events.load(Ordering::Relaxed));
    w.str(" irqs ");
    w.dec(c.irqs.load(Ordering::Relaxed));
    w.len
}

unsafe extern "C" fn device_new(ctrl: usize, path: *const UsbDevicePath) -> i32 {
    let Some(c) = controller(ctrl) else {
        return USB_EGONE;
    };
    if path.is_null() {
        return USB_EIO;
    }
    let path = unsafe { &*path };
    let tt = if path.tt_hub != 0 && path.speed != USB_SPEED_HIGH {
        match get(ctrl, path.tt_hub) {
            Ok(hub) => Some((hub.address, path.tt_port)),
            Err(e) => return e,
        }
    } else {
        None
    };
    let mps0 = match path.speed {
        USB_SPEED_HIGH => 64,
        _ => 8,
    };
    let Some(table) = DEVICES.get(ctrl) else {
        return USB_EIO;
    };
    for (i, s) in table.iter().enumerate() {
        let mut entry = s.dev.lock();
        if entry.is_some() {
            continue;
        }
        let mut d = Device { address: 0, speed: path.speed, tt, ep0_max_packet: mps0, ep0: unsafe { core::mem::zeroed() }, eps: [None; MAX_ENDPOINTS] };
        let Some(ep0) = hc::pipe_new(c, &ep0_target(&d), true) else {
            return USB_EIO;
        };
        d.ep0 = ep0;
        *entry = Some(d);
        s.gone.store(false, Ordering::Release);
        return i as i32 + 1;
    }
    USB_EIO
}

/// SET_ADDRESS to the device, still at address 0, then its QHs say so.
unsafe extern "C" fn device_address(ctrl: usize, hdev: u32) -> i32 {
    let Some(c) = controller(ctrl) else {
        return USB_EGONE;
    };
    let d = match get(ctrl, hdev) {
        Ok(d) => d,
        Err(e) => return e,
    };
    // The handle is the address: unique on this controller's bus.
    let address = hdev as u8;
    let r = control_transfer(c, ctrl, hdev, &d, 0x00, 5, u16::from(address), 0, core::ptr::null_mut(), 0);
    if r < 0 {
        return r;
    }
    let _ = update(ctrl, hdev, |d| {
        d.address = address;
        hc::pipe_retarget(&d.ep0, &ep0_target(d));
    });
    // The device has 2 ms to take its new address.
    sleep_ms(2);
    0
}

unsafe extern "C" fn device_ep0(ctrl: usize, hdev: u32, max_packet: u16) -> i32 {
    match update(ctrl, hdev, |d| {
        d.ep0_max_packet = max_packet;
        hc::pipe_retarget(&d.ep0, &ep0_target(d));
    }) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// The endpoints of the configuration about to be set: a QH each (the old
/// configuration's gone).
unsafe extern "C" fn device_configure(ctrl: usize, hdev: u32, eps: *const UsbEndpoint, n: usize) -> i32 {
    let Some(c) = controller(ctrl) else {
        return USB_EGONE;
    };
    if n != 0 && eps.is_null() {
        return USB_EIO;
    }
    let eps: &[UsbEndpoint] = if n == 0 { &[] } else { unsafe { core::slice::from_raw_parts(eps, n) } };
    let Some(s) = slot(ctrl, hdev) else {
        return USB_EGONE;
    };
    let mut entry = s.dev.lock();
    let Some(d) = entry.as_mut() else {
        return USB_EGONE;
    };
    for e in d.eps.iter_mut() {
        if let Some(e) = e.take() {
            hc::pipe_free(c, &e.pipe);
        }
    }
    match add_endpoints(c, d, eps) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

unsafe extern "C" fn endpoints_change(ctrl: usize, hdev: u32, add: *const UsbEndpoint, n_add: usize, drop: *const u8, n_drop: usize) -> i32 {
    let Some(c) = controller(ctrl) else {
        return USB_EGONE;
    };
    if (n_add != 0 && add.is_null()) || (n_drop != 0 && drop.is_null()) {
        return USB_EIO;
    }
    let add: &[UsbEndpoint] = if n_add == 0 { &[] } else { unsafe { core::slice::from_raw_parts(add, n_add) } };
    let drop: &[u8] = if n_drop == 0 { &[] } else { unsafe { core::slice::from_raw_parts(drop, n_drop) } };
    let Some(s) = slot(ctrl, hdev) else {
        return USB_EGONE;
    };
    let mut entry = s.dev.lock();
    let Some(d) = entry.as_mut() else {
        return USB_EGONE;
    };
    for e in d.eps.iter_mut() {
        if e.is_some_and(|e| drop.contains(&e.address)) {
            hc::pipe_free(c, &e.take().unwrap().pipe);
        }
    }
    // Isochronous endpoints are not driven here: an alternate setting with
    // one comes up, its transfers refused (`isoch_queue`).
    match add_endpoints(c, d, add) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// Isochronous transfers (high-speed iTDs, split siTDs) are not driven
/// (issue #622).
unsafe extern "C" fn isoch_queue(_ctrl: usize, _hdev: u32, _endpoint: u8, _data: *const u8, _lens: *const u16, _n: usize) -> i32 {
    USB_EIO
}

unsafe extern "C" fn isoch_reap(_ctrl: usize, _hdev: u32, _endpoint: u8) -> i32 {
    USB_EIO
}

/// A hub needs nothing of the controller: its children's QHs name it as
/// their translator (`UsbDevicePath::tt_hub`).
unsafe extern "C" fn device_hub(ctrl: usize, hdev: u32, _ports: u8, _tt_think: u8, _multi_tt: u8) -> i32 {
    match get(ctrl, hdev) {
        Ok(_) => 0,
        Err(e) => e,
    }
}

/// The device is going: its waiters give up now.
unsafe extern "C" fn device_abort(ctrl: usize, hdev: u32) {
    let Some(c) = controller(ctrl) else {
        return;
    };
    if let Some(s) = slot(ctrl, hdev) {
        s.gone.store(true, Ordering::Release);
    }
    api().wake(c.key());
    core_wake(c);
}

unsafe extern "C" fn device_free(ctrl: usize, hdev: u32) {
    let Some(c) = controller(ctrl) else {
        return;
    };
    let Some(s) = slot(ctrl, hdev) else {
        return;
    };
    let mut entry = s.dev.lock();
    if let Some(d) = entry.take() {
        for e in d.eps.iter().flatten() {
            hc::pipe_free(c, &e.pipe);
        }
        hc::pipe_free(c, &d.ep0);
    }
}

/// A control transfer on the device's endpoint 0: the setup packet and
/// the data stage in its data page, the status stage the other way; the
/// bytes of the data stage.
#[allow(clippy::too_many_arguments)]
fn control_transfer(c: &Controller, ctrl: usize, hdev: u32, d: &Device, request_type: u8, request: u8, value: u16, index: u16, data: *mut u8, len: u16) -> i32 {
    let len = usize::from(len);
    if len > CONTROL_MAX {
        return USB_EIO;
    }
    let Some(s) = slot(ctrl, hdev) else {
        return USB_EGONE;
    };
    let p = &d.ep0;
    let to_host = request_type & 0x80 != 0;
    let setup = unsafe { core::slice::from_raw_parts_mut(p.data, 8) };
    setup[0] = request_type;
    setup[1] = request;
    setup[2..4].copy_from_slice(&value.to_le_bytes());
    setup[4..6].copy_from_slice(&index.to_le_bytes());
    setup[6..8].copy_from_slice(&(len as u16).to_le_bytes());
    if len != 0 && !to_host {
        unsafe { core::ptr::copy_nonoverlapping(data, p.data.add(CONTROL_DATA), len) };
    }
    hc::sync_to(c, p.data, CONTROL_DATA + len);
    let status_pid = if len == 0 || !to_host { PID_IN } else { PID_OUT };
    let data_pid = if to_host { PID_IN } else { PID_OUT };
    let n = if len != 0 {
        p.set_qtd(0, Some(1), None, PID_SETUP, false, p.data_bus, 8, false);
        p.set_qtd(1, Some(2), Some(2), data_pid, true, p.data_bus + CONTROL_DATA as u64, len, false);
        p.set_qtd(2, None, None, status_pid, true, 0, 0, true);
        3
    } else {
        p.set_qtd(0, Some(1), None, PID_SETUP, false, p.data_bus, 8, false);
        p.set_qtd(1, None, None, status_pid, true, 0, 0, true);
        2
    };
    hc::start(c, p, false);
    let deadline = api().monotonic_ns() + CONTROL_TIMEOUT_MS * 1_000_000;
    let r = if len != 0 { hc::wait(c, p, n, &[1], &[len], deadline, &s.gone) } else { hc::wait(c, p, n, &[], &[], deadline, &s.gone) };
    if hc::halted(p) || r.is_err() {
        hc::unhalt(p);
    }
    match r {
        Ok(moved) => {
            if to_host && moved != 0 {
                hc::sync_from(c, unsafe { p.data.add(CONTROL_DATA) }, moved);
                unsafe { core::ptr::copy_nonoverlapping(p.data.add(CONTROL_DATA), data, moved.min(len)) };
            }
            moved as i32
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn control(ctrl: usize, hdev: u32, request_type: u8, request: u8, value: u16, index: u16, data: *mut u8, len: u16) -> i32 {
    let Some(c) = controller(ctrl) else {
        return USB_EGONE;
    };
    if len != 0 && data.is_null() {
        return USB_EIO;
    }
    let d = match get(ctrl, hdev) {
        Ok(d) => d,
        Err(e) => return e,
    };
    control_transfer(c, ctrl, hdev, &d, request_type, request, value, index, data, len)
}

/// A bulk transfer, through the controller's bounce buffer in pieces of
/// its size, one qTD each; the QH carries the data toggle across.
unsafe extern "C" fn bulk(ctrl: usize, hdev: u32, endpoint: u8, data: *mut u8, len: usize, timeout_ms: u32) -> i32 {
    let Some(c) = controller(ctrl) else {
        return USB_EGONE;
    };
    if data.is_null() && len != 0 {
        return USB_EIO;
    }
    let d = match get(ctrl, hdev) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let Some(s) = slot(ctrl, hdev) else {
        return USB_EGONE;
    };
    let Some(e) = d.eps.iter().flatten().find(|e| e.address == endpoint && e.kind == 2).copied() else {
        return USB_EIO;
    };
    let to_host = endpoint & 0x80 != 0;
    let deadline = api().monotonic_ns() + u64::from(timeout_ms.max(1)) * 1_000_000;
    let bounce = c.bounce.lock();
    let chunk_max = hc::BOUNCE_PAGES * 4096;
    let mut done = 0usize;
    let mut result = 0;
    loop {
        let chunk = (len - done).min(chunk_max);
        if !to_host && chunk != 0 {
            unsafe { core::ptr::copy_nonoverlapping(data.add(done), bounce.va, chunk) };
        }
        hc::sync_to(c, bounce.va, chunk.max(1));
        e.pipe.set_qtd(0, None, None, if to_host { PID_IN } else { PID_OUT }, false, bounce.bus, chunk, true);
        hc::start(c, &e.pipe, true);
        match hc::wait(c, &e.pipe, 1, &[0], &[chunk], deadline, &s.gone) {
            Ok(moved) => {
                if to_host && moved != 0 {
                    hc::sync_from(c, bounce.va, moved);
                    unsafe { core::ptr::copy_nonoverlapping(bounce.va, data.add(done), moved) };
                }
                done += moved;
                // A short packet, or everything, ends the transfer.
                if moved < chunk || done >= len {
                    break;
                }
            }
            Err(err) => {
                if hc::halted(&e.pipe) {
                    hc::unhalt(&e.pipe);
                }
                result = err;
                break;
            }
        }
    }
    drop(bounce);
    if result < 0 { result } else { done as i32 }
}

/// Queue an IN transfer on an interrupt or bulk endpoint (the core's
/// `transfer_start`; a page at most): one qTD into the endpoint's data
/// page, interrupting when done; the core asks [`transfer_take`].
unsafe extern "C" fn transfer_start(ctrl: usize, hdev: u32, endpoint: u8, data: *mut u8, len: usize) -> i32 {
    let Some(c) = controller(ctrl) else {
        return USB_EGONE;
    };
    if data.is_null() || len == 0 || len > 4096 || endpoint & 0x80 == 0 {
        return USB_EIO;
    }
    let r = update(ctrl, hdev, |d| {
        let Some(e) = d.eps.iter_mut().flatten().find(|e| e.address == endpoint) else {
            return USB_EIO;
        };
        if e.queued.is_some() || e.pipe.data.is_null() {
            return USB_EIO;
        }
        // An interrupt endpoint's one report a transfer.
        let len = if e.kind == 3 { len.min(usize::from(e.max_packet)) } else { len };
        e.pipe.set_qtd(0, None, None, PID_IN, false, e.pipe.data_bus, len, true);
        hc::start(c, &e.pipe, true);
        e.queued = Some((data as usize, len));
        0
    });
    r.unwrap_or_else(|e| e)
}

/// The queued transfer on `endpoint`: [`USB_EPENDING`] while it runs (a
/// NAKing endpoint is asked again by the controller), else its result,
/// the bytes copied to the core's buffer.
unsafe extern "C" fn transfer_take(ctrl: usize, hdev: u32, endpoint: u8) -> i32 {
    let Some(c) = controller(ctrl) else {
        return USB_EGONE;
    };
    let gone = slot(ctrl, hdev).is_none_or(|s| s.gone.load(Ordering::Acquire));
    let r = update(ctrl, hdev, |d| {
        let Some(e) = d.eps.iter_mut().flatten().find(|e| e.address == endpoint) else {
            return USB_EIO;
        };
        let Some((user, len)) = e.queued else {
            return USB_EIO;
        };
        if gone {
            hc::cancel(c, &e.pipe);
            e.queued = None;
            return USB_EGONE;
        }
        match hc::outcome(c, &e.pipe, 1, &[0], &[len]) {
            None => USB_EPENDING,
            Some(Ok(moved)) => {
                hc::sync_from(c, e.pipe.data, moved.max(1));
                unsafe { core::ptr::copy_nonoverlapping(e.pipe.data, user as *mut u8, moved) };
                e.queued = None;
                moved as i32
            }
            Some(Err(err)) => {
                hc::unhalt(&e.pipe);
                e.queued = None;
                err
            }
        }
    });
    r.unwrap_or(USB_EGONE)
}

/// The controller's side of recovering a stalled endpoint: its QH's toggle
/// back to DATA0, the halt cleared (the core sends the CLEAR_FEATURE).
unsafe extern "C" fn clear_halt(ctrl: usize, hdev: u32, endpoint: u8) -> i32 {
    let r = update(ctrl, hdev, |d| {
        if let Some(e) = d.eps.iter().flatten().find(|e| e.address == endpoint) {
            hc::reset_toggle(&e.pipe);
        }
    });
    match r {
        Ok(()) => 0,
        Err(e) => e,
    }
}

// ---- Interrupts and probing -------------------------------------------------

unsafe extern "C" fn irq_handler(ctx: *mut c_void) {
    let Some(c) = controller(ctx as usize) else {
        return;
    };
    c.irqs.fetch_add(1, Ordering::Relaxed);
    hc::process(c);
}

const CLASS_SERIAL_BUS: u8 = 0x0C;
const SUBCLASS_USB: u8 = 0x03;
const PROGIF_EHCI: u8 = 0x20;

/// Every EHCI controller on the bus not taken yet: brought up and
/// registered with the core; the number of them.
fn probe(api: &KernelApi) -> usize {
    let mut new = 0;
    let mut nth = 0u32;
    loop {
        let (mut bus, mut slot, mut func) = (0u8, 0u8, 0u8);
        if api.pci_find_class(CLASS_SERIAL_BUS, SUBCLASS_USB, nth, &mut bus, &mut slot, &mut func) != 0 {
            break;
        }
        nth += 1;
        let class = api.pci_cfg_read32(bus, slot, func, 0x08);
        if ((class >> 8) & 0xFF) as u8 != PROGIF_EHCI {
            continue;
        }
        let Some(index) = CONTROLLERS.iter().position(|c| c.lock().is_none()) else {
            break;
        };
        api.pci_enable(bus, slot, func);
        let (mut va, mut size) = (0usize, 0u64);
        if api.pci_bar_map(bus, slot, func, 0, &mut va, &mut size) != 0 || size < 0x100 {
            continue;
        }
        // 32-bit pointers: its DMA below 4 GiB.
        let dma = api.dma_attach(&DmaDevice::pci(bus, slot, func).below(1 << 32));
        match hc::init(va, (bus, slot, func), dma, index as u8) {
            Ok(c) => {
                // SAFETY: each cell is written once, here, before the
                // controller is published; the module's init and rescan
                // do not run at once.
                let c: &'static Controller = unsafe {
                    let cells = &mut *core::ptr::addr_of_mut!(CELLS);
                    cells[index] = Some(c);
                    cells[index].as_ref().unwrap()
                };
                *CONTROLLERS[index].lock() = Some(c);
                let mut msix = 0u16;
                let rc = api.pci_irq_enable(bus, slot, func, "ehci", irq_handler, index as *mut c_void, &mut msix);
                c.irq_on.store(rc == 0, Ordering::Release);
                if rc == 0 {
                    hc::enable_interrupts(c);
                }
                let core_index = core().controller_register(&HCD_OPS, index);
                if core_index < 0 {
                    status_fail(api, "ehci: the usb core refused the controller");
                    continue;
                }
                c.core_index.store(core_index as u32, Ordering::Release);
                new += 1;
            }
            Err(why) => {
                let mut buf = [0u8; 64];
                let mut w = Writer { buf: &mut buf, len: 0 };
                w.str("ehci: ");
                w.str(why);
                let n = w.len;
                status_fail(api, core::str::from_utf8(&buf[..n]).unwrap_or("ehci"));
            }
        }
    }
    new
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api_ptr: *const KernelApi) -> i32 {
    if api_ptr.is_null() {
        return -1;
    }
    let api: &'static KernelApi = unsafe { &*api_ptr };
    if api.abi_version != ABI_VERSION {
        return -2;
    }
    unsafe { API.set(api) };
    let table = api.service_lookup(USB_CORE_SERVICE) as *const UsbCoreOps;
    if table.is_null() {
        api.write_str("ehci: no usb core\n");
        return -3;
    }
    if unsafe { (*table).version } != USB_CORE_VERSION {
        return -4;
    }
    CORE.store(table.cast_mut(), Ordering::Release);
    if probe(api) == 0 {
        api.write_str("ehci none\n");
        return 0;
    }
    let mut buf = [0u8; 64];
    let mut w = Writer { buf: &mut buf, len: 0 };
    w.str("ehci: ");
    let mut first = true;
    for i in 0..hc::MAX_CTRL {
        if let Some(c) = controller(i) {
            if !first {
                w.str(", ");
            }
            first = false;
            w.dec(u32::from(c.ports));
            w.str(" ports");
            w.str(if c.irq_on.load(Ordering::Relaxed) { " irq" } else { " polled" });
        }
    }
    let n = w.len;
    status_ok(api, core::str::from_utf8(&buf[..n]).unwrap_or("ehci"));
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_rescan() {
    // A controller that appeared after boot.
    if !CORE.load(Ordering::Acquire).is_null() {
        probe(api());
    }
}

// So `cargo build --bin ehci` links. The kernel never jumps here.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let api = api();
    let mut buf = [0u8; 96];
    let mut w = Writer { buf: &mut buf, len: 0 };
    w.str("ehci: panic");
    if let Some(l) = info.location() {
        let file = l.file();
        let tail = file.rsplit('/').next().unwrap_or(file);
        w.str(" at ");
        w.str(tail);
        w.str(":");
        w.dec(l.line());
    }
    let n = w.len;
    status_fail(api, core::str::from_utf8(&buf[..n]).unwrap_or("ehci: panic"));
    loop {
        api.task_yield();
    }
}
