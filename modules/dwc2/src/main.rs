//! DWC2: a USB host controller driver module (`docs/usb.md`,
//! `docs/raspberry-pi.md`) for Synopsys's DesignWare USB 2.0 OTG core, the
//! Raspberry Pi 3's (`brcm,bcm2708-usb` in its device tree, one root port).
//!
//! Brings the controller up (`hc`), powers it through the Pi's firmware
//! when the `rpi_firmware` module is there, and registers it with the USB
//! core (the `usb` module's [`USB_CORE_SERVICE`]), which owns the devices
//! and their enumeration: this module does the controller's part through
//! the [`UsbHcdOps`] it hands the core (`dev`: the device records; the
//! transfers on the host channels). The interrupt handler only records the
//! channels' halts and tells the core what changed.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

mod dev;
mod hc;

use core::ffi::c_void;
use core::sync::atomic::{AtomicPtr, Ordering, compiler_fence};

use myos_abi::{
    ABI_VERSION, ApiCell, DmaDevice, KernelApi, MmioDevice, RPI_FIRMWARE_SERVICE, RPI_FIRMWARE_VERSION,
    RPI_POWER_USB_HCD, RpiFirmwareOps, StrRef, USB_ASYNC_BULK_MAX, USB_CORE_SERVICE, USB_CORE_VERSION, USB_EGONE,
    USB_EIO, USB_EPENDING, USB_HCD_VERSION, USB_SPEED_HIGH, Lock, UsbCoreOps, UsbDevicePath, UsbEndpoint, UsbHcdOps,
    UsbPortStatus, status_fail, status_ok,
};

use hc::{Controller, Halt, PID_DATA0, PID_DATA1, PID_SETUP, Target};

/// A control transfer's data stage at most (the descriptors), in the
/// channel's page after the setup packet.
const CONTROL_MAX: usize = 1024;
const SETUP_BYTES: usize = 8;
const CONTROL_DATA: usize = 64;
/// A queued transfer at most (the channel's page; [`USB_ASYNC_BULK_MAX`]).
const QUEUED_MAX: usize = USB_ASYNC_BULK_MAX;
/// How long a control transfer may take.
const CONTROL_TIMEOUT_MS: u64 = 1000;

static API: ApiCell = ApiCell::new();
/// The core's table, kept by `module_init`.
static CORE: AtomicPtr<UsbCoreOps> = AtomicPtr::new(core::ptr::null_mut());
/// The one controller, set by `module_init` and read by every context after.
static CONTROLLER: AtomicPtr<Controller> = AtomicPtr::new(core::ptr::null_mut());
static CONTROLLER_CELL: Lock<Option<Controller>> = Lock::new(None);

pub fn api() -> &'static KernelApi {
    API.get()
}

fn core() -> &'static UsbCoreOps {
    // SAFETY: the core's service table, which outlives this module (a
    // lookup keeps the core loaded).
    unsafe { CORE.load(Ordering::Acquire).as_ref() }.expect("dwc2: usb core")
}

fn controller() -> Option<&'static Controller> {
    // SAFETY: set once by `module_init`, from a `static` cell, never
    // written again.
    unsafe { CONTROLLER.load(Ordering::Acquire).as_ref() }
}

/// Sleep the calling task `ms` milliseconds.
pub fn sleep_ms(ms: u64) {
    let until = api().monotonic_ns() + ms * 1_000_000;
    api().task_sleep_until(until);
}

/// Writes to memory reach the controller before the channel is enabled.
pub fn dma_wmb() {
    compiler_fence(Ordering::Release);
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("fence rw,rw", options(nostack, preserves_flags));
    }
}

/// A completion the core's thread takes (`hc::process_interrupts`).
pub fn core_wake(c: &Controller) {
    let index = c.core_index.load(Ordering::Acquire);
    if index != u32::MAX {
        core().wake(index);
    }
}

