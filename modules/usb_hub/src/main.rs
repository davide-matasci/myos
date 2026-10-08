//! USB hub class driver (`docs/usb.md`): a hub is a USB device of class 9
//! whose job is more ports. This module takes every hub interface the host
//! offers, powers the ports, resets the one a device appears on, and hands
//! the child to the host (`hub_attach`); the hub's status-change interrupt
//! endpoint tells it about connects and disconnects from then on. All of
//! this runs on the host's USB thread (probe, the completion callback),
//! so the hub table's lock is never contended; it is taken only around
//! the table itself, never across a host call, since `hub_attach`
//! enumerates the child on this same thread and a nested hub's `probe`
//! comes back here.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicPtr, AtomicU8, Ordering};

use myos_abi::{
    ABI_VERSION, ApiCell, KernelApi, Lock, StrRef, USB_EGONE, USB_HOST_VERSION, USB_SERVICE,
    USB_SPEED_FULL, USB_SPEED_HIGH, USB_SPEED_LOW, USB_SPEED_SUPER, UsbDeviceInfo, UsbDriverOps,
    UsbHostOps, UsbInterfaceInfo, status_fail, status_ok,
};

const MAX_HUBS: usize = 8;
const CLASS_HUB: u8 = 9;

// Standard requests and the hub class's features.
const GET_STATUS: u8 = 0;
const CLEAR_FEATURE: u8 = 1;
const SET_FEATURE: u8 = 3;
const GET_DESCRIPTOR: u8 = 6;
const PORT_RESET: u16 = 4;
const PORT_POWER: u16 = 8;
const C_PORT_CONNECTION: u16 = 16;
const C_PORT_ENABLE: u16 = 17;
const C_PORT_SUSPEND: u16 = 18;
const C_PORT_OVER_CURRENT: u16 = 19;
const C_PORT_RESET: u16 = 20;
/// `bmRequestType`: class request to the hub (device) / to a port (other).
const RT_HUB_IN: u8 = 0xA0;
const RT_PORT_IN: u8 = 0xA3;
const RT_PORT_OUT: u8 = 0x23;

/// A hub, as the hooks copy it out of [`HUBS`]: a hub that is gone is not
/// in the table.
#[derive(Clone, Copy)]
struct Hub {
    dev: u32,
    intf: u8,
    ports: u8,
    super_speed: bool,
    /// The status-change interrupt endpoint and the length of its report
    /// (one bit per port, bit 0 the hub itself; the bytes in [`REPORTS`]).
    status_ep: u8,
    report_len: usize,
}

static API: ApiCell = ApiCell::new();
/// The host's table, kept by `module_init`.
static HOST: AtomicPtr<UsbHostOps> = AtomicPtr::new(core::ptr::null_mut());
static HUBS: Lock<[Option<Hub>; MAX_HUBS]> = Lock::new([None; MAX_HUBS]);
/// Each hub's status-change report: the host writes it when the transfer
/// completes, outside any lock of this module, so it is atomics.
static REPORTS: [[AtomicU8; 8]; MAX_HUBS] = [const { [const { AtomicU8::new(0) }; 8] }; MAX_HUBS];

static DRIVER: UsbDriverOps = UsbDriverOps {
    name: StrRef { ptr: b"usb_hub".as_ptr(), len: 7 },
    probe,
    disconnect,
};

fn api() -> &'static KernelApi {
    API.get()
}

fn host() -> &'static UsbHostOps {
    // SAFETY: the host's service table, which outlives this module.
    unsafe { HOST.load(Ordering::Acquire).as_ref() }.expect("usb_hub: host")
}

/// The hub in slot `i`, if any (a copy).
fn hub_at(i: usize) -> Option<Hub> {
    *HUBS.lock().get(i)?
}

fn sleep_ms(ms: u64) {
    let until = api().monotonic_ns() + ms * 1_000_000;
    api().task_sleep_until(until);
}

