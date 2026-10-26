//! UHCI: a USB host controller driver module (`docs/usb.md`) for USB 1.1
//! controllers on PCI (class 0C/03, interface 0x00): Intel's and VIA's,
//! alone or as an EHCI's companions, which the `ehci` module hands its
//! full- and low-speed devices to.
//!
//! Brings each controller up (`hc`) and registers it with the USB core
//! (the `usb` module's [`USB_CORE_SERVICE`]), which owns the devices and
//! their enumeration: this module does the controller's part through the
//! [`UsbHcdOps`] it hands the core: control, bulk and interrupt
//! transfers, and isochronous OUT ones (a speaker, `usb_audio`). Its
//! registers are in PCI I/O space ([`KernelApi::pci_io_map`]).

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

mod hc;

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use myos_abi::{
    ABI_VERSION, ApiCell, DmaDevice, KernelApi, Lock, StrRef, USB_CORE_SERVICE, USB_CORE_VERSION, USB_EGONE, USB_EIO,
    USB_EPENDING, USB_HCD_VERSION, USB_ISOCH_MAX, UsbCoreOps, UsbDevicePath, UsbEndpoint, UsbHcdOps,
    UsbPortStatus, status_fail, status_ok,
};

use hc::{Controller, Iso, Kind, PID_IN, PID_OUT, PID_SETUP, Pipe, Target};

/// A control transfer's data stage at most (the descriptors), in the
/// endpoint's data page after the setup packet.
const CONTROL_MAX: usize = 1024;
const CONTROL_DATA: usize = 64;
const CONTROL_TIMEOUT_MS: u64 = 2000;
const MAX_DEVICES: usize = 32;
const MAX_ENDPOINTS: usize = 15;
/// Isochronous endpoints running at once, over every controller.
const MAX_ISO: usize = 8;

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
    unsafe { CORE.load(Ordering::Acquire).as_ref() }.expect("uhci: usb core")
}

fn controller(ctrl: usize) -> Option<&'static Controller> {
    *CONTROLLERS.get(ctrl)?.lock()
}

pub fn sleep_ms(ms: u64) {
    let until = api().monotonic_ns() + ms * 1_000_000;
    api().task_sleep_until(until);
}

pub fn core_wake(c: &Controller) {
    let index = c.core_index.load(Ordering::Acquire);
    if index != u32::MAX {
        core().wake(index);
    }
}

pub fn core_port_change(c: &Controller, port: u8) {
    let index = c.core_index.load(Ordering::Acquire);
    if index != u32::MAX {
        core().port_change(index, port);
    }
}

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
    /// Transfer type (`bmAttributes & 3`): 1 isochronous, 2 bulk, 3 interrupt.
    kind: u8,
    max_packet: u16,
    /// The next packet's data toggle: DATA1 when set.
    toggle: bool,
    /// Bulk and interrupt: its QH. Isochronous packets go in the frame list
    /// (`ISOS`).
    pipe: Option<Pipe>,
    /// A queued transfer (`transfer_start`): the core's buffer, length,
    /// and the TDs it took.
    queued: Option<(usize, usize, usize)>,
}

#[derive(Clone, Copy)]
struct Device {
    address: u8,
    speed: u8,
    ep0_max_packet: u16,
    ep0: Pipe,
    eps: [Option<Endpoint>; MAX_ENDPOINTS],
}

struct Slot {
    gone: AtomicBool,
    dev: Lock<Option<Device>>,
}

static DEVICES: [[Slot; MAX_DEVICES]; hc::MAX_CTRL] =
    [const { [const { Slot { gone: AtomicBool::new(false), dev: Lock::new(None) } }; MAX_DEVICES] }; hc::MAX_CTRL];

/// The isochronous endpoints' packet rings, `(controller, device, endpoint)`.
static ISOS: [Lock<Option<(usize, u32, u8, Iso)>>; MAX_ISO] = [const { Lock::new(None) }; MAX_ISO];

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

fn target(d: &Device, endpoint: u8, max_packet: u16) -> Target {
    Target { address: d.address, endpoint: endpoint & 0xF, speed: d.speed, max_packet }
}

