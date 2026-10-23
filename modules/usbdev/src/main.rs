//! `/dev/usb`: USB devices for drivers that run as programs.
//!
//! The module is the class driver of last resort: registered after the
//! kernel's own (`usb_hub`, `usb_storage`, `usb_hid`), the host offers it
//! every interface they left, and it takes them all. A directory per
//! device with such an interface (`NNN`, its `/proc/usb` id in hex) has:
//!
//! - `info`: what the device is, `id VVVV:PPPP`, `class CC:SS:PP`,
//!   `speed low|full|high|super`, then each interface taken
//!   (`ifN CC:SS:PP`) and its endpoints (`epXX bulk|interrupt in|out
//!   MAXPACKET`, the address in hex), and `held` while a program drives it.
//! - `ctl`: held by one program at a time (the driver; root only), from
//!   its open on. Reads as `info` does; writes `clear XX` (recover the
//!   stalled endpoint `XX`). Closing it stops the device's IN endpoints
//!   being read (a transfer already queued completes into nothing).
//! - `ep0`: control transfers. A write is one: its 8-byte setup packet
//!   (`bmRequestType`, `bRequest`, `wValue`, `wIndex`, `wLength`, little
//!   endian), then an OUT transfer's data stage. An IN transfer's data
//!   stage is the next read (whatever of it came; an empty read once
//!   taken). A failed transfer fails the write.
//! - `epXX`, each bulk or interrupt endpoint: an OUT one's write is one
//!   transfer (1 s at most); an IN one keeps a transfer queued
//!   (`UsbHostOps::transfer_start`, from its file's first open on) and a read
//!   returns one completed transfer whole (a buffer of
//!   `USB_ASYNC_BULK_MAX` bytes takes any; a smaller one gets its start),
//!   waits for one, and `poll` says one is there. Up to [`QUEUE`]
//!   transfers wait to be read; later ones are dropped.
//!
//! The files besides `info` and `ctl` are only there while `ctl` is held.
//! A device pulled out fails every file's I/O; its directory goes once
//! `ctl` is let go.
//!
//! Unlike a PCI function's, a USB device's transfers go through the
//! controller's own buffers: it reaches no memory it was not given, so the
//! module is loaded at boot.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

use myos_abi::{
    status_fail, status_ok, ApiCell, KernelApi, Lock, ModuleVfsOps, StrRef, UsbDeviceInfo, UsbDriverOps, UsbHostOps,
    UsbInterfaceInfo, VfsStatInfo, ABI_VERSION, MYOS_POLLIN, MYOS_READ_WAIT, USB_ASYNC_BULK_MAX, USB_EGONE,
    USB_HOST_VERSION, USB_SERVICE, USB_SPEED_FULL, USB_SPEED_HIGH, USB_SPEED_LOW,
};

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
/// Devices listed.
const MAX_DEV: usize = 8;
/// Interfaces and endpoints a device's entry keeps.
const MAX_INTF: usize = 4;
const MAX_EP: usize = 16;
/// Completed IN transfers an endpoint keeps for its reader.
const QUEUE: usize = 4;
/// An OUT transfer's time.
const OUT_TIMEOUT_MS: u32 = 1000;
/// A control transfer's data stage at most (the host's limit).
const CONTROL_MAX: usize = 1024;

static API: ApiCell = ApiCell::new();
static HOST: AtomicPtr<UsbHostOps> = AtomicPtr::new(core::ptr::null_mut());

fn api() -> &'static KernelApi {
    API.get()
}

fn host() -> &'static UsbHostOps {
    // SAFETY: the host's service table, which outlives this module.
    unsafe { HOST.load(Ordering::Acquire).as_ref() }.expect("usbdev: host")
}

#[derive(Clone, Copy)]
struct Ep {
    address: u8,
    /// Transfer type: 2 bulk, 3 interrupt.
    kind: u8,
    max_packet: u16,
    /// An IN endpoint's: the buffer its queued transfer lands in, the
    /// transfers completed and not read yet ({buffer, length} each, the
    /// oldest at `head`), whether one is queued.
    landing: *mut u8,
    slots: [*mut u8; QUEUE],
    lens: [usize; QUEUE],
    head: usize,
    count: usize,
    queued: bool,
}