fn control(dev: u32, rt: u8, req: u8, value: u16, index: u16, data: &mut [u8]) -> i32 {
    host().control(dev, rt, req, value, index, data)
}

fn set_port_feature(dev: u32, port: u8, feature: u16) -> i32 {
    control(dev, RT_PORT_OUT, SET_FEATURE, feature, u16::from(port), &mut [])
}

fn clear_port_feature(dev: u32, port: u8, feature: u16) -> i32 {
    control(dev, RT_PORT_OUT, CLEAR_FEATURE, feature, u16::from(port), &mut [])
}

/// `wPortStatus`, `wPortChange` of `port`.
fn port_status(dev: u32, port: u8) -> Option<(u16, u16)> {
    let mut s = [0u8; 4];
    if control(dev, RT_PORT_IN, GET_STATUS, 0, u16::from(port), &mut s) < 4 {
        return None;
    }
    Some((u16::from_le_bytes([s[0], s[1]]), u16::from_le_bytes([s[2], s[3]])))
}

/// A device appeared on `port`: debounce, reset, tell the host its speed.
fn connect(hub: &Hub, port: u8) {
    sleep_ms(100);
    let Some((status, _)) = port_status(hub.dev, port) else {
        return;
    };
    if status & 1 == 0 {
        return;
    }
    if set_port_feature(hub.dev, port, PORT_RESET) < 0 {
        return;
    }
    let mut enabled = None;
    for _ in 0..50 {
        sleep_ms(10);
        let Some((status, change)) = port_status(hub.dev, port) else {
            return;
        };
        if change & (1 << 4) != 0 {
            let _ = clear_port_feature(hub.dev, port, C_PORT_RESET);
            enabled = Some(status);
            break;
        }
    }
    let Some(status) = enabled else {
        status_fail(api(), "usb_hub: port reset did not complete");
        return;
    };
    if status & 1 == 0 || status & 2 == 0 {
        return;
    }
    let speed = if hub.super_speed {
        USB_SPEED_SUPER
    } else if status & (1 << 9) != 0 {
        USB_SPEED_LOW
    } else if status & (1 << 10) != 0 {
        USB_SPEED_HIGH
    } else {
        USB_SPEED_FULL
    };
    let r = host().hub_attach(hub.dev, port, speed);
    if r < 0 {
        status_fail(api(), "usb_hub: a device did not enumerate");
    }
}

/// What changed on `port`, per its change bits.
fn port_changed(hub: &Hub, port: u8) {
    let Some((status, change)) = port_status(hub.dev, port) else {
        return;
    };
    for (bit, feature) in [(1u16, C_PORT_ENABLE), (2, C_PORT_SUSPEND), (3, C_PORT_OVER_CURRENT), (4, C_PORT_RESET)] {
        if change & (1 << bit) != 0 {
            let _ = clear_port_feature(hub.dev, port, feature);
        }
    }
    if change & 1 != 0 {
        let _ = clear_port_feature(hub.dev, port, C_PORT_CONNECTION);
        if status & 1 != 0 {
            host().hub_detach(hub.dev, port);
            connect(hub, port);
        } else {
            host().hub_detach(hub.dev, port);
        }
    }
}

/// Queue the status-change transfer of the hub in slot `i`.
fn arm(i: usize) {
    let Some(hub) = hub_at(i) else {
        return;
    };
    // The report's bytes: the atomics are contiguous, and the host writes
    // them only before `status_done` runs.
    let report = REPORTS[i][0].as_ptr();
    let r = unsafe { (host().interrupt_start)(hub.dev, hub.status_ep, report, hub.report_len, status_done, i as *mut c_void) };
    if r < 0 && r != USB_EGONE {
        status_fail(api(), "usb_hub: status endpoint");
    }
}

