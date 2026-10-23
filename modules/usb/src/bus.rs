//! The devices: enumeration (address, descriptors, endpoints) through their
//! controller's driver, their interfaces offered to the class drivers,
//! hubs' children with their route strings, and detach. All of it runs on
//! the USB thread (`crate::thread_main`), except the transfers class
//! drivers start.
//!
//! A device lives in a [`Slot`] behind a `SleepLock`, held for a transfer
//! (which waits for its completion in the controller's driver), by the
//! thread enumerating it and by `detach_id`. The `gone` flag `detach_id`
//! raises before it waits for the lock is outside it. No lock is held
//! across a call into a class driver (`probe`, `disconnect`, a completion
//! callback): the driver calls back in.

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};

use myos_abi::{
    Lock, SleepLock, USB_EGONE, USB_EIO, USB_EPENDING, USB_LABEL_MAX, USB_MAX_ENDPOINTS, USB_SPEED_FULL,
    USB_SPEED_HIGH, USB_SPEED_LOW, USB_SPEED_SUPER, UsbCompletion, UsbDeviceInfo, UsbDevicePath, UsbDriverOps,
    UsbEndpoint, UsbInterfaceInfo,
};

use crate::{Controller, MAX_CTRL};

/// Devices per controller.
pub const MAX_DEVICES: usize = 32;
/// Interfaces per device the core describes (the rest are ignored).
pub const MAX_INTERFACES: usize = 8;
/// Class drivers that may register.
pub const MAX_DRIVERS: usize = 8;
/// Interrupt transfers queued on one device at a time (its endpoints with
/// a class driver's callback).
const MAX_CALLBACKS: usize = 4;
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

/// A transfer a class driver queued (`transfer_start`): its
/// endpoint and the callback run when the controller reports it done.
#[derive(Clone, Copy)]
pub struct Callback {
    pub endpoint: u8,
    pub done: UsbCompletion,
    pub ctx: *mut c_void,
}

pub struct Device {
    /// The controller driver's handle.
    pub hdev: u32,
    pub info: UsbDeviceInfo,
    pub path: UsbDevicePath,
    pub hub_ports: u8,
    pub interfaces: [Option<Interface>; MAX_INTERFACES],
    pub callbacks: [Option<Callback>; MAX_CALLBACKS],
}

// SAFETY: the callbacks' contexts are the class drivers' own, handed back
// to them on the USB thread.
unsafe impl Send for Device {}

/// A device table entry. The index is in the device's id.
pub struct Slot {
    /// Raised by `detach_id` before it takes the lock: a transfer finding
    /// it set gives up (`USB_EGONE`) instead of starting.
    pub gone: AtomicBool,
    dev: SleepLock<Option<Device>>,
}

impl Slot {
    const EMPTY: Self = Self { gone: AtomicBool::new(false), dev: SleepLock::new(&crate::API, None) };
}

/// The enumeration step that failed last, for the boot log.
pub static STEP: Lock<&'static str> = Lock::new("");

fn step(s: &'static str) {
    *STEP.lock() = s;
}

static DEVICES: [[Slot; MAX_DEVICES]; MAX_CTRL] = [const { [const { Slot::EMPTY }; MAX_DEVICES] }; MAX_CTRL];
/// The class drivers, in registration order (`driver_register` adds one
/// from any task; the thread copies the list out before calling into it).
pub static DRIVERS: Lock<[Option<&'static UsbDriverOps>; MAX_DRIVERS]> = Lock::new([None; MAX_DRIVERS]);

/// The table entry `index` of controller `ctrl`.
fn slot_at(ctrl: u8, index: usize) -> Option<&'static Slot> {
    DEVICES.get(usize::from(ctrl))?.get(index)
}

/// A device id: controller in the high byte, table index + 1 below.
pub fn id_of(ctrl: u8, index: usize) -> u32 {
    (u32::from(ctrl) << 8) | (index as u32 + 1)
}

/// The table entry a device id names.
fn slot_of(id: u32) -> Option<&'static Slot> {
    let index = (id & 0xFF) as usize;
    if index == 0 {
        return None;
    }
    slot_at((id >> 8) as u8, index - 1)
}

/// Run `f` on the device `id`, its controller and its slot, with the
/// slot's lock held: `USB_EGONE` when there is no such device or it is
/// gone.
pub fn with_device<R>(id: u32, f: impl FnOnce(&Controller, &Slot, &mut Device) -> R) -> Result<R, i32> {
    let c = crate::controller((id >> 8) as u8).ok_or(USB_EGONE)?;
    let s = slot_of(id).ok_or(USB_EGONE)?;
    let mut dev = s.dev.lock();
    if s.gone.load(Ordering::Acquire) {
        return Err(USB_EGONE);
    }
    let dev = dev.as_mut().ok_or(USB_EGONE)?;
    Ok(f(c, s, dev))
}

