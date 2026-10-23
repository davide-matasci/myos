//! xHCI: a USB host controller driver module (`docs/usb.md`).
//!
//! Finds the controllers by PCI class (`0x0C/0x03`, programming interface
//! `0x30`), brings them up (`hc`) and registers each with the USB core
//! (the `usb` module's [`USB_CORE_SERVICE`]), which owns the devices and
//! their enumeration: this module does the controller's part through the
//! [`UsbHcdOps`] it hands the core (`dev`: slots, contexts, rings, the
//! transfers). The interrupt handler only consumes the event ring
//! (`hc::process_events`) and tells the core what changed.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

mod dev;
mod dma;
mod hc;
mod sync;

use core::ffi::c_void;
use core::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

use myos_abi::{
    ABI_VERSION, ApiCell, KernelApi, MYOS_IRQ_INTX, StrRef, USB_CORE_SERVICE, USB_CORE_VERSION, USB_EGONE,
    USB_EIO, USB_HCD_VERSION, UsbCoreOps, UsbDevicePath, UsbEndpoint, UsbHcdOps, UsbPortStatus, status_fail,
    status_ok,
};

static API: ApiCell = ApiCell::new();
/// The core's table, kept by `module_init`.
static CORE: AtomicPtr<UsbCoreOps> = AtomicPtr::new(core::ptr::null_mut());
/// The controllers, each set once by `probe` and read by every context
/// after (`controller`).
static CONTROLLERS: [sync::Once<hc::Controller>; hc::MAX_CTRL] = [const { sync::Once::new() }; hc::MAX_CTRL];

pub fn api() -> &'static KernelApi {
    API.get()
}

fn core() -> &'static UsbCoreOps {
    // SAFETY: the core's service table, which outlives this module (a
    // lookup keeps the core loaded).
    unsafe { CORE.load(Ordering::Acquire).as_ref() }.expect("xhci: usb core")
}

pub fn controller(i: u8) -> Option<&'static hc::Controller> {
    CONTROLLERS.get(usize::from(i))?.get()
}

/// Sleep the calling task `ms` milliseconds.
pub fn sleep_ms(ms: u64) {
    let until = api().monotonic_ns() + ms * 1_000_000;
    api().task_sleep_until(until);
}

/// A completion the core's thread takes (`hc::handle_event`).
pub fn core_wake(c: &hc::Controller) {
    let index = c.core_index.load(Ordering::Acquire);
    if index != u32::MAX {
        core().wake(index);
    }
}