/// Bulk and interrupt endpoints of `eps` get a QH each (an IN one a data
/// page for its queued transfers); isochronous ones are recorded.
fn add_endpoints(c: &Controller, d: &mut Device, eps: &[UsbEndpoint]) -> Result<(), i32> {
    for e in eps {
        let kind = e.attributes & 3;
        if kind == 0 || d.eps.iter().flatten().any(|x| x.address == e.address) {
            continue;
        }
        let free = d.eps.iter().position(|x| x.is_none()).ok_or(USB_EIO)?;
        let pipe = match kind {
            2 => Some(hc::pipe_new(c, Kind::Bulk, e.address & 0x80 != 0).ok_or(USB_EIO)?),
            3 => Some(hc::pipe_new(c, Kind::Interrupt, e.address & 0x80 != 0).ok_or(USB_EIO)?),
            _ => None,
        };
        d.eps[free] = Some(Endpoint { address: e.address, kind, max_packet: e.max_packet & 0x7FF, toggle: false, pipe, queued: None });
    }
    Ok(())
}

/// Forget endpoint `e` of device `hdev`: its QH, or its packet ring.
fn drop_endpoint(c: &Controller, ctrl: usize, hdev: u32, e: &Endpoint) {
    if let Some(p) = e.pipe {
        hc::pipe_free(c, &p);
    }
    for s in ISOS.iter() {
        let taken = {
            let mut s = s.lock();
            if s.as_ref().is_some_and(|i| i.0 == ctrl && i.1 == hdev && i.2 == e.address) { s.take() } else { None }
        };
        if let Some((_, _, _, iso)) = taken {
            hc::iso_free(c, iso);
        }
    }
}

// ---- The controller driver's table ---------------------------------------------

static HCD_OPS: UsbHcdOps = UsbHcdOps {
    version: USB_HCD_VERSION,
    name: StrRef { ptr: b"uhci".as_ptr(), len: 4 },
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
    controller(ctrl).map_or(0, |_| hc::PORTS)
}

unsafe extern "C" fn port_status(ctrl: usize, port: u8, out: *mut UsbPortStatus) -> i32 {
    let Some(c) = controller(ctrl) else {
        return USB_EGONE;
    };
    if out.is_null() || port >= hc::PORTS {
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
    if port >= hc::PORTS {
        return USB_EIO;
    }
    if hc::port_reset(c, port) { 0 } else { USB_EIO }
}

/// Every pass of the core's thread: the ports looked at (UHCI does not
/// interrupt for them), and without an interrupt the status too. Always
/// nonzero: the core comes back every few milliseconds.
unsafe extern "C" fn poll(ctrl: usize) -> i32 {
    let Some(c) = controller(ctrl) else {
        return 0;
    };
    if !c.irq_on.load(Ordering::Relaxed) {
        hc::process(c);
    }
    hc::poll_ports(c);
    1
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
    w.str("uhci ");
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
    let Some(table) = DEVICES.get(ctrl) else {
        return USB_EIO;
    };
    for (i, s) in table.iter().enumerate() {
        let mut entry = s.dev.lock();
        if entry.is_some() {
            continue;
        }
        let Some(ep0) = hc::pipe_new(c, Kind::Control, true) else {
            return USB_EIO;
        };
        *entry = Some(Device { address: 0, speed: path.speed, ep0_max_packet: 8, ep0, eps: [None; MAX_ENDPOINTS] });
        s.gone.store(false, Ordering::Release);
        return i as i32 + 1;
    }
    USB_EIO
}

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
    let _ = update(ctrl, hdev, |d| d.address = address);
    sleep_ms(2);
    0
}

unsafe extern "C" fn device_ep0(ctrl: usize, hdev: u32, max_packet: u16) -> i32 {
    match update(ctrl, hdev, |d| d.ep0_max_packet = max_packet) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

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
            drop_endpoint(c, ctrl, hdev, &e);
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
            drop_endpoint(c, ctrl, hdev, &e.take().unwrap());
        }
    }
    match add_endpoints(c, d, add) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// Isochronous OUT packets on `endpoint`, each in a frame of its own