/// The device `id`'s field `f`, if it is there: a copy out from under the
/// lock.
fn peek<R>(id: u32, f: impl FnOnce(&Device) -> R) -> Option<R> {
    let s = slot_of(id)?;
    let dev = s.dev.lock();
    if s.gone.load(Ordering::Acquire) {
        return None;
    }
    dev.as_ref().map(f)
}

fn ep0_max_packet(speed: u8) -> u16 {
    match speed {
        USB_SPEED_LOW | USB_SPEED_FULL => 8,
        USB_SPEED_HIGH => 64,
        _ => 512,
    }
}

/// A control transfer on `dev`'s endpoint 0 (the host's `control`): bytes
/// moved in the data stage, or an error.
pub fn control(c: &Controller, dev: &Device, request_type: u8, request: u8, value: u16, index: u16, data: *mut u8, len: u16) -> i32 {
    if usize::from(len) > CONTROL_MAX {
        return USB_EIO;
    }
    unsafe { (c.ops.control)(c.ctx, dev.hdev, request_type, request, value, index, data, len) }
}

/// Queue an interrupt or bulk IN transfer (the host's `transfer_start`): the
/// controller takes it, the callback waits for its completion.
pub fn transfer_start(c: &Controller, dev: &mut Device, endpoint: u8, data: *mut u8, len: usize, done: UsbCompletion, ctx: *mut c_void) -> i32 {
    if dev.callbacks.iter().flatten().any(|cb| cb.endpoint == endpoint) {
        return USB_EIO;
    }
    let Some(free) = dev.callbacks.iter().position(|cb| cb.is_none()) else {
        return USB_EIO;
    };
    let r = unsafe { (c.ops.transfer_start)(c.ctx, dev.hdev, endpoint, data, len) };
    if r < 0 {
        return r;
    }
    dev.callbacks[free] = Some(Callback { endpoint, done, ctx });
    0
}

/// A completed queued transfer of the device in `s`, if any: its record
/// released in the controller, its callback taken (to be run with the
/// lock dropped).
fn take_completion(c: &Controller, s: &Slot) -> Option<(UsbCompletion, *mut c_void, i32)> {
    let mut dev = s.dev.lock();
    let dev = dev.as_mut()?;
    for cb in dev.callbacks.iter_mut() {
        let Some(Callback { endpoint, done, ctx }) = *cb else {
            continue;
        };
        let r = unsafe { (c.ops.transfer_take)(c.ctx, dev.hdev, endpoint) };
        if r == USB_EPENDING {
            continue;
        }
        *cb = None;
        return Some((done, ctx, r));
    }
    None
}

/// Completed queued transfers of every device: run their callbacks (the
/// USB thread), each with no lock held: a callback starts the next
/// transfer.
pub fn run_async_completions() {
    for ctrl in 0..MAX_CTRL as u8 {
        let Some(c) = crate::controller(ctrl) else {
            continue;
        };
        for i in 0..MAX_DEVICES {
            let Some(s) = slot_at(ctrl, i) else {
                continue;
            };
            while let Some((f, ctx, r)) = take_completion(c, s) {
                unsafe { f(ctx, r) };
            }
        }
    }
}

/// Recover a stalled endpoint (the host's `clear_halt`): the controller's
/// side, then CLEAR_FEATURE(ENDPOINT_HALT) on the endpoint.
pub fn clear_halt(c: &Controller, dev: &Device, endpoint: u8) -> i32 {
    let r = unsafe { (c.ops.clear_halt)(c.ctx, dev.hdev, endpoint) };
    if r < 0 {
        return r;
    }
    let r = control(c, dev, 0x02, 1, 0, u16::from(endpoint), core::ptr::null_mut(), 0);
    if r < 0 { r } else { 0 }
}

