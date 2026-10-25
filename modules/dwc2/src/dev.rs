//! The devices, as the controller needs them (`UsbHcdOps`, `docs/usb.md`):
//! a DWC2 keeps no per-device state in hardware, so a device is a record
//! here: its address, speed, the hub whose transaction translator it goes
//! through, and its endpoints with their data toggles. The core calls in
//! one call per device at a time; `device_abort` is the exception and
//! touches only the `gone` flag and the channels.

use core::sync::atomic::{AtomicBool, Ordering};

use myos_abi::{Lock, USB_EGONE, USB_EIO, USB_SPEED_HIGH, USB_SPEED_LOW, UsbDevicePath, UsbEndpoint};

/// Devices the controller drives at once.
pub const MAX_DEVICES: usize = 32;
/// Endpoints of a device besides endpoint 0 the record holds.
const MAX_ENDPOINTS: usize = 15;

#[derive(Clone, Copy)]
pub struct Endpoint {
    /// `bEndpointAddress` (the direction in bit 7).
    pub address: u8,
    /// Transfer type (`bmAttributes & 3`): 2 bulk, 3 interrupt.
    pub kind: u8,
    pub max_packet: u16,
    pub interval: u8,
    /// The next packet's data toggle: DATA1 when set.
    pub toggle: bool,
}

#[derive(Clone, Copy)]
pub struct Device {
    /// The address on the bus: 0 until `device_address`.
    pub address: u8,
    pub speed: u8,
    /// A low- or full-speed device behind a high-speed hub: the address of
    /// the hub whose transaction translator its transfers go through, and
    /// the port on it (split transactions).
    pub tt: Option<(u8, u8)>,
    pub ep0_max_packet: u16,
    pub eps: [Option<Endpoint>; MAX_ENDPOINTS],
}

/// A device table entry: the index + 1 is the handle the core knows the
/// device by.
pub struct Slot {
    /// Raised by `device_abort`: a transfer finding it set gives up.
    pub gone: AtomicBool,
    dev: Lock<Option<Device>>,
}

impl Slot {
    const EMPTY: Self = Self { gone: AtomicBool::new(false), dev: Lock::new(None) };
}

static DEVICES: [Slot; MAX_DEVICES] = [const { Slot::EMPTY }; MAX_DEVICES];

/// The table entry the handle `hdev` (table index + 1) names.
pub fn slot_of(hdev: u32) -> Option<&'static Slot> {
    if hdev == 0 {
        return None;
    }
    DEVICES.get(hdev as usize - 1)
}

/// A copy of the device `hdev`'s record: `USB_EGONE` when there is none or
/// the device is going.
pub fn get(hdev: u32) -> Result<Device, i32> {
    let s = slot_of(hdev).ok_or(USB_EGONE)?;
    if s.gone.load(Ordering::Acquire) {
        return Err(USB_EGONE);
    }
    s.dev.lock().ok_or(USB_EGONE)
}

/// Change the device `hdev`'s record.
pub fn update(hdev: u32, f: impl FnOnce(&mut Device)) -> Result<(), i32> {
    let s = slot_of(hdev).ok_or(USB_EGONE)?;
    let mut dev = s.dev.lock();
    let dev = dev.as_mut().ok_or(USB_EGONE)?;
    f(dev);
    Ok(())
}

fn ep0_max_packet(speed: u8) -> u16 {
    match speed {
        USB_SPEED_LOW => 8,
        USB_SPEED_HIGH => 64,
        _ => 8,
    }
}

/// A device at `path` (the core's `device_new`): a record, not yet
/// addressed; the handle.
pub fn new(path: &UsbDevicePath) -> Result<u32, i32> {
    let tt = if path.tt_hub != 0 { Some((get(path.tt_hub)?.address, path.tt_port)) } else { None };
    let (index, mut entry) = (0..MAX_DEVICES)
        .find_map(|i| {
            let entry = DEVICES[i].dev.lock();
            entry.is_none().then_some((i, entry))
        })
        .ok_or(USB_EIO)?;
    *entry = Some(Device {
        address: 0,
        speed: path.speed,
        tt,
        ep0_max_packet: ep0_max_packet(path.speed),
        eps: [None; MAX_ENDPOINTS],
    });
    DEVICES[index].gone.store(false, Ordering::Release);
    Ok(index as u32 + 1)
}

/// The endpoints of the configuration about to be set (the core's
/// `device_configure`): recorded, each with its toggle at DATA0.
pub fn configure(hdev: u32, eps: &[UsbEndpoint]) -> Result<(), i32> {
    update(hdev, |dev| {
        dev.eps = [None; MAX_ENDPOINTS];
        let mut n = 0;
        for e in eps {
            if e.attributes & 3 == 1 || n == MAX_ENDPOINTS {
                continue;
            }
            dev.eps[n] = Some(Endpoint {
                address: e.address,
                kind: e.attributes & 3,
                max_packet: e.max_packet & 0x7FF,
                interval: e.interval,
                toggle: false,
            });
            n += 1;
        }
    })
}

/// An interface's alternate setting changed (the core's
/// `endpoints_change`): the endpoints `drop` names forgotten, the
/// non-isochronous ones of `add` recorded (isochronous transfers are not
/// driven: `isoch_queue` refuses them).
pub fn endpoints_change(hdev: u32, add: &[UsbEndpoint], drop: &[u8]) -> Result<(), i32> {
    update(hdev, |dev| {
        for e in dev.eps.iter_mut() {
            if e.is_some_and(|e| drop.contains(&e.address)) {
                *e = None;
            }
        }
        for e in add.iter().filter(|e| e.attributes & 3 != 1) {
            if let Some(free) = dev.eps.iter_mut().find(|x| x.is_none()) {
                *free = Some(Endpoint {
                    address: e.address,
                    kind: e.attributes & 3,
                    max_packet: e.max_packet & 0x7FF,
                    interval: e.interval,
                    toggle: false,
                });
            }
        }
    })
}

/// The device is going (the core's `device_abort`).
pub fn abort(hdev: u32) {
    if let Some(s) = slot_of(hdev) {
        s.gone.store(true, Ordering::Release);
    }
}

/// Forget the device (the core's `device_free`).
pub fn free(hdev: u32) {
    if let Some(s) = slot_of(hdev) {
        *s.dev.lock() = None;
    }
}

impl Device {
    pub fn endpoint(&self, address: u8) -> Option<Endpoint> {
        self.eps.iter().flatten().find(|e| e.address == address).copied()
    }
}

/// Set endpoint `address`'s toggle (after a transfer moved `packets`
/// packets, or to DATA0 after `clear_halt`).
pub fn set_toggle(hdev: u32, address: u8, toggle: bool) {
    let _ = update(hdev, |dev| {
        if let Some(e) = dev.eps.iter_mut().flatten().find(|e| e.address == address) {
            e.toggle = toggle;
        }
    });
}
