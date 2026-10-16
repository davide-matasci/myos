//! USB HID class driver (`docs/usb.md`): mice and tablets, into the
//! console's pointer (`/dev/console/mouse`, `docs/tty.md`) through its
//! [`POINTER_SERVICE`]. It takes an interface of class 3 whose report
//! descriptor ([`report`]) has an X and a Y axis: a mouse's are relative,
//! a tablet's (QEMU's `usb-tablet`) absolute, scaled to the pointer's
//! `0..=POINTER_ABS_MAX`. A boot mouse whose descriptor cannot be read or
//! parsed is switched to the boot protocol and its fixed report. Keyboards
//! are not taken: the console's are PS/2 and virtio-input.
//!
//! The device reports on its interrupt IN endpoint: one transfer is always
//! queued (`interrupt_start`) and its completion, on the host's USB
//! thread, hands the changes to the pointer and queues the next. Like the
//! hub driver, everything here runs on that thread, so the table's lock is
//! never contended.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

mod report;

use core::ffi::c_void;
use core::sync::atomic::{AtomicPtr, AtomicU8, Ordering};

use myos_abi::{
    ABI_VERSION, ApiCell, BTN_EXTRA, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, BTN_SIDE, KernelApi, Lock, POINTER_ABS_MAX,
    POINTER_SERVICE, POINTER_VERSION, PointerOps, StrRef, USB_EGONE, USB_ESTALL, USB_HOST_VERSION, USB_SERVICE,
    UsbDeviceInfo, UsbDriverOps, UsbHostOps, UsbInterfaceInfo, status_fail, status_ok,
};

use report::{Layout, MAX_BUTTONS};

const MAX_POINTERS: usize = 8;
const CLASS_HID: u8 = 3;
const SUBCLASS_BOOT: u8 = 1;
const PROTOCOL_MOUSE: u8 = 2;
/// The bytes of a report transfer (the host's interrupt bounce buffer).
const REPORT_MAX: usize = 64;

// Requests: the standard GET_DESCRIPTOR (of the interface: its HID report
// descriptor) and the HID class's SET_IDLE and SET_PROTOCOL.
const GET_DESCRIPTOR: u8 = 6;
const SET_IDLE: u8 = 0x0A;
const SET_PROTOCOL: u8 = 0x0B;
const RT_INTERFACE_IN: u8 = 0x81;
const RT_CLASS_OUT: u8 = 0x21;
const DESC_REPORT: u16 = 0x22;

/// Button usages 1 to 5 as the pointer's codes.
const BUTTONS: [u16; MAX_BUTTONS] = [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE, BTN_SIDE, BTN_EXTRA];

/// A pointer, as the hooks copy it out of [`POINTERS`].
#[derive(Clone, Copy)]
struct Pointer {
    dev: u32,
    intf: u8,
    ep: u8,
    len: usize,
    layout: Layout,
    /// The buttons held, bit `n` for usage `n + 1`.
    buttons: u8,
    /// A tablet's last position: every report repeats it.
    position: Option<(i32, i32)>,
    /// Transfers failed in a row.
    errors: u8,
}

static API: ApiCell = ApiCell::new();
/// The host's and the console's tables, kept by `module_init`.
static HOST: AtomicPtr<UsbHostOps> = AtomicPtr::new(core::ptr::null_mut());
static POINTER: AtomicPtr<PointerOps> = AtomicPtr::new(core::ptr::null_mut());
static POINTERS: Lock<[Option<Pointer>; MAX_POINTERS]> = Lock::new([None; MAX_POINTERS]);
/// Each pointer's report: the host writes it when the transfer completes,
/// outside any lock of this module, so it is atomics.
static REPORTS: [[AtomicU8; REPORT_MAX]; MAX_POINTERS] =
    [const { [const { AtomicU8::new(0) }; REPORT_MAX] }; MAX_POINTERS];

static DRIVER: UsbDriverOps = UsbDriverOps {
    name: StrRef { ptr: b"usb_hid".as_ptr(), len: 7 },
    probe,
    disconnect,
};