/// Enumerate the device on `root_port` (1-based) of controller `c`, or on
/// `port` of hub `parent`, at `speed`: the new device's id.
pub fn attach(c: &Controller, parent: Option<(u32, u8)>, root_port: u8, speed: u8) -> Result<u32, i32> {
    let ctrl = c.index;
    let mut path = UsbDevicePath { speed, root_port, port: root_port, ..UsbDevicePath::default() };
    let parent_id = match parent {
        None => 0,
        Some((hub_id, port)) => {
            let (hub_path, hub_hdev) = peek(hub_id, |hub| (hub.path, hub.hdev)).ok_or(USB_EGONE)?;
            if hub_path.depth >= MAX_DEPTH || port == 0 || port > 15 {
                return Err(USB_EIO);
            }
            path.root_port = hub_path.root_port;
            path.depth = hub_path.depth + 1;
            path.port = port;
            path.route = hub_path.route | (u32::from(port) << (4 * hub_path.depth));
            path.parent = hub_hdev;
            // The transaction translator: the first high-speed hub above a
            // slower device.
            (path.tt_hub, path.tt_port) = if hub_path.speed == USB_SPEED_HIGH && speed != USB_SPEED_HIGH {
                (hub_hdev, port)
            } else {
                (hub_path.tt_hub, hub_path.tt_port)
            };
            hub_id
        }
    };
    // A free entry, held until the device is enumerated.
    let (index, s, mut entry) = (0..MAX_DEVICES)
        .find_map(|i| {
            let s = slot_at(ctrl, i)?;
            let entry = s.dev.lock();
            entry.is_none().then_some((i, s, entry))
        })
        .ok_or(USB_EIO)?;
    path.address = index as u8 + 1;
    step("new device");
    let hdev = unsafe { (c.ops.device_new)(c.ctx, &path) };
    if hdev <= 0 {
        return Err(if hdev < 0 { hdev } else { USB_EIO });
    }
    let dev = Device {
        hdev: hdev as u32,
        info: UsbDeviceInfo {
            id: id_of(ctrl, index),
            parent: parent_id,
            speed,
            port: path.port,
            depth: path.depth,
            ..UsbDeviceInfo::default()
        },
        path,
        hub_ports: 0,
        interfaces: [const { None }; MAX_INTERFACES],
        callbacks: [None; MAX_CALLBACKS],
    };
    s.gone.store(false, Ordering::Release);
    let id = dev.info.id;
    let dev = entry.insert(dev);

    let enumerated = enumerate(c, dev);
    // The lock drops before the drivers see the device: `probe` calls in.
    drop(entry);
    match enumerated {
        Ok(()) => {
            offer(s);
            crate::proc_update();
            Ok(id)
        }
        Err(e) => {
            detach_id(c, id);
            Err(e)
        }
    }
}