#[derive(Clone, Copy)]
struct Dev {
    info: UsbDeviceInfo,
    intfs: [Option<UsbInterfaceInfo>; MAX_INTF],
    eps: [Option<Ep>; MAX_EP],
    /// `ctl` is open.
    held: bool,
    /// Pulled out: every transfer fails.
    gone: bool,
    /// An IN control transfer's data stage, for the next read of `ep0`.
    control: [u8; CONTROL_MAX],
    control_len: usize,
}

// SAFETY: the pointers are this module's own buffers (`KernelApi::alloc`),
// reached under `DEVS`'s lock (and by the host, for a queued transfer).
unsafe impl Send for Dev {}

static DEVS: Lock<[Option<Dev>; MAX_DEV]> = Lock::new([None; MAX_DEV]);
/// Per device entry: completed transfers, all endpoints ('poll`'s change
/// count).
static CHANGES: [AtomicU64; MAX_DEV] = [const { AtomicU64::new(0) }; MAX_DEV];

static DRIVER: UsbDriverOps = UsbDriverOps { name: StrRef { ptr: b"usbdev".as_ptr(), len: 6 }, probe, disconnect };

/// How long an IN endpoint's transfer is (an interrupt endpoint's packet,
/// a bulk one's page).
fn in_len(e: &Ep) -> usize {
    if e.kind == 3 { usize::from(e.max_packet).min(64) } else { USB_ASYNC_BULK_MAX }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum File {
    Info,
    Ctl,
    Ep0,
    Ep(usize),
}

enum Node {
    Root,
    Dir(usize),
    File(usize, File),
}

fn hex_digit(v: u8) -> u8 {
    if v < 10 { b'0' + v } else { b'a' + v - 10 }
}

/// `NNN`, the device's id.
fn dev_name(d: &Dev) -> [u8; 3] {
    let id = d.info.id;
    [hex_digit((id >> 8) as u8 & 15), hex_digit((id >> 4) as u8 & 15), hex_digit(id as u8 & 15)]
}

fn ep_name(address: u8) -> [u8; 4] {
    [b'e', b'p', hex_digit(address >> 4), hex_digit(address & 15)]
}

fn parse_file(s: &[u8], d: &Dev) -> Option<File> {
    let file = match s {
        b"info" => File::Info,
        b"ctl" => File::Ctl,
        b"ep0" => File::Ep0,
        _ => File::Ep(d.eps.iter().position(|e| e.is_some_and(|e| ep_name(e.address)[..] == *s))?),
    };
    (matches!(file, File::Info | File::Ctl) || d.held).then_some(file)
}

fn node(devs: &[Option<Dev>; MAX_DEV], path: *const u8, len: usize) -> Option<Node> {
    let path = if path.is_null() || len == 0 { &[][..] } else { unsafe { core::slice::from_raw_parts(path, len) } };
    let path = path.strip_suffix(b"/").unwrap_or(path);
    if path.is_empty() || path == b"." {
        return Some(Node::Root);
    }
    let (dir, file) = match path.iter().position(|&c| c == b'/') {
        Some(i) => (&path[..i], Some(&path[i + 1..])),
        None => (path, None),
    };
    let i = devs.iter().position(|d| d.as_ref().is_some_and(|d| dev_name(d) == dir))?;
    let d = devs[i].as_ref()?;
    match file {
        None => Some(Node::Dir(i)),
        Some(f) => Some(Node::File(i, parse_file(f, d)?)),
    }
}

/// A text buffer for `info` and the listings.
struct Text {
    buf: [u8; 1024],
    len: usize,
}

impl Text {
    fn new() -> Self {
        Self { buf: [0; 1024], len: 0 }
    }

    fn push(&mut self, s: &[u8]) {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s[..n]);
        self.len += n;
    }

    fn dec(&mut self, mut v: u64) {
        let mut d = [0u8; 20];
        let mut i = d.len();
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.push(&d[i..]);
    }

    fn hex(&mut self, v: u64, digits: usize) {
        for k in (0..digits).rev() {
            self.push(&[hex_digit((v >> (4 * k)) as u8 & 15)]);
        }
    }

    fn triple(&mut self, a: u8, b: u8, c: u8) {
        self.hex(u64::from(a), 2);
        self.push(b":");
        self.hex(u64::from(b), 2);
        self.push(b":");
        self.hex(u64::from(c), 2);
    }

    fn bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

fn info_text(d: &Dev) -> Text {
    let mut t = Text::new();
    let i = &d.info;
    t.push(b"id ");
    t.hex(u64::from(i.vendor), 4);
    t.push(b":");
    t.hex(u64::from(i.product), 4);
    t.push(b"\nclass ");
    t.triple(i.class, i.subclass, i.protocol);
    t.push(b"\nspeed ");
    t.push(match i.speed {
        USB_SPEED_LOW => b"low",
        USB_SPEED_FULL => b"full",
        USB_SPEED_HIGH => b"high",
        _ => b"super",
    });
    t.push(b"\n");
    for f in d.intfs.iter().flatten() {
        t.push(b"if");
        t.dec(u64::from(f.number));
        t.push(b" ");
        t.triple(f.class, f.subclass, f.protocol);
        t.push(b"\n");
    }
    for e in d.eps.iter().flatten() {
        t.push(&ep_name(e.address));
        t.push(if e.kind == 2 { b" bulk" } else { b" interrupt" });
        t.push(if e.address & 0x80 != 0 { b" in " } else { b" out " });
        t.dec(u64::from(e.max_packet));
        t.push(b"\n");
    }
    if d.gone {
        t.push(b"gone\n");
    } else if d.held {
        t.push(b"held\n");
    }
    t
}

/// The device entry of host device `dev`.
fn index_of(devs: &[Option<Dev>; MAX_DEV], dev: u32) -> Option<usize> {
    devs.iter().position(|d| d.as_ref().is_some_and(|d| d.info.id == dev && !d.gone))
}

/// An interface no kernel driver took: this module's, its bulk and
/// interrupt endpoints with it (an IN one's buffers allocated now).
unsafe extern "C" fn probe(dev: *const UsbDeviceInfo, intf: *const UsbInterfaceInfo) -> i32 {
    let (dev, intf) = unsafe { (&*dev, &*intf) };
    let mut devs = DEVS.lock();
    let i = match index_of(&devs, dev.id) {
        Some(i) => i,
        None => {
            let Some(i) = devs.iter().position(Option::is_none) else {
                return -1;
            };
            devs[i] = Some(Dev {
                info: *dev,
                intfs: [None; MAX_INTF],
                eps: [None; MAX_EP],
                held: false,
                gone: false,
                control: [0; CONTROL_MAX],
                control_len: 0,
            });
            i
        }
    };
    let d = devs[i].as_mut().unwrap();
    let Some(f) = d.intfs.iter_mut().find(|f| f.is_none()) else {
        return -1;
    };
    *f = Some(*intf);
    for e in &intf.endpoints[..usize::from(intf.n_endpoints)] {
        let kind = e.attributes & 3;
        if !matches!(kind, 2 | 3) {
            continue;
        }
        let Some(slot) = d.eps.iter_mut().find(|s| s.is_none()) else {
            break;
        };
        let mut ep = Ep {
            address: e.address,
            kind,
            max_packet: e.max_packet,
            landing: core::ptr::null_mut(),
            slots: [core::ptr::null_mut(); QUEUE],
            lens: [0; QUEUE],
            head: 0,
            count: 0,
            queued: false,
        };
        if e.address & 0x80 != 0 {
            let n = in_len(&ep);
            ep.landing = api().alloc(n, 8);
            for s in &mut ep.slots {
                *s = api().alloc(n, 8);
            }
            if ep.landing.is_null() || ep.slots.iter().any(|s| s.is_null()) {
                continue;
            }
        }
        *slot = Some(ep);
    }
    0
}

/// The device is gone: its transfers fail, its readers wake; its entry
/// goes now, or when its holder lets go.
unsafe extern "C" fn disconnect(dev: u32, _intf: u8) {
    let mut devs = DEVS.lock();
    let Some(i) = index_of(&devs, dev) else {
        return;
    };
    let d = devs[i].as_mut().unwrap();
    d.gone = true;
    if !d.held {
        forget(&mut devs[i]);
    }
    CHANGES[i].fetch_add(1, Ordering::Relaxed);
    api().wake_any();
}

/// An entry's buffers back, unless a transfer may still land in one (a
/// gone device's queued transfer completes with `USB_EGONE`, before
/// `disconnect`: none is left then).
fn forget(slot: &mut Option<Dev>) {
    if let Some(d) = slot.take() {
        for e in d.eps.iter().flatten() {
            if e.address & 0x80 == 0 || (e.queued && !d.gone) {
                continue;
            }
            let n = in_len(e);
            unsafe { (api().dealloc)(e.landing, n, 8) };
            for &s in &e.slots {
                unsafe { (api().dealloc)(s, n, 8) };
            }
        }
    }
}

/// The context of a queued transfer: the device entry and the endpoint.
fn ctx(i: usize, k: usize) -> *mut c_void {
    (i * MAX_EP + k) as *mut c_void
}

/// A queued IN transfer completed: kept for the reader, the next queued
/// while the device is held and there is room.
unsafe extern "C" fn in_done(c: *mut c_void, status: i32) {
    let (i, k) = (c as usize / MAX_EP, c as usize % MAX_EP);
    let again = {
        let mut devs = DEVS.lock();
        let Some(d) = devs.get_mut(i).and_then(Option::as_mut) else {
            return;
        };
        let (held, gone) = (d.held, d.gone || status == USB_EGONE);
        let Some(e) = d.eps[k].as_mut() else {
            return;
        };
        e.queued = false;
        if status > 0 && held && e.count < QUEUE {
            let at = (e.head + e.count) % QUEUE;
            let n = (status as usize).min(in_len(e));
            // SAFETY: both are this endpoint's buffers of in_len bytes.
            unsafe { core::ptr::copy_nonoverlapping(e.landing, e.slots[at], n) };
            e.lens[at] = n;
            e.count += 1;
        }
        CHANGES[i].fetch_add(1, Ordering::Relaxed);
        let again = held && !gone && status >= 0 && e.count < QUEUE;
        if again {
            e.queued = true;
        }
        again.then(|| (d.info.id, e.address, e.landing, in_len(e)))
    };
    if let Some((dev, address, landing, n)) = again {
        queue(i, k, dev, address, landing, n);
    }
    api().wake_any();
}

/// Queue the endpoint's transfer (`queued` already set, the lock let go:
/// the host takes the device's own).
fn queue(i: usize, k: usize, dev: u32, address: u8, landing: *mut u8, n: usize) {
    // SAFETY: `landing` is the endpoint's buffer of `n` bytes, which stays
    // while a transfer is queued (`forget`).
    let r = unsafe { (host().transfer_start)(dev, address, landing, n, in_done, ctx(i, k)) };
    if r != 0 {
        let mut devs = DEVS.lock();
        if let Some(e) = devs[i].as_mut().and_then(|d| d.eps[k].as_mut()) {
            e.queued = false;
        }
    }
}

unsafe extern "C" fn usb_lookup(_: *const u8, _: usize, _: *mut *const u8, _: *mut usize) -> i32 {
    -1
}

unsafe extern "C" fn usb_stat(path: *const u8, len: usize, out: *mut VfsStatInfo) -> i32 {
    let devs = DEVS.lock();
    let Some(n) = node(&devs, path, len) else {
        return -1;
    };
    let (mode, size, ino) = match n {
        Node::Root => (S_IFDIR | 0o755, 0, 1),
        Node::Dir(i) => (S_IFDIR | 0o755, 0, 64 * (i as u64 + 1)),
        Node::File(i, f) => {
            let d = devs[i].as_ref().unwrap();
            let (k, mode, size) = match f {
                File::Info => (0, 0o444, info_text(d).len as u64),
                File::Ctl => (1, 0o600, info_text(d).len as u64),
                File::Ep0 => (2, 0o600, 0),
                File::Ep(n) => (3 + n as u64, 0o600, 0),
            };
            (S_IFREG | mode, size, 64 * (i as u64 + 1) + k)
        }
    };
    let dir = matches!(n, Node::Root | Node::Dir(_));
    unsafe { *out = VfsStatInfo { mode, size, ino: ino as u32, nlink: if dir { 2 } else { 1 }, mtime: 0, atime: 0 } };
    0
}

unsafe extern "C" fn usb_listdir(path: *const u8, len: usize, buf: *mut u8, cap: usize, out_len: *mut usize) -> i32 {
    let devs = DEVS.lock();
    let mut t = Text::new();
    match node(&devs, path, len) {
        Some(Node::Root) => {
            for d in devs.iter().flatten() {
                t.push(&dev_name(d));
                t.push(b"\n");
            }
        }
        Some(Node::Dir(i)) => {
            let d = devs[i].as_ref().unwrap();
            t.push(b"info\nctl\n");
            if d.held {
                t.push(b"ep0\n");
                for e in d.eps.iter().flatten() {
                    t.push(&ep_name(e.address));
                    t.push(b"\n");
                }
            }
        }
        _ => return -1,
    }
    if t.len > cap {
        return -1;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(t.buf.as_ptr(), buf, t.len);
        *out_len = t.len;
    }
    0
}

/// `ctl`: one holder, root, a device still there.
/// An IN endpoint's file: its transfers start coming.
unsafe extern "C" fn usb_open(path: *const u8, len: usize, _out: *mut u8, _out_cap: usize) -> i32 {
    let mut devs = DEVS.lock();
    match node(&devs, path, len) {
        Some(Node::File(i, File::Ctl)) => {
            let d = devs[i].as_mut().unwrap();
            if d.held || d.gone || api().current_uid() != 0 {
                return -1;
            }
            d.held = true;
            d.control_len = 0;
        }
        Some(Node::File(i, File::Ep(k))) => {
            if let Some((dev, address, landing, n)) = start(&mut devs, i, k) {
                drop(devs);
                queue(i, k, dev, address, landing, n);
            }
        }
        _ => {}
    }
    0
}

/// IN endpoint `k` of entry `i` keeps a transfer queued from now on: what
/// `queue` needs when none is yet.
fn start(devs: &mut [Option<Dev>; MAX_DEV], i: usize, k: usize) -> Option<(u32, u8, *mut u8, usize)> {
    let d = devs[i].as_mut()?;
    let (dev, ok) = (d.info.id, d.held && !d.gone);
    let e = d.eps[k].as_mut().filter(|e| e.address & 0x80 != 0 && ok && !e.queued)?;
    e.queued = true;
    Some((dev, e.address, e.landing, in_len(e)))
}

/// The holder let go: what came and was not read is dropped (a queued
/// transfer's lands in nothing); a gone device's entry goes.
unsafe extern "C" fn usb_release(path: *const u8, len: usize) -> i32 {
    let mut devs = DEVS.lock();
    if let Some(Node::File(i, File::Ctl)) = node(&devs, path, len) {
        let d = devs[i].as_mut().unwrap();
        d.held = false;
        for e in d.eps.iter_mut().flatten() {
            e.count = 0;
        }
        if d.gone {
            forget(&mut devs[i]);
        }
    }
    0
}

unsafe extern "C" fn usb_read(path: *const u8, len: usize, pos: usize, buf: *mut u8, cap: usize) -> i32 {
    let mut devs = DEVS.lock();
    let Some(Node::File(i, f)) = node(&devs, path, len) else {
        return -1;
    };
    let d = devs[i].as_mut().unwrap();
    let out = |src: &[u8], pos: usize| {
        let n = cap.min(src.len().saturating_sub(pos));
        if n != 0 {
            unsafe { core::ptr::copy_nonoverlapping(src[pos..].as_ptr(), buf, n) };
        }
        n as i32
    };
    match f {
        File::Info | File::Ctl => out(info_text(d).bytes(), pos),
        File::Ep0 => {
            let n = out(&d.control[..d.control_len], 0);
            d.control_len = 0;
            n
        }
        File::Ep(k) => {
            let gone = d.gone;
            let Some(e) = d.eps[k].as_mut() else {
                return -1;
            };
            if e.address & 0x80 == 0 {
                return -1;
            }
            if e.count > 0 {
                let at = e.head;
                // SAFETY: a slot of in_len bytes, `lens[at]` of them filled.
                let data = unsafe { core::slice::from_raw_parts(e.slots[at], e.lens[at]) };
                let n = out(data, 0);
                e.head = (at + 1) % QUEUE;
                e.count -= 1;
                return n;
            }
            if gone {
                return -1;
            }
            if let Some((dev, address, landing, n)) = start(&mut devs, i, k) {
                drop(devs);
                queue(i, k, dev, address, landing, n);
            }
            MYOS_READ_WAIT
        }
    }
}

unsafe extern "C" fn usb_write(path: *const u8, len: usize, _pos: usize, buf: *const u8, n: usize) -> i32 {
    let devs = DEVS.lock();
    let Some(Node::File(i, f)) = node(&devs, path, len) else {
        return -1;
    };
    let d = devs[i].as_ref().unwrap();
    let (dev, gone) = (d.info.id, d.gone);
    let data = unsafe { core::slice::from_raw_parts(buf, n) };
    match f {
        File::Ctl => {
            let address = data.trim_ascii().strip_prefix(b"clear ").and_then(|h| {
                (h.len() == 2).then(|| h.iter().try_fold(0u8, |v, &c| Some(v << 4 | (c as char).to_digit(16)? as u8)))?
            });
            let Some(address) = address.filter(|a| d.eps.iter().flatten().any(|e| e.address == *a)) else {
                return -1;
            };
            drop(devs);
            if gone || host().clear_halt(dev, address) != 0 { -1 } else { n as i32 }
        }
        File::Ep0 => {
            if gone || n < 8 {
                return -1;
            }
            drop(devs);
            let (rt, req) = (data[0], data[1]);
            let value = u16::from_le_bytes([data[2], data[3]]);
            let index = u16::from_le_bytes([data[4], data[5]]);
            let length = usize::from(u16::from_le_bytes([data[6], data[7]]));
            let mut stage = [0u8; CONTROL_MAX];
            let len = if rt & 0x80 != 0 { length } else { n - 8 };
            if len > CONTROL_MAX || (rt & 0x80 == 0 && len != length) {
                return -1;
            }
            if rt & 0x80 == 0 {
                stage[..len].copy_from_slice(&data[8..]);
            }
            let r = host().control(dev, rt, req, value, index, &mut stage[..len]);
            if r < 0 {
                return -1;
            }
            if rt & 0x80 != 0 {
                let mut devs = DEVS.lock();
                if let Some(d) = devs[i].as_mut() {
                    let got = (r as usize).min(len);
                    d.control[..got].copy_from_slice(&stage[..got]);
                    d.control_len = got;
                }
            }
            n as i32
        }
        File::Ep(k) => {
            let Some(e) = d.eps[k] else {
                return -1;
            };
            if e.address & 0x80 != 0 || e.kind != 2 || gone {
                return -1;
            }
            drop(devs);
            let r = host().bulk_out(dev, e.address, data, OUT_TIMEOUT_MS);
            if r < 0 { -1 } else { r }
        }
        File::Info => -1,
    }
}

/// A shell's `>` truncates first: nothing to do but let it.
unsafe extern "C" fn usb_truncate(path: *const u8, len: usize) -> i32 {
    match node(&DEVS.lock(), path, len) {
        Some(Node::File(_, File::Ctl | File::Ep0 | File::Ep(_))) => 0,
        _ => -1,
    }
}

unsafe extern "C" fn usb_poll(path: *const u8, len: usize, changes: *mut u64) -> u32 {
    let devs = DEVS.lock();
    let Some(Node::File(i, File::Ep(k))) = node(&devs, path, len) else {
        return 0;
    };
    if !changes.is_null() {
        unsafe { *changes = CHANGES[i].load(Ordering::Relaxed) };
    }
    let d = devs[i].as_ref().unwrap();
    // A gone device's reader wakes to its error.
    if d.gone || d.eps[k].is_some_and(|e| e.count > 0) { MYOS_POLLIN } else { 0 }
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api_ptr: *const KernelApi) -> i32 {
    let Some(api) = (unsafe { api_ptr.as_ref() }) else {
        return -1;
    };
    if api.abi_version != ABI_VERSION {
        return -2;
    }
    unsafe { API.set(api_ptr) };
    let table = api.service_lookup(USB_SERVICE) as *const UsbHostOps;
    if table.is_null() {
        api.write_str("usbdev: no usb host\n");
        return -3;
    }
    if unsafe { (*table).version } != USB_HOST_VERSION {
        return -4;
    }
    HOST.store(table.cast_mut(), Ordering::Release);
    // Built here, not in a static: aarch64 ET_EXEC modules do not relocate
    // fn pointers in .rodata (see netfs).
    let ops = ModuleVfsOps {
        lookup: usb_lookup,
        stat: usb_stat,
        listdir: usb_listdir,
        register: None,
        read: Some(usb_read),
        write: Some(usb_write),
        create: None,
        truncate: Some(usb_truncate),
        mkdir: None,
        rmdir: None,
        unlink: None,
        rename: None,
        symlink: None,
        readlink: None,
        release: Some(usb_release),
        mmap: None,
        poll: Some(usb_poll),
        open: Some(usb_open),
        set_times: None,
        unmount: None,
        unlink_keep: None,
        read_ino: None,
        write_ino: None,
        stat_ino: None,
        forget_ino: None,
        set_size: None,
        set_size_ino: None,
        file_id: None,
        set_times_ino: None,
        statfs: None,
    };
    if api.vfs_mount("usb", "dev/usb", &ops) != 0 {
        status_fail(api, "usbdev: /dev/usb");
        return -5;
    }
    if host().driver_register(&DRIVER) != 0 {
        return -6;
    }
    status_ok(api, "usbdev");
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

// So `cargo build --bin usbdev` links. The kernel never jumps here.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