/// Root port `port` (0-based) changed (`hc::handle_event`).
pub fn core_port_change(c: &hc::Controller, port: u8) {
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

// ---- The controller driver's table ---------------------------------------------

static HCD_OPS: UsbHcdOps = UsbHcdOps {
    version: USB_HCD_VERSION,
    name: StrRef { ptr: b"xhci".as_ptr(), len: 4 },
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
    interrupt_start,
    interrupt_take,
    clear_halt,
};

/// `dev::with_device` for a hook: the device's lock held for `f`, and
/// `USB_EGONE` when the controller or the device is not there.
fn on_device(ctrl: usize, hdev: u32, f: impl FnOnce(&hc::Controller, &dev::Slot, &mut dev::Device) -> i32) -> i32 {
    let Some(c) = u8::try_from(ctrl).ok().and_then(controller) else {
        return USB_EGONE;
    };
    dev::with_device(c, hdev, |s, d| f(c, s, d)).unwrap_or_else(|e| e)
}

fn result(r: Result<(), i32>) -> i32 {
    match r {
        Ok(()) => 0,
        Err(e) => e,
    }
}

unsafe extern "C" fn ports(ctrl: usize) -> u8 {
    u8::try_from(ctrl).ok().and_then(controller).map_or(0, |c| c.ports)
}

unsafe extern "C" fn port_status(ctrl: usize, port: u8, out: *mut UsbPortStatus) -> i32 {
    let Some(c) = u8::try_from(ctrl).ok().and_then(controller) else {
        return USB_EGONE;
    };
    if out.is_null() || port >= c.ports {
        return USB_EIO;
    }
    let s = hc::port_status(c, usize::from(port));
    hc::port_ack(c, usize::from(port));
    unsafe {
        *out = UsbPortStatus { connected: u8::from(s.connected), enabled: u8::from(s.enabled), speed: dev::speed_from_psi(s.speed) };
    }
    0
}

/// Reset a USB 2 port; a USB 3 port whose link trained is enabled already.
unsafe extern "C" fn port_reset(ctrl: usize, port: u8) -> i32 {
    let Some(c) = u8::try_from(ctrl).ok().and_then(controller) else {
        return USB_EGONE;
    };
    if port >= c.ports {
        return USB_EIO;
    }
    let p = usize::from(port);
    let s = hc::port_status(c, p);
    if (s.enabled && c.port_major[p.min(63)] == 3) || hc::port_reset(c, p) { 0 } else { USB_EIO }
}

/// Without an interrupt the event ring is consumed here, every pass of the
/// core's thread.
unsafe extern "C" fn poll(ctrl: usize) -> i32 {
    let Some(c) = u8::try_from(ctrl).ok().and_then(controller) else {
        return 0;
    };
    if c.irq_on.load(Ordering::Relaxed) {
        return 0;
    }
    hc::process_events(c);
    1
}

unsafe extern "C" fn describe(ctrl: usize, buf: *mut u8, len: usize) -> usize {
    let Some(c) = u8::try_from(ctrl).ok().and_then(controller) else {
        return 0;
    };
    if buf.is_null() {
        return 0;
    }
    let buf = unsafe { core::slice::from_raw_parts_mut(buf, len) };
    let mut w = Writer { buf, len: 0 };
    w.str(if c.irq_on.load(Ordering::Relaxed) { "irq on" } else { "irq off" });
    w.str(" events ");
    w.dec(c.events.load(Ordering::Relaxed));
    w.str(" irqs ");
    w.dec(IRQS.load(Ordering::Relaxed));
    w.len
}

unsafe extern "C" fn device_new(ctrl: usize, path: *const UsbDevicePath) -> i32 {
    let Some(c) = u8::try_from(ctrl).ok().and_then(controller) else {
        return USB_EGONE;
    };
    if path.is_null() {
        return USB_EIO;
    }
    match dev::device_new(c, unsafe { &*path }) {
        Ok(hdev) => hdev as i32,
        Err(e) => e,
    }
}

unsafe extern "C" fn device_address(ctrl: usize, hdev: u32) -> i32 {
    on_device(ctrl, hdev, |c, _, d| result(dev::device_address(c, d)))
}

unsafe extern "C" fn device_ep0(ctrl: usize, hdev: u32, max_packet: u16) -> i32 {
    on_device(ctrl, hdev, |c, _, d| result(dev::device_ep0(c, d, max_packet)))
}

unsafe extern "C" fn device_configure(ctrl: usize, hdev: u32, eps: *const UsbEndpoint, n: usize) -> i32 {
    if n != 0 && eps.is_null() {
        return USB_EIO;
    }
    let eps: &[UsbEndpoint] = if n == 0 { &[] } else { unsafe { core::slice::from_raw_parts(eps, n) } };
    on_device(ctrl, hdev, |c, s, d| result(dev::device_configure(c, s, d, eps)))
}

unsafe extern "C" fn device_hub(ctrl: usize, hdev: u32, ports: u8, tt_think: u8, multi_tt: u8) -> i32 {
    on_device(ctrl, hdev, |c, _, d| dev::device_hub(c, d, ports, tt_think, multi_tt != 0))
}

unsafe extern "C" fn device_abort(ctrl: usize, hdev: u32) {
    if let Some(c) = u8::try_from(ctrl).ok().and_then(controller) {
        dev::device_abort(c, hdev);
    }
}

unsafe extern "C" fn device_free(ctrl: usize, hdev: u32) {
    if let Some(c) = u8::try_from(ctrl).ok().and_then(controller) {
        dev::device_free(c, hdev);
    }
}

unsafe extern "C" fn control(ctrl: usize, hdev: u32, request_type: u8, request: u8, value: u16, index: u16, data: *mut u8, len: u16) -> i32 {
    if len != 0 && data.is_null() {
        return USB_EIO;
    }
    on_device(ctrl, hdev, |c, s, d| dev::control(c, s, d, request_type, request, value, index, data, len))
}

unsafe extern "C" fn bulk(ctrl: usize, hdev: u32, endpoint: u8, data: *mut u8, len: usize, timeout_ms: u32) -> i32 {
    if data.is_null() && len != 0 {
        return USB_EIO;
    }
    on_device(ctrl, hdev, |c, s, d| dev::bulk(c, s, d, endpoint, data, len, timeout_ms))
}

unsafe extern "C" fn interrupt_start(ctrl: usize, hdev: u32, endpoint: u8, data: *mut u8, len: usize) -> i32 {
    if data.is_null() || len == 0 {
        return USB_EIO;
    }
    on_device(ctrl, hdev, |c, s, d| dev::interrupt_start(c, s, d, endpoint, data, len))
}

unsafe extern "C" fn interrupt_take(ctrl: usize, hdev: u32, endpoint: u8) -> i32 {
    on_device(ctrl, hdev, |c, s, d| dev::interrupt_take(c, s, d, endpoint))
}

unsafe extern "C" fn clear_halt(ctrl: usize, hdev: u32, endpoint: u8) -> i32 {
    on_device(ctrl, hdev, |c, _, d| dev::clear_halt(c, d, endpoint))
}

// ---- Interrupts and probing -------------------------------------------------

static IRQS: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn irq_handler(ctx: *mut c_void) {
    IRQS.fetch_add(1, Ordering::Relaxed);
    let Some(c) = controller(ctx as usize as u8) else {
        return;
    };
    hc::process_events(c);
}

const CLASS_SERIAL_BUS: u8 = 0x0C;
const SUBCLASS_USB: u8 = 0x03;
const PROGIF_XHCI: u8 = 0x30;

/// Every xHCI controller on the bus not attached yet: brought up and
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
        if ((class >> 8) & 0xFF) as u8 != PROGIF_XHCI {
            continue;
        }
        let Some(index) = CONTROLLERS.iter().position(|c| c.get().is_none()) else {
            break;
        };
        api.pci_enable(bus, slot, func);
        let (mut va, mut size) = (0usize, 0u64);
        if api.pci_bar_map(bus, slot, func, 0, &mut va, &mut size) != 0 || size < 0x1000 {
            continue;
        }
        if CONTROLLERS.iter().filter_map(sync::Once::get).any(|c| c.mmio() == va) {
            continue;
        }
        match hc::init(va, index as u8, api.dma_attach(&myos_abi::DmaDevice::pci(bus, slot, func))) {
            Ok(c) => {
                // In the table before the interrupt is on: the controller
                // raises one as soon as it runs (the ports with devices),
                // and a handler finding no controller would leave IP set,
                // which blocks every later interrupt.
                CONTROLLERS[index].set(c);
                let Some(c) = controller(index as u8) else {
                    continue;
                };
                let mut msix = 0u16;
                let rc = api.pci_irq_enable(bus, slot, func, "xhci", irq_handler, index as *mut c_void, &mut msix);
                let _ = msix == MYOS_IRQ_INTX;
                c.irq_on.store(rc == 0, Ordering::Release);
                // Known to the core only now, with the function set up:
                // the core's thread reads the registers as soon as it
                // knows the controller, and a read while `pci_irq_enable`
                // reconfigures the function sees nothing. A port change
                // the handler saw meanwhile is not lost: the core looks at
                // every port of a new controller.
                let core_index = core().controller_register(&HCD_OPS, index);
                if core_index < 0 {
                    status_fail(api, "xhci: the usb core refused the controller");
                    continue;
                }
                c.core_index.store(core_index as u32, Ordering::Release);
                new += 1;
            }
            Err(why) => {
                let mut buf = [0u8; 64];
                let mut w = Writer { buf: &mut buf, len: 0 };
                w.str("xhci: ");
                w.str(why);
                let n = w.len;
                status_fail(api, core::str::from_utf8(&buf[..n]).unwrap_or("xhci"));
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
        api.write_str("xhci: no usb core\n");
        return -3;
    }
    if unsafe { (*table).version } != USB_CORE_VERSION {
        return -4;
    }
    CORE.store(table.cast_mut(), Ordering::Release);
    if probe(api) == 0 {
        api.write_str("xhci none\n");
        return 0;
    }
    let mut buf = [0u8; 64];
    let mut w = Writer { buf: &mut buf, len: 0 };
    w.str("xhci: ");
    for i in 0..hc::MAX_CTRL as u8 {
        if let Some(c) = controller(i) {
            if i > 0 {
                w.str(", ");
            }
            w.dec(u32::from(c.ports));
            w.str(" ports");
            w.str(if c.irq_on.load(Ordering::Relaxed) { " irq" } else { " polled" });
        }
    }
    let n = w.len;
    status_ok(api, core::str::from_utf8(&buf[..n]).unwrap_or("xhci"));
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_rescan() {
    // A controller that appeared after boot: bring it up and register it;
    // the core scans its ports.
    if !CORE.load(Ordering::Acquire).is_null() {
        probe(api());
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let api = api();
    let mut buf = [0u8; 96];
    let mut w = Writer { buf: &mut buf, len: 0 };
    w.str("xhci: panic");
    if let Some(l) = info.location() {
        // The file's name, not a registry path.
        let file = l.file();
        let tail = file.rsplit('/').next().unwrap_or(file);
        w.str(" at ");
        w.str(tail);
        w.str(":");
        w.dec(l.line());
    }
    let n = w.len;
    status_fail(api, core::str::from_utf8(&buf[..n]).unwrap_or("xhci: panic"));
    loop {
        api.task_yield();
    }
}