fn api() -> &'static KernelApi {
    API.get()
}

fn host() -> &'static UsbHostOps {
    // SAFETY: the host's service table, which outlives this module.
    unsafe { HOST.load(Ordering::Acquire).as_ref() }.expect("usb_hid: host")
}

fn pointer() -> &'static PointerOps {
    // SAFETY: the console's service table, which outlives this module.
    unsafe { POINTER.load(Ordering::Acquire).as_ref() }.expect("usb_hid: pointer")
}

fn pointer_at(i: usize) -> Option<Pointer> {
    *POINTERS.lock().get(i)?
}

fn sleep_ms(ms: u64) {
    let until = api().monotonic_ns() + ms * 1_000_000;
    api().task_sleep_until(until);
}

/// `v` from the field's logical range to the pointer's absolute one.
fn scale(v: i32, min: i32, max: i32) -> i32 {
    if max <= min {
        return 0;
    }
    let v = i64::from(v.clamp(min, max)) - i64::from(min);
    (v * i64::from(POINTER_ABS_MAX) / (i64::from(max) - i64::from(min))) as i32
}

/// Hand what report `r` says to the pointer, and note in `p` the buttons
/// held now and a tablet's position.
fn deliver(p: &mut Pointer, r: &[u8]) {
    let l = &p.layout;
    let r = if l.id != 0 {
        match r.split_first() {
            Some((&id, rest)) if id == l.id => rest,
            // Another report of the device (its keyboard's, say).
            _ => return,
        }
    } else {
        r
    };
    let out = pointer();
    if let (Some(x), Some(y)) = (l.x.get(r), l.y.get(r)) {
        if l.x.relative {
            out.motion(x, y);
        } else {
            let at = (scale(x, l.x.min, l.x.max), scale(y, l.y.min, l.y.max));
            if p.position != Some(at) {
                out.absolute(at.0, at.1);
                p.position = Some(at);
            }
        }
    }
    if let Some(dz) = l.wheel.and_then(|w| w.get(r)) {
        out.wheel(dz);
    }
    let mut held = 0u8;
    for (n, b) in l.buttons.iter().enumerate() {
        if b.and_then(|b| b.get(r)).is_some_and(|v| v != 0) {
            held |= 1 << n;
        }
    }
    for (n, code) in BUTTONS.iter().enumerate() {
        if (held ^ p.buttons) & (1 << n) != 0 {
            out.button(*code, held & (1 << n) != 0);
        }
    }
    p.buttons = held;
}

/// Queue the report transfer of the pointer in slot `i`.
fn arm(i: usize) {
    let Some(p) = pointer_at(i) else {
        return;
    };
    // The report's bytes: the atomics are contiguous, and the host writes
    // them only before `report_done` runs.
    let buf = REPORTS[i][0].as_ptr();
    let r = unsafe { (host().interrupt_start)(p.dev, p.ep, buf, p.len, report_done, i as *mut c_void) };
    if r < 0 && r != USB_EGONE {
        status_fail(api(), "usb_hid: report endpoint");
    }
}

/// A report arrived, or the transfer failed (the USB thread).
unsafe extern "C" fn report_done(ctx: *mut c_void, status: i32) {
    let i = ctx as usize;
    if status == USB_EGONE {
        return;
    }
    let Some(mut p) = pointer_at(i) else {
        return;
    };
    if status >= 0 {
        let n = (status as usize).min(p.len);
        let mut r = [0u8; REPORT_MAX];
        for (b, a) in r[..n].iter_mut().zip(&REPORTS[i]) {
            *b = a.load(Ordering::Acquire);
        }
        deliver(&mut p, &r[..n]);
        p.errors = 0;
    } else {
        if status == USB_ESTALL {
            let _ = host().clear_halt(p.dev, p.ep);
        }
        p.errors += 1;
        if p.errors >= 5 {
            status_fail(api(), "usb_hid: the device stopped reporting");
            return;
        }
        sleep_ms(100);
    }
    let mut table = POINTERS.lock();
    if let Some(slot) = table[i].as_mut() {
        slot.buttons = p.buttons;
        slot.position = p.position;
        slot.errors = p.errors;
    } else {
        return;
    }
    drop(table);
    arm(i);
}