/// ([`hc::iso_queue`]); the endpoint's ring made on its first packets.
unsafe extern "C" fn isoch_queue(ctrl: usize, hdev: u32, endpoint: u8, data: *const u8, lens: *const u16, n: usize) -> i32 {
    let Some(c) = controller(ctrl) else {
        return USB_EGONE;
    };
    if n == 0 || n > USB_ISOCH_MAX || data.is_null() || lens.is_null() || endpoint & 0x80 != 0 {
        return USB_EIO;
    }
    let lens = unsafe { core::slice::from_raw_parts(lens, n) };
    let total: usize = lens.iter().map(|&l| usize::from(l)).sum();
    let data = unsafe { core::slice::from_raw_parts(data, total) };
    let d = match get(ctrl, hdev) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let Some(e) = d.eps.iter().flatten().find(|e| e.address == endpoint && e.kind == 1).copied() else {
        return USB_EIO;
    };
    let t = target(&d, endpoint, e.max_packet);
    let mine = |i: &(usize, u32, u8, Iso)| i.0 == ctrl && i.1 == hdev && i.2 == endpoint;
    if !ISOS.iter().any(|s| s.lock().as_ref().is_some_and(mine)) {
        let Some(free) = ISOS.iter().find(|s| s.lock().is_none()) else {
            return USB_EIO;
        };
        let Some(iso) = hc::iso_new(c, e.max_packet) else {
            return USB_EIO;
        };
        *free.lock() = Some((ctrl, hdev, endpoint, iso));
    }
    for s in ISOS.iter() {
        let mut s = s.lock();
        if let Some(i) = s.as_mut().filter(|i| mine(i)) {
            return hc::iso_queue(c, &mut i.3, &t, data, lens);
        }
    }
    USB_EIO
}

/// The packets of `endpoint` that went since the last call.
unsafe extern "C" fn isoch_reap(ctrl: usize, hdev: u32, endpoint: u8) -> i32 {
    let Some(c) = controller(ctrl) else {
        return USB_EGONE;
    };
    if slot(ctrl, hdev).is_none_or(|s| s.gone.load(Ordering::Acquire)) {
        return USB_EGONE;
    }
    for s in ISOS.iter() {
        let mut s = s.lock();
        if let Some(i) = s.as_mut().filter(|i| i.0 == ctrl && i.1 == hdev && i.2 == endpoint) {
            hc::iso_reap(c, &mut i.3);
            let done = i.3.done;
            i.3.done = 0;
            return done as i32;
        }
    }
    0
}

/// A hub needs nothing of the controller: a full-speed bus has no
/// transaction translators, and a low-speed device behind a hub is reached
/// with the TD's low-speed bit (the controller sends the preamble).
unsafe extern "C" fn device_hub(ctrl: usize, hdev: u32, _ports: u8, _tt_think: u8, _multi_tt: u8) -> i32 {
    match get(ctrl, hdev) {
        Ok(_) => 0,
        Err(e) => e,
    }
}

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
            drop_endpoint(c, ctrl, hdev, e);
        }
        hc::pipe_free(c, &d.ep0);
    }
}