/// Address the device, read its descriptors and configure its endpoints
/// (the slot's lock held throughout).
fn enumerate(c: &Controller, dev: &mut Device) -> Result<(), i32> {
    let hdev = dev.hdev;
    step("address device");
    let r = unsafe { (c.ops.device_address)(c.ctx, hdev) };
    if r < 0 {
        return Err(r);
    }
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
        step("control endpoint");
        let r = unsafe { (c.ops.device_ep0)(c.ctx, hdev, mps0) };
        if r < 0 {
            return Err(r);
        }
        step("device descriptor");
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

    // The endpoints of every interface, each once, to the controller.
    let mut eps = [UsbEndpoint::default(); MAX_INTERFACES * USB_MAX_ENDPOINTS];
    let mut n_eps = 0;
    for intf in dev.interfaces.iter().flatten() {
        for e in &intf.info.endpoints[..usize::from(intf.info.n_endpoints)] {
            if e.attributes & 3 == 1 || eps[..n_eps].iter().any(|k| k.address == e.address) {
                continue; // isochronous: not driven
            }
            eps[n_eps] = *e;
            n_eps += 1;
        }
    }
    step("configure endpoints");
    let r = unsafe { (c.ops.device_configure)(c.ctx, hdev, eps.as_ptr(), n_eps) };
    if r < 0 {
        return Err(r);
    }
    // SET_CONFIGURATION.
    step("set configuration");
    let config = dev.info.config;
    let r = control(c, dev, 0x00, 9, u16::from(config), 0, core::ptr::null_mut(), 0);
    if r < 0 {
        return Err(r);
    }
    Ok(())
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
                            max_packet: u16::from_le_bytes([d[4], d[5]]) & 0x7FF,
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

/// Offer every unclaimed interface of the device in `s` to the drivers,
/// with no lock held across a `probe`.
pub fn offer(s: &Slot) {
    let drivers = *DRIVERS.lock();
    for i in 0..MAX_INTERFACES {
        // The interface, if unclaimed, and the device, copied out.
        let unclaimed = {
            let dev = s.dev.lock();
            dev.as_ref().and_then(|dev| {
                let intf = dev.interfaces[i].as_ref()?;
                intf.driver.is_none().then_some((intf.info, dev.info))
            })
        };
        let Some((info, dinfo)) = unclaimed else {
            continue;
        };
        for (k, drv) in drivers.iter().enumerate() {
            let Some(drv) = drv else {
                continue;
            };
            if unsafe { (drv.probe)(&dinfo, &info) } == 0 {
                if let Some(intf) = s.dev.lock().as_mut().and_then(|dev| dev.interfaces[i].as_mut()) {
                    intf.driver = Some(k);
                }
                break;
            }
        }
    }
}

/// Every unclaimed interface of every device, to the drivers (a driver
/// registered after the devices were enumerated).
pub fn offer_all() {
    for ctrl in 0..MAX_CTRL as u8 {
        if crate::controller(ctrl).is_none() {
            continue;
        }
        for i in 0..MAX_DEVICES {
            let Some(s) = slot_at(ctrl, i) else {
                continue;
            };
            if s.dev.lock().is_some() && !s.gone.load(Ordering::Acquire) {
                offer(s);
            }
        }
    }
    crate::proc_update();
}

/// The first device of controller `ctrl` whose entry is there, not gone,
/// and passes `f`: its id.
fn find_by(ctrl: u8, f: impl Fn(&Device) -> bool) -> Option<u32> {
    (0..MAX_DEVICES).find_map(|i| {
        let s = slot_at(ctrl, i)?;
        let dev = s.dev.lock();
        let dev = dev.as_ref()?;
        (!s.gone.load(Ordering::Acquire) && f(dev)).then_some(dev.info.id)
    })
}

/// The device on `port` of `parent` (0: a root port).
pub fn find(ctrl: u8, parent: u32, port: u8) -> Option<u32> {
    find_by(ctrl, |d| d.info.parent == parent && d.info.port == port)
}

/// Tear `id` down: its children first (a hub), its transfers failed, its
/// drivers told, its controller state freed.
pub fn detach_id(c: &Controller, id: u32) {
    let ctrl = c.index;
    let Some(s) = slot_of(id) else {
        return;
    };
    // Children.
    while let Some(child) = find_by(ctrl, |d| d.info.parent == id) {
        detach_id(c, child);
    }
    // Gone, before the lock: no transfer starts from here on, and the
    // ones in flight fail now (`device_abort`), so whoever holds the lock
    // lets go of it (an empty entry has no transfer, and `attach` clears
    // the flag).
    s.gone.store(true, Ordering::Release);
    let hdev = {
        let dev = s.dev.lock();
        let Some(dev) = dev.as_ref() else {
            return;
        };
        dev.hdev
    };
    unsafe { (c.ops.device_abort)(c.ctx, hdev) };
    // A queued transfer completes like any other, with the error: the
    // driver hears before its `disconnect`, and the record is released.
    while let Some((f, ctx, r)) = take_completion(c, s) {
        unsafe { f(ctx, r) };
    }
    // The drivers that took its interfaces, told with the lock dropped
    // (they call back in).
    let claimed: [Option<(usize, u8)>; MAX_INTERFACES] = {
        let dev = s.dev.lock();
        let Some(dev) = dev.as_ref() else {
            return;
        };
        let mut claimed = [None; MAX_INTERFACES];
        for (i, intf) in dev.interfaces.iter().enumerate() {
            if let Some(intf) = intf {
                claimed[i] = intf.driver.map(|k| (k, intf.info.number));
            }
        }
        claimed
    };
    for (k, number) in claimed.into_iter().flatten() {
        let drv = DRIVERS.lock()[k];
        if let Some(drv) = drv {
            unsafe { (drv.disconnect)(id, number) };
        }
    }
    // The entry emptied, the controller's state freed.
    let mut entry = s.dev.lock();
    let Some(dev) = entry.take() else {
        return;
    };
    unsafe { (c.ops.device_free)(c.ctx, dev.hdev) };
    drop(entry);
    crate::proc_update();
}

/// `/proc/usb`: a line per controller, one per device.
pub fn proc_text(out: &mut [u8]) -> usize {
    let mut w = crate::Writer { buf: out, len: 0 };
    for ctrl in 0..MAX_CTRL as u8 {
        let Some(c) = crate::controller(ctrl) else {
            continue;
        };
        w.str("controller ");
        w.dec(u32::from(ctrl));
        w.str(" ports ");
        w.dec(u32::from(c.ports));
        w.str(" ");
        let n = w.len;
        let m = unsafe { (c.ops.describe)(c.ctx, w.buf[n..].as_mut_ptr(), w.buf.len() - n) };
        w.len = n + m.min(w.buf.len() - n);
        w.str("\n");
        for i in 0..MAX_DEVICES {
            let Some(s) = slot_at(ctrl, i) else {
                continue;
            };
            let dev = s.dev.lock();
            let Some(dev) = dev.as_ref() else {
                continue;
            };
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
                    if let Some(drv) = DRIVERS.lock()[k] {
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
            if s.gone.load(Ordering::Relaxed) {
                w.str(" gone");
            }
            w.str("\n");
        }
    }
    w.len
}