/// The root port changed (`hc::process_interrupts`).
pub fn core_port_change(c: &Controller) {
    let index = c.core_index.load(Ordering::Acquire);
    if index != u32::MAX {
        core().port_change(index, 0);
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
    pub fn hex(&mut self, v: u32) {
        let mut t = [0u8; 10];
        t[0] = b'0';
        t[1] = b'x';
        for i in 0..8 {
            let d = ((v >> (28 - 4 * i)) & 0xF) as u8;
            t[2 + i] = if d < 10 { b'0' + d } else { b'a' + d - 10 };
        }
        self.str(core::str::from_utf8(&t).unwrap_or(""));
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
    name: StrRef { ptr: b"dwc2".as_ptr(), len: 4 },
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
};

unsafe extern "C" fn ports(_ctrl: usize) -> u8 {
    u8::from(controller().is_some())
}

unsafe extern "C" fn port_status(_ctrl: usize, port: u8, out: *mut UsbPortStatus) -> i32 {
    let Some(c) = controller() else {
        return USB_EGONE;
    };
    if out.is_null() || port != 0 {
        return USB_EIO;
    }
    let s = hc::port_status(c);
    hc::port_ack(c);
    unsafe {
        *out = UsbPortStatus { connected: u8::from(s.connected), enabled: u8::from(s.enabled), speed: s.speed };
    }
    0
}

unsafe extern "C" fn port_reset(_ctrl: usize, port: u8) -> i32 {
    let Some(c) = controller() else {
        return USB_EGONE;
    };
    if port != 0 {
        return USB_EIO;
    }
    if hc::port_reset(c) { 0 } else { USB_EIO }
}

/// Every pass of the core's thread: without an interrupt the controller's
/// events are taken here; and a NAKed interrupt endpoint whose time came
/// is asked again. Nonzero while there is something to come back for.
unsafe extern "C" fn poll(_ctrl: usize) -> i32 {
    let Some(c) = controller() else {
        return 0;
    };
    let polled = !c.irq_on.load(Ordering::Relaxed);
    if polled {
        hc::process_interrupts(c);
    }
    let now = api().monotonic_ns();
    let mut waiting = false;
    for ch in 0..c.channels {
        let chan = &c.chans[ch];
        let at = chan.retry_at.load(Ordering::Acquire);
        if at == 0 || !chan.queued.load(Ordering::Acquire) {
            continue;
        }
        if at > now {
            waiting = true;
            continue;
        }
        chan.retry_at.store(0, Ordering::Relaxed);
        if let Some(q) = queued(c, ch) {
            start_queued(c, ch, &q, q.complete);
        }
    }
    i32::from(polled || waiting)
}

unsafe extern "C" fn describe(_ctrl: usize, buf: *mut u8, len: usize) -> usize {
    let Some(c) = controller() else {
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
    w.dec(c.irqs.load(Ordering::Relaxed));
    w.len
}

unsafe extern "C" fn device_new(_ctrl: usize, path: *const UsbDevicePath) -> i32 {
    if controller().is_none() {
        return USB_EGONE;
    }
    if path.is_null() {
        return USB_EIO;
    }
    match dev::new(unsafe { &*path }) {
        Ok(hdev) => hdev as i32,
        Err(e) => e,
    }
}

/// The target of a control transfer to `d`.
fn ep0(d: &dev::Device) -> Target {
    Target { address: d.address, speed: d.speed, endpoint: 0, kind: 0, max_packet: d.ep0_max_packet, tt: d.tt }
}

/// SET_ADDRESS to the device, still at address 0, then its record.
unsafe extern "C" fn device_address(_ctrl: usize, hdev: u32) -> i32 {
    let Some(c) = controller() else {
        return USB_EGONE;
    };
    let d = match dev::get(hdev) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let address = hdev as u8;
    let r = control_transfer(c, hdev, &ep0(&d), 0x00, 5, u16::from(address), 0, core::ptr::null_mut(), 0);
    if r < 0 {
        return r;
    }
    let _ = dev::update(hdev, |d| d.address = address);
    // The device has 2 ms to take its new address.
    sleep_ms(2);
    0
}

unsafe extern "C" fn device_ep0(_ctrl: usize, hdev: u32, max_packet: u16) -> i32 {
    match dev::update(hdev, |d| d.ep0_max_packet = max_packet) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

unsafe extern "C" fn device_configure(_ctrl: usize, hdev: u32, eps: *const UsbEndpoint, n: usize) -> i32 {
    if n != 0 && eps.is_null() {
        return USB_EIO;
    }
    let eps: &[UsbEndpoint] = if n == 0 { &[] } else { unsafe { core::slice::from_raw_parts(eps, n) } };
    match dev::configure(hdev, eps) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// A hub needs nothing of the controller: its children's transfers name
/// it through their translator (`UsbDevicePath::tt_hub`).
unsafe extern "C" fn device_hub(_ctrl: usize, hdev: u32, _ports: u8, _tt_think: u8, _multi_tt: u8) -> i32 {
    match dev::get(hdev) {
        Ok(_) => 0,
        Err(e) => e,
    }
}

/// The device is going: its transfers in flight fail now (the channels
/// disabled, the waiters woken), and no transfer starts on it from here on.
unsafe extern "C" fn device_abort(_ctrl: usize, hdev: u32) {
    let Some(c) = controller() else {
        return;
    };
    dev::abort(hdev);
    for ch in 0..c.channels {
        let chan = &c.chans[ch];
        if !chan.busy.load(Ordering::Acquire) || chan.owner.load(Ordering::Relaxed) != hdev {
            continue;
        }
        chan.aborted.store(true, Ordering::Release);
        chan.retry_at.store(0, Ordering::Relaxed);
        hc::disable(c, ch);
        chan.done.store(true, Ordering::Release);
        if chan.queued.load(Ordering::Acquire) {
            core_wake(c);
        } else {
            api().wake(chan.key());
        }
    }
}

unsafe extern "C" fn device_free(_ctrl: usize, hdev: u32) {
    dev::free(hdev);
}

/// A control transfer: the setup packet from the channel's page, the data
/// stage (in or out, up to [`CONTROL_MAX`] bytes after it), the status
/// stage the other way; the bytes of the data stage.
fn control_transfer(c: &Controller, hdev: u32, t: &Target, request_type: u8, request: u8, value: u16, index: u16, data: *mut u8, len: u16) -> i32 {
    let len = usize::from(len);
    if len > CONTROL_MAX {
        return USB_EIO;
    }
    let to_host = request_type & 0x80 != 0;
    let Some(ch) = hc::take(c, hdev) else {
        return USB_EIO;
    };
    let (bus, page) = c.pages[ch];
    let deadline = api().monotonic_ns() + CONTROL_TIMEOUT_MS * 1_000_000;
    let r = (|| {
        let setup = unsafe { hc::bytes(page, SETUP_BYTES) };
        setup[0] = request_type;
        setup[1] = request;
        setup[2..4].copy_from_slice(&value.to_le_bytes());
        setup[4..6].copy_from_slice(&index.to_le_bytes());
        setup[6..8].copy_from_slice(&(len as u16).to_le_bytes());
        if len != 0 && !to_host {
            unsafe {
                core::ptr::copy_nonoverlapping(data, page.add(CONTROL_DATA), len);
            }
        }
        hc::sync_to(c, page, CONTROL_DATA + len);
        hc::transfer(c, ch, t, false, bus, SETUP_BYTES, PID_SETUP, deadline)?;
        let mut moved = 0;
        if len != 0 {
            let (bytes, _) = hc::transfer(c, ch, t, to_host, bus + CONTROL_DATA as u64, len, PID_DATA1, deadline)?;
            moved = bytes;
            if to_host {
                hc::sync_from(c, unsafe { page.add(CONTROL_DATA) }, len);
                unsafe {
                    core::ptr::copy_nonoverlapping(page.add(CONTROL_DATA), data, moved.min(len));
                }
            }
        }
        // The status stage runs against the data stage's direction (IN
        // when there was no data).
        hc::transfer(c, ch, t, len == 0 || !to_host, bus, 0, PID_DATA1, deadline)?;
        Ok(moved as i32)
    })();
    hc::give_back(c, ch);
    r.unwrap_or_else(|e| e)
}

unsafe extern "C" fn control(_ctrl: usize, hdev: u32, request_type: u8, request: u8, value: u16, index: u16, data: *mut u8, len: u16) -> i32 {
    let Some(c) = controller() else {
        return USB_EGONE;
    };
    if len != 0 && data.is_null() {
        return USB_EIO;
    }
    let d = match dev::get(hdev) {
        Ok(d) => d,
        Err(e) => return e,
    };
    control_transfer(c, hdev, &ep0(&d), request_type, request, value, index, data, len)
}

/// The target of a transfer to endpoint `address` of `d`.
fn endpoint_target(d: &dev::Device, address: u8) -> Option<(Target, dev::Endpoint)> {
    let e = d.endpoint(address)?;
    let t = Target { address: d.address, speed: d.speed, endpoint: address, kind: e.kind, max_packet: e.max_packet, tt: d.tt };
    Some((t, e))
}

/// A bulk transfer, through the controller's bounce buffer in pieces of
/// its size, the endpoint's data toggle carried across.
unsafe extern "C" fn bulk(_ctrl: usize, hdev: u32, endpoint: u8, data: *mut u8, len: usize, timeout_ms: u32) -> i32 {
    let Some(c) = controller() else {
        return USB_EGONE;
    };
    if data.is_null() && len != 0 {
        return USB_EIO;
    }
    let d = match dev::get(hdev) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let Some((t, ep)) = endpoint_target(&d, endpoint) else {
        return USB_EIO;
    };
    let to_host = endpoint & 0x80 != 0;
    let Some(ch) = hc::take(c, hdev) else {
        return USB_EIO;
    };
    let deadline = api().monotonic_ns() + u64::from(timeout_ms.max(1)) * 1_000_000;
    let bounce = c.bounce.lock();
    let mut toggle = ep.toggle;
    let mut done_bytes = 0usize;
    let mut result = 0;
    while done_bytes < len {
        let chunk = (len - done_bytes).min(bounce.len);
        if !to_host {
            unsafe {
                core::ptr::copy_nonoverlapping(data.add(done_bytes), bounce.va, chunk);
            }
        }
        hc::sync_to(c, bounce.va, chunk);
        let pid = if toggle { PID_DATA1 } else { PID_DATA0 };
        match hc::transfer(c, ch, &t, to_host, bounce.bus, chunk, pid, deadline) {
            Ok((moved, packets)) => {
                if packets & 1 != 0 {
                    toggle = !toggle;
                }
                if to_host {
                    hc::sync_from(c, bounce.va, moved);
                    unsafe {
                        core::ptr::copy_nonoverlapping(bounce.va, data.add(done_bytes), moved);
                    }
                }
                done_bytes += moved;
                if moved < chunk {
                    break; // a short packet ends the transfer
                }
            }
            Err(e) => {
                result = e;
                break;
            }
        }
    }
    drop(bounce);
    hc::give_back(c, ch);
    dev::set_toggle(hdev, endpoint, toggle);
    if result < 0 { result } else { done_bytes as i32 }
}

/// What a queued transfer on a channel is (`transfer_start`), for the
/// halts that continue it (`transfer_take`) and the retries `poll` starts.
#[derive(Clone, Copy)]
struct Queued {
    target: Target,
    /// The caller's buffer and its length; what the channel's page has so
    /// far, and the next packet's PID.
    user: *mut u8,
    len: usize,
    moved: usize,
    pid: u32,
    hdev: u32,
    /// How long after a NAK the endpoint is asked again.
    interval_ns: u64,
    /// Behind a transaction translator: the complete half is due.
    complete: bool,
}

// SAFETY: the pointer is the core's buffer, written on its thread only.
unsafe impl Send for Queued {}

/// The queued transfer of each channel (one whose `queued` flag is set).
static QUEUED: [Lock<Option<Queued>>; hc::CHANNELS] = [const { Lock::new(None) }; hc::CHANNELS];

fn queued(c: &Controller, ch: usize) -> Option<Queued> {
    if !c.chans[ch].queued.load(Ordering::Acquire) {
        return None;
    }
    *QUEUED[ch].lock()
}

/// A device's interrupt endpoint is asked again after its interval: frames
/// at low and full speed, 2^(n-1) microframes at high speed.
fn interval_ns(speed: u8, b_interval: u8) -> u64 {
    let ms = if speed == USB_SPEED_HIGH {
        (1u64 << u64::from(b_interval.clamp(1, 16) - 1)).div_ceil(8)
    } else {
        u64::from(b_interval.max(1))
    };
    ms.max(1) * 1_000_000
}

/// The next transaction set of `q`: its bytes and packets (one packet
/// behind a transaction translator, the rest otherwise).
fn next_set(q: &Queued) -> (usize, usize) {
    let mps = usize::from(q.target.max_packet).max(1);
    let rest = q.len - q.moved;
    let len = if q.target.tt.is_some() { rest.min(mps) } else { rest };
    (len, len.div_ceil(mps).max(1))
}

/// Start the next set of `q` on channel `ch`, into the page after what
/// came (`complete_split`: the set's complete half).
fn start_queued(c: &Controller, ch: usize, q: &Queued, complete_split: bool) {
    let (len, packets) = next_set(q);
    hc::start(c, ch, &q.target, true, c.pages[ch].0 + q.moved as u64, len, packets, q.pid, complete_split);
}

/// Queue an IN transfer on an interrupt or bulk endpoint (the core's
/// `transfer_start`; a page at most, an interrupt endpoint's one report):
/// a channel taken for as long as it is queued; its halts wake the core,
/// which asks [`transfer_take`].
unsafe extern "C" fn transfer_start(_ctrl: usize, hdev: u32, endpoint: u8, data: *mut u8, len: usize) -> i32 {
    let Some(c) = controller() else {
        return USB_EGONE;
    };
    if data.is_null() || len == 0 || len > QUEUED_MAX {
        return USB_EIO;
    }
    let d = match dev::get(hdev) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let Some((t, ep)) = endpoint_target(&d, endpoint) else {
        return USB_EIO;
    };
    if !matches!(ep.kind, 2 | 3) || endpoint & 0x80 == 0 {
        return USB_EIO;
    }
    if (0..c.channels).any(|ch| c.chans[ch].owner.load(Ordering::Relaxed) == hdev && queued(c, ch).is_some_and(|q| q.target.endpoint == endpoint)) {
        return USB_EIO;
    }
    let Some(ch) = hc::take(c, hdev) else {
        return USB_EIO;
    };
    let q = Queued {
        target: t,
        user: data,
        len: if ep.kind == 3 { len.min(usize::from(t.max_packet)) } else { len },
        moved: 0,
        pid: if ep.toggle { PID_DATA1 } else { PID_DATA0 },
        hdev,
        interval_ns: if ep.kind == 3 { interval_ns(d.speed, ep.interval) } else { 1_000_000 },
        complete: false,
    };
    *QUEUED[ch].lock() = Some(q);
    c.chans[ch].queued.store(true, Ordering::Release);
    start_queued(c, ch, &q, false);
    0
}

/// The queued transfer on `endpoint` (the core's `transfer_take`):
/// [`USB_EPENDING`] while it runs (a NAK, or a split transaction's next
/// half, has its retry set up here), else its result, the bytes copied
/// to the caller and the channel given back.
unsafe extern "C" fn transfer_take(_ctrl: usize, hdev: u32, endpoint: u8) -> i32 {
    let Some(c) = controller() else {
        return USB_EGONE;
    };
    let Some(ch) = (0..c.channels).find(|&ch| c.chans[ch].owner.load(Ordering::Relaxed) == hdev && queued(c, ch).is_some_and(|q| q.target.endpoint == endpoint)) else {
        return USB_EIO;
    };
    let chan = &c.chans[ch];
    let Some(mut q) = queued(c, ch) else {
        return USB_EIO;
    };
    if !chan.done.load(Ordering::Acquire) {
        return USB_EPENDING;
    }
    let finish = |r: i32| {
        *QUEUED[ch].lock() = None;
        hc::give_back(c, ch);
        r
    };
    let again = |q: Queued, retry_ns: u64| {
        *QUEUED[ch].lock() = Some(q);
        chan.done.store(false, Ordering::Release);
        chan.retry_at.store(api().monotonic_ns() + retry_ns, Ordering::Release);
        USB_EPENDING
    };
    if chan.aborted.load(Ordering::Acquire) {
        return finish(USB_EGONE);
    }
    let (set, packets) = next_set(&q);
    match hc::halt(c, ch, set, packets) {
        Halt::Done { bytes, packets } => {
            if packets & 1 != 0 {
                q.pid = if q.pid == PID_DATA0 { PID_DATA1 } else { PID_DATA0 };
            }
            q.moved += bytes;
            q.complete = false;
            // A short packet, or everything, ends the transfer; behind a
            // translator the next packet is a set of its own.
            if bytes == set && q.moved < q.len {
                *QUEUED[ch].lock() = Some(q);
                start_queued(c, ch, &q, false);
                return USB_EPENDING;
            }
            dev::set_toggle(q.hdev, endpoint, q.pid == PID_DATA1);
            hc::sync_from(c, c.pages[ch].1, q.moved);
            unsafe {
                core::ptr::copy_nonoverlapping(c.pages[ch].1, q.user, q.moved);
            }
            finish(q.moved as i32)
        }
        // Nothing yet: again after the endpoint's interval (`poll`).
        Halt::Nak => again(Queued { complete: false, ..q }, q.interval_ns),
        Halt::Ack if q.target.tt.is_some() => {
            // The start half was taken: the complete half.
            q.complete = true;
            *QUEUED[ch].lock() = Some(q);
            start_queued(c, ch, &q, true);
            USB_EPENDING
        }
        // The translator has no answer yet: the complete half again, next
        // microframe.
        Halt::Nyet => again(Queued { complete: true, ..q }, 125_000),
        Halt::Ack => finish(USB_EIO),
        Halt::Error(e) => finish(e),
    }
}

/// The controller's side of recovering a stalled endpoint: its toggle back
/// to DATA0 (the device's is, after the core's CLEAR_FEATURE).
unsafe extern "C" fn clear_halt(_ctrl: usize, hdev: u32, endpoint: u8) -> i32 {
    match dev::get(hdev) {
        Ok(_) => {
            dev::set_toggle(hdev, endpoint, false);
            0
        }
        Err(e) => e,
    }
}

// ---- Interrupts and probing -------------------------------------------------

unsafe extern "C" fn irq_handler(_ctx: *mut c_void) {
    let Some(c) = controller() else {
        return;
    };
    c.irqs.fetch_add(1, Ordering::Relaxed);
    hc::process_interrupts(c);
}

/// The Pi's firmware powers the controller's domain on, when its module is
/// there (U-Boot left it on otherwise, on the boards this boots).
fn power_on(api: &KernelApi) {
    let table = api.service_lookup(RPI_FIRMWARE_SERVICE) as *const RpiFirmwareOps;
    if table.is_null() {
        return;
    }
    let fw: &'static RpiFirmwareOps = unsafe { &*table };
    if fw.version != RPI_FIRMWARE_VERSION {
        return;
    }
    if !fw.power_on(RPI_POWER_USB_HCD) {
        status_fail(api, "dwc2: the firmware did not power the controller");
    }
}

/// The device tree names the Pi 3's controller: the firmware's name, and
/// upstream Linux's.
const COMPATIBLE: &[&str] = &["brcm,bcm2708-usb", "brcm,bcm2835-usb"];

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
    let mut mmio = MmioDevice::default();
    if !COMPATIBLE.iter().any(|compatible| api.dt_mmio_find(compatible, 0, &mut mmio) == 0) {
        api.write_str("dwc2 none\n");
        return 0;
    }
    let table = api.service_lookup(USB_CORE_SERVICE) as *const UsbCoreOps;
    if table.is_null() {
        api.write_str("dwc2: no usb core\n");
        return -3;
    }
    if unsafe { (*table).version } != USB_CORE_VERSION {
        return -4;
    }
    CORE.store(table.cast_mut(), Ordering::Release);
    power_on(api);
    let c = match hc::init(mmio.base, api.dma_attach(&DmaDevice::mmio(mmio.phys))) {
        Ok(c) => c,
        Err(why) => {
            let mut buf = [0u8; 64];
            let mut w = Writer { buf: &mut buf, len: 0 };
            w.str("dwc2: ");
            w.str(why);
            let n = w.len;
            status_fail(api, core::str::from_utf8(&buf[..n]).unwrap_or("dwc2"));
            return 0;
        }
    };
    {
        let mut cell = CONTROLLER_CELL.lock();
        *cell = Some(c);
        let p: *mut Controller = cell.as_mut().unwrap();
        CONTROLLER.store(p, Ordering::Release);
    }
    let Some(c) = controller() else {
        return -5;
    };
    let irq_on = mmio.irq != 0 && api.irq_enable(mmio.irq, "dwc2", irq_handler, core::ptr::null_mut()) == 0;
    c.irq_on.store(irq_on, Ordering::Release);
    // Known to the core only now, with the interrupt set up: a port change
    // the handler saw meanwhile is not lost, the core looks at every port
    // of a new controller.
    let core_index = core().controller_register(&HCD_OPS, 0);
    if core_index < 0 {
        status_fail(api, "dwc2: the usb core refused the controller");
        return -6;
    }
    c.core_index.store(core_index as u32, Ordering::Release);
    let mut buf = [0u8; 64];
    let mut w = Writer { buf: &mut buf, len: 0 };
    w.str("dwc2: ");
    w.dec(c.channels as u32);
    w.str(" channels");
    w.str(if irq_on { " irq" } else { " polled" });
    let n = w.len;
    status_ok(api, core::str::from_utf8(&buf[..n]).unwrap_or("dwc2"));
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

// So `cargo build --bin dwc2` links. The kernel never jumps here.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let api = api();
    let mut buf = [0u8; 96];
    let mut w = Writer { buf: &mut buf, len: 0 };
    w.str("dwc2: panic");
    if let Some(l) = info.location() {
        let file = l.file();
        let tail = file.rsplit('/').next().unwrap_or(file);
        w.str(" at ");
        w.str(tail);
        w.str(":");
        w.dec(l.line());
    }
    let n = w.len;
    status_fail(api, core::str::from_utf8(&buf[..n]).unwrap_or("dwc2: panic"));
    loop {
        api.task_yield();
    }
}