/// A control transfer on the device's endpoint 0: the setup TD, one TD a
/// packet of the data stage (its toggle from DATA1), the status TD the
/// other way; the bytes of the data stage.
#[allow(clippy::too_many_arguments)]
fn control_transfer(c: &Controller, ctrl: usize, hdev: u32, d: &Device, request_type: u8, request: u8, value: u16, index: u16, data: *mut u8, len: u16) -> i32 {
    let len = usize::from(len);
    let mps = usize::from(d.ep0_max_packet.max(8));
    if len > CONTROL_MAX || len.div_ceil(mps) + 2 > hc::MAX_TDS {
        return USB_EIO;
    }
    let Some(s) = slot(ctrl, hdev) else {
        return USB_EGONE;
    };
    let p = &d.ep0;
    let t = target(d, 0, d.ep0_max_packet);
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
    p.set_td(0, &t, PID_SETUP, false, p.data_bus, 8, false);
    let mut i = 1;
    let mut at = 0;
    let mut dt = true;
    while at < len {
        let n = (len - at).min(mps);
        p.set_td(i, &t, if to_host { PID_IN } else { PID_OUT }, dt, p.data_bus + (CONTROL_DATA + at) as u64, n, false);
        dt = !dt;
        at += n;
        i += 1;
    }
    let status = i;
    p.set_td(status, &t, if len == 0 || !to_host { PID_IN } else { PID_OUT }, true, 0, 0, true);
    hc::start(c, p);
    let deadline = api().monotonic_ns() + CONTROL_TIMEOUT_MS * 1_000_000;
    match hc::wait(c, p, status + 1, Some(status), deadline, &s.gone) {
        Ok((moved, _)) => {
            hc::clear(c, p);
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

/// Write the TDs of `len` bytes at `buf` for `t` on `p`, packet by packet
/// from toggle `dt`: how many.
fn packets(p: &Pipe, t: &Target, pid: u32, mut dt: bool, buf: u64, len: usize) -> usize {
    let mps = usize::from(t.max_packet.max(1));
    let n = len.div_ceil(mps).max(1);
    for i in 0..n {
        let at = i * mps;
        let size = (len - at.min(len)).min(mps);
        p.set_td(i, t, pid, dt, buf + at as u64, size, i + 1 == n);
        dt = !dt;
    }
    n
}

/// A bulk transfer, through the controller's bounce buffer in pieces of
/// what fits (its size, the TDs a QH's page holds); the toggle carried
/// across in the endpoint's record.
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
    let Some(p) = e.pipe else {
        return USB_EIO;
    };
    let t = target(&d, endpoint, e.max_packet);
    let to_host = endpoint & 0x80 != 0;
    let deadline = api().monotonic_ns() + u64::from(timeout_ms.max(1)) * 1_000_000;
    let bounce = c.bounce.lock();
    let chunk_max = (hc::BOUNCE_PAGES * 4096).min(usize::from(e.max_packet.max(1)) * hc::MAX_TDS);
    let mut toggle = e.toggle;
    let mut done = 0usize;
    let mut result = 0;
    loop {
        let chunk = (len - done).min(chunk_max);
        if !to_host && chunk != 0 {
            unsafe { core::ptr::copy_nonoverlapping(data.add(done), bounce.va, chunk) };
        }
        hc::sync_to(c, bounce.va, chunk.max(1));
        let n = packets(&p, &t, if to_host { PID_IN } else { PID_OUT }, toggle, bounce.bus, chunk);
        hc::start(c, &p);
        match hc::wait(c, &p, n, None, deadline, &s.gone) {
            Ok((moved, sent)) => {
                hc::clear(c, &p);
                if sent & 1 != 0 {
                    toggle = !toggle;
                }
                if to_host && moved != 0 {
                    hc::sync_from(c, bounce.va, moved);
                    unsafe { core::ptr::copy_nonoverlapping(bounce.va, data.add(done), moved) };
                }
                done += moved;
                if moved < chunk || done >= len {
                    break;
                }
            }
            Err(err) => {
                result = err;
                break;
            }
        }
    }
    drop(bounce);
    let _ = update(ctrl, hdev, |d| {
        if let Some(e) = d.eps.iter_mut().flatten().find(|e| e.address == endpoint) {
            e.toggle = toggle;
        }
    });
    if result < 0 { result } else { done as i32 }
}

/// Queue an IN transfer on an interrupt or bulk endpoint (a page at most;
/// an interrupt endpoint's one report): its TDs into the endpoint's data
/// page on its QH, which the controller visits every frame (interrupt)
/// or every pass (bulk); a NAK leaves a TD active for the next.
unsafe extern "C" fn transfer_start(ctrl: usize, hdev: u32, endpoint: u8, data: *mut u8, len: usize) -> i32 {
    let Some(c) = controller(ctrl) else {
        return USB_EGONE;
    };
    if data.is_null() || len == 0 || len > 4096 || endpoint & 0x80 == 0 {
        return USB_EIO;
    }
    let r = update(ctrl, hdev, |d| {
        let (address, speed) = (d.address, d.speed);
        let Some(e) = d.eps.iter_mut().flatten().find(|e| e.address == endpoint) else {
            return USB_EIO;
        };
        let Some(p) = e.pipe.filter(|p| !p.data.is_null()) else {
            return USB_EIO;
        };
        if e.queued.is_some() {
            return USB_EIO;
        }
        let len = if e.kind == 3 { len.min(usize::from(e.max_packet)) } else { len };
        let t = Target { address, endpoint: endpoint & 0xF, speed, max_packet: e.max_packet };
        let n = packets(&p, &t, PID_IN, e.toggle, p.data_bus, len);
        hc::start(c, &p);
        e.queued = Some((data as usize, len, n));
        0
    });
    r.unwrap_or_else(|e| e)
}

unsafe extern "C" fn transfer_take(ctrl: usize, hdev: u32, endpoint: u8) -> i32 {
    let Some(c) = controller(ctrl) else {
        return USB_EGONE;
    };
    let gone = slot(ctrl, hdev).is_none_or(|s| s.gone.load(Ordering::Acquire));
    let r = update(ctrl, hdev, |d| {
        let Some(e) = d.eps.iter_mut().flatten().find(|e| e.address == endpoint) else {
            return USB_EIO;
        };
        let (Some((user, _, n)), Some(p)) = (e.queued, e.pipe) else {
            return USB_EIO;
        };
        if gone {
            hc::clear(c, &p);
            e.queued = None;
            return USB_EGONE;
        }
        match hc::outcome(c, &p, n, None) {
            None => USB_EPENDING,
            Some(Ok((moved, sent))) => {
                hc::clear(c, &p);
                if sent & 1 != 0 {
                    e.toggle = !e.toggle;
                }
                hc::sync_from(c, p.data, moved.max(1));
                unsafe { core::ptr::copy_nonoverlapping(p.data, user as *mut u8, moved) };
                e.queued = None;
                moved as i32
            }
            Some(Err(err)) => {
                hc::clear(c, &p);
                e.queued = None;
                err
            }
        }
    });
    r.unwrap_or(USB_EGONE)
}

/// The controller's side of recovering a stalled endpoint: its toggle back
/// to DATA0 (the device's is, after the core's CLEAR_FEATURE).
unsafe extern "C" fn clear_halt(ctrl: usize, hdev: u32, endpoint: u8) -> i32 {
    match update(ctrl, hdev, |d| {
        if let Some(e) = d.eps.iter_mut().flatten().find(|e| e.address == endpoint) {
            e.toggle = false;
        }
    }) {
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
const PROGIF_UHCI: u8 = 0x00;
/// UHCI's registers: I/O BAR 4 (USBBASE).
const USBBASE_BAR: u8 = 4;

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
        if ((class >> 8) & 0xFF) as u8 != PROGIF_UHCI {
            continue;
        }
        if (0..hc::MAX_CTRL).any(|i| controller(i).is_some_and(|c| c.pci == (bus, slot, func))) {
            continue;
        }
        let Some(index) = CONTROLLERS.iter().position(|c| c.lock().is_none()) else {
            break;
        };
        let Some(io) = api.pci_io_map(bus, slot, func, USBBASE_BAR).filter(|io| io.size() >= 0x14) else {
            status_fail(api, "uhci: no I/O BAR");
            continue;
        };
        api.pci_enable(bus, slot, func);
        let dma = api.dma_attach(&DmaDevice::pci(bus, slot, func).below(1 << 32));
        match hc::init(io, (bus, slot, func), dma) {
            Ok(c) => {
                // SAFETY: each cell is written once, before the controller
                // is published; init and rescan do not run at once.
                let c: &'static Controller = unsafe {
                    let cells = &mut *core::ptr::addr_of_mut!(CELLS);
                    cells[index] = Some(c);
                    cells[index].as_ref().unwrap()
                };
                *CONTROLLERS[index].lock() = Some(c);
                let mut msix = 0u16;
                let rc = api.pci_irq_enable(bus, slot, func, "uhci", irq_handler, index as *mut c_void, &mut msix);
                c.irq_on.store(rc == 0, Ordering::Release);
                if rc == 0 {
                    hc::enable_interrupts(c);
                }
                let core_index = core().controller_register(&HCD_OPS, index);
                if core_index < 0 {
                    status_fail(api, "uhci: the usb core refused the controller");
                    continue;
                }
                c.core_index.store(core_index as u32, Ordering::Release);
                new += 1;
            }
            Err(why) => {
                let mut buf = [0u8; 64];
                let mut w = Writer { buf: &mut buf, len: 0 };
                w.str("uhci: ");
                w.str(why);
                let n = w.len;
                status_fail(api, core::str::from_utf8(&buf[..n]).unwrap_or("uhci"));
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
        api.write_str("uhci: no usb core\n");
        return -3;
    }
    if unsafe { (*table).version } != USB_CORE_VERSION {
        return -4;
    }
    CORE.store(table.cast_mut(), Ordering::Release);
    let n = probe(api);
    if n == 0 {
        api.write_str("uhci none\n");
        return 0;
    }
    let mut buf = [0u8; 64];
    let mut w = Writer { buf: &mut buf, len: 0 };
    w.str("uhci: ");
    w.dec(n as u32);
    w.str(if n == 1 { " controller" } else { " controllers" });
    let irq = (0..hc::MAX_CTRL).filter_map(controller).all(|c| c.irq_on.load(Ordering::Relaxed));
    w.str(if irq { " irq" } else { " polled" });
    let n = w.len;
    status_ok(api, core::str::from_utf8(&buf[..n]).unwrap_or("uhci"));
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_rescan() {
    if !CORE.load(Ordering::Acquire).is_null() {
        probe(api());
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let api = api();
    let mut buf = [0u8; 96];
    let mut w = Writer { buf: &mut buf, len: 0 };
    w.str("uhci: panic");
    if let Some(l) = info.location() {
        let file = l.file();
        let tail = file.rsplit('/').next().unwrap_or(file);
        w.str(" at ");
        w.str(tail);
        w.str(":");
        w.dec(l.line());
    }
    let n = w.len;
    status_fail(api, core::str::from_utf8(&buf[..n]).unwrap_or("uhci: panic"));
    loop {
        api.task_yield();
    }
}