/// The status-change report arrived (the USB thread).
unsafe extern "C" fn status_done(ctx: *mut c_void, status: i32) {
    let i = ctx as usize;
    let Some(hub) = hub_at(i) else {
        return;
    };
    if status == USB_EGONE {
        return;
    }
    if status > 0 {
        let report = REPORTS[i].each_ref().map(|b| b.load(Ordering::Acquire));
        for port in 1..=hub.ports {
            let byte = usize::from(port / 8);
            if byte < report.len() && report[byte] & (1 << (port % 8)) != 0 {
                // `port_changed` may detach and attach: the hub itself may
                // be gone by now.
                let Some(hub) = hub_at(i) else {
                    return;
                };
                port_changed(&hub, port);
            }
        }
    }
    if hub_at(i).is_some() {
        arm(i);
    }
}

unsafe extern "C" fn probe(dev: *const UsbDeviceInfo, intf: *const UsbInterfaceInfo) -> i32 {
    let (dev, intf) = unsafe { (&*dev, &*intf) };
    if intf.class != CLASS_HUB {
        return -1;
    }
    let Some(status_ep) = intf.endpoints[..usize::from(intf.n_endpoints)]
        .iter()
        .find(|e| e.attributes & 3 == 3 && e.address & 0x80 != 0)
        .map(|e| e.address)
    else {
        return -1;
    };
    let Some(slot) = HUBS.lock().iter().position(|h| h.is_none()) else {
        return -1;
    };
    let super_speed = dev.speed == USB_SPEED_SUPER;
    // The hub descriptor: port count, characteristics, power-on delay.
    let mut d = [0u8; 16];
    let n = control(dev.id, RT_HUB_IN, GET_DESCRIPTOR, if super_speed { 0x2A00 } else { 0x2900 }, 0, &mut d);
    if n < 7 {
        status_fail(api(), "usb_hub: no hub descriptor");
        return -1;
    }
    let ports = d[2].min(15);
    let characteristics = u16::from_le_bytes([d[3], d[4]]);
    let tt_think = ((characteristics >> 5) & 3) as u8;
    let multi_tt = u8::from(dev.protocol == 2);
    let power_delay_ms = u64::from(d[5]) * 2;
    if host().hub_configure(dev.id, ports, tt_think, multi_tt) < 0 {
        status_fail(api(), "usb_hub: hub slot");
        return -1;
    }
    HUBS.lock()[slot] = Some(Hub {
        dev: dev.id,
        intf: intf.number,
        ports,
        super_speed,
        status_ep,
        report_len: (usize::from(ports) + 8) / 8,
    });
    for port in 1..=ports {
        let _ = set_port_feature(dev.id, port, PORT_POWER);
    }
    sleep_ms(power_delay_ms + 100);
    // The devices already plugged in.
    for port in 1..=ports {
        // A child's enumeration may have detached this hub meanwhile.
        let Some(hub) = hub_at(slot) else {
            return 0;
        };
        if let Some((status, change)) = port_status(dev.id, port) {
            if change & 1 != 0 {
                let _ = clear_port_feature(dev.id, port, C_PORT_CONNECTION);
            }
            if status & 1 != 0 {
                connect(&hub, port);
            }
        }
    }
    if hub_at(slot).is_some() {
        arm(slot);
    }
    0
}

unsafe extern "C" fn disconnect(dev: u32, intf: u8) {
    for h in HUBS.lock().iter_mut() {
        if h.is_some_and(|h| h.dev == dev && h.intf == intf) {
            // The host detached the children already and failed the
            // pending status transfer (`status_done` got `USB_EGONE` and
            // did not re-arm).
            *h = None;
        }
    }
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
    let table = api.service_lookup(USB_SERVICE) as *const UsbHostOps;
    if table.is_null() {
        api.write_str("usb_hub: no usb host\n");
        return -3;
    }
    let host: &'static UsbHostOps = unsafe { &*table };
    if host.version != USB_HOST_VERSION {
        return -4;
    }
    HOST.store(table.cast_mut(), Ordering::Release);
    if host.driver_register(&DRIVER) != 0 {
        return -5;
    }
    status_ok(api, "usb_hub");
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    let api = api();
    status_fail(api, "usb_hub: panic");
    loop {
        api.task_yield();
    }
}