/// The interface's report descriptor, parsed.
fn report_layout(dev: u32, intf: u8) -> Option<Layout> {
    let mut d = [0u8; 512];
    let n = host().control(dev, RT_INTERFACE_IN, GET_DESCRIPTOR, DESC_REPORT << 8, u16::from(intf), &mut d);
    let n = usize::try_from(n).ok()?;
    report::parse(&d[..n.min(d.len())])
}

unsafe extern "C" fn probe(dev: *const UsbDeviceInfo, intf: *const UsbInterfaceInfo) -> i32 {
    let (dev, intf) = unsafe { (&*dev, &*intf) };
    if intf.class != CLASS_HID {
        return -1;
    }
    let Some(ep) = intf.endpoints[..usize::from(intf.n_endpoints)]
        .iter()
        .find(|e| e.attributes & 3 == 3 && e.address & 0x80 != 0)
        .copied()
    else {
        return -1;
    };
    let boot_mouse = intf.subclass == SUBCLASS_BOOT && intf.protocol == PROTOCOL_MOUSE;
    let index = u16::from(intf.number);
    let (layout, boot) = match report_layout(dev.id, intf.number) {
        Some(l) => (l, false),
        None if boot_mouse => (Layout::boot_mouse(), true),
        None => return -1,
    };
    // A boot device starts in the report protocol, but say so; then the
    // boot protocol if that is what it speaks here. Reports only when
    // something changed (idle 0). Either may stall: optional requests.
    if intf.subclass == SUBCLASS_BOOT {
        let _ = host().control(dev.id, RT_CLASS_OUT, SET_PROTOCOL, u16::from(!boot), index, &mut []);
    }
    let _ = host().control(dev.id, RT_CLASS_OUT, SET_IDLE, 0, index, &mut []);
    let pointer = Pointer {
        dev: dev.id,
        intf: intf.number,
        ep: ep.address,
        len: usize::from(ep.max_packet).clamp(1, REPORT_MAX),
        layout,
        buttons: 0,
        position: None,
        errors: 0,
    };
    let slot = {
        let mut table = POINTERS.lock();
        let Some(slot) = table.iter().position(|p| p.is_none()) else {
            return -1;
        };
        table[slot] = Some(pointer);
        slot
    };
    let label: &[u8] = if layout.x.relative { b"mouse" } else { b"tablet" };
    let _ = host().interface_label(dev.id, intf.number, label);
    arm(slot);
    0
}

unsafe extern "C" fn disconnect(dev: u32, intf: u8) {
    let mut released = 0u8;
    for p in POINTERS.lock().iter_mut() {
        if p.is_some_and(|p| p.dev == dev && p.intf == intf) {
            // The host failed the pending transfer (`report_done` got
            // `USB_EGONE` and did not re-arm).
            released |= p.map_or(0, |p| p.buttons);
            *p = None;
        }
    }
    // A button held while the device went away is up now.
    for (n, code) in BUTTONS.iter().enumerate() {
        if released & (1 << n) != 0 {
            pointer().button(*code, false);
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
    let ptr_table = api.service_lookup(POINTER_SERVICE) as *const PointerOps;
    if ptr_table.is_null() || unsafe { (*ptr_table).version } != POINTER_VERSION {
        api.write_str("usb_hid: no pointer\n");
        return -3;
    }
    POINTER.store(ptr_table.cast_mut(), Ordering::Release);
    let table = api.service_lookup(USB_SERVICE) as *const UsbHostOps;
    if table.is_null() {
        api.write_str("usb_hid: no usb host\n");
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
    status_ok(api, "usb_hid");
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    let api = api();
    status_fail(api, "usb_hid: panic");
    loop {
        api.task_yield();
    }
}
