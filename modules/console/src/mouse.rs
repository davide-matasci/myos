//! `/dev/console/mouse`: the pointer, for programs that draw on the screen
//! themselves (a graphical session on `/dev/fb`). Pointing device drivers in
//! other modules (`usb_hid`) feed it through the [`POINTER_SERVICE`] table,
//! this module's own (the PS/2 mouse, virtio-input's mice and tablets)
//! through the same table here; every device lands in the same file.
//!
//! One event per line: `m <dx> <dy>` relative motion (right and down
//! positive), `a <x> <y>` an absolute position (both axes 0 to
//! [`POINTER_ABS_MAX`], a tablet), `w <dz>` the wheel (positive: up), and
//! `d <code>` / `u <code>` a button pressed / released (Linux `BTN_*`:
//! left 272, right 273, middle 274). Motion queued and not read yet is
//! merged: two `m` lines in a row become one, as do two `a` or two `w`.
//!
//! Opening the file takes the pointer; a second open fails. Nothing is
//! queued while nobody holds it.

use core::sync::atomic::{AtomicBool, Ordering};

use myos_abi::{
    Lock, MYOS_POLL_RECHECK, MYOS_POLLIN, MYOS_READ_WAIT, ModuleVfsOps, POINTER_ABS_MAX, POINTER_SERVICE, POINTER_VERSION, PointerOps,
    VfsStatInfo,
};

use crate::{api, keyboard};

const S_IFREG: u32 = 0o100000;

#[derive(Clone, Copy)]
enum Event {
    Motion(i32, i32),
    Absolute(i32, i32),
    Wheel(i32),
    Button(u16, bool),
}

/// Events not read yet; new ones are dropped while it is full.
struct Queue {
    ev: [Event; 128],
    head: usize,
    len: usize,
    /// How many times the queue changed: the change count `poll` reports.
    changes: u64,
}

static QUEUE: Lock<Queue> = Lock::new(Queue { ev: [Event::Wheel(0); 128], head: 0, len: 0, changes: 0 });
/// Someone holds the file.
static HELD: AtomicBool = AtomicBool::new(false);

/// Queue `e`, merged into the last event when both are motion of one kind,
/// and wake the readers.
fn push(e: Event) {
    if !HELD.load(Ordering::SeqCst) {
        return;
    }
    {
        let mut q = QUEUE.lock();
        let n = q.ev.len();
        let last = (q.head + q.len + n - 1) % n;
        let merged = match (q.len > 0).then(|| q.ev[last]) {
            Some(Event::Motion(x, y)) => match e {
                Event::Motion(dx, dy) => Some(Event::Motion(x.saturating_add(dx), y.saturating_add(dy))),
                _ => None,
            },
            Some(Event::Absolute(..)) => matches!(e, Event::Absolute(..)).then_some(e),
            Some(Event::Wheel(z)) => match e {
                Event::Wheel(dz) => Some(Event::Wheel(z.saturating_add(dz))),
                _ => None,
            },
            _ => None,
        };
        if let Some(m) = merged {
            q.ev[last] = m;
        } else if q.len < n {
            let i = (q.head + q.len) % n;
            q.ev[i] = e;
            q.len += 1;
        } else {
            return;
        }
        q.changes = q.changes.wrapping_add(1);
    }
    api().wake_any();
}

unsafe extern "C" fn motion(dx: i32, dy: i32) {
    if dx != 0 || dy != 0 {
        push(Event::Motion(dx, dy));
    }
}

unsafe extern "C" fn absolute(x: i32, y: i32) {
    push(Event::Absolute(x.clamp(0, POINTER_ABS_MAX), y.clamp(0, POINTER_ABS_MAX)));
}

unsafe extern "C" fn wheel(dz: i32) {
    if dz != 0 {
        push(Event::Wheel(dz));
    }
}

unsafe extern "C" fn button(code: u16, pressed: bool) {
    push(Event::Button(code, pressed));
}

static OPS: PointerOps = PointerOps { version: POINTER_VERSION, motion, absolute, wheel, button };

/// The pointer's table, for this module's own mice (PS/2, virtio-input).
pub(crate) fn pointer() -> &'static PointerOps {
    &OPS
}

/// `v` in decimal at `out[n..]`; the new length.
fn put_int(out: &mut [u8], mut n: usize, v: i32) -> usize {
    if v < 0 {
        out[n] = b'-';
        n += 1;
    }
    let mut digits = [0u8; 10];
    let mut i = digits.len();
    let mut v = v.unsigned_abs();
    loop {
        i -= 1;
        digits[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    out[n..n + digits.len() - i].copy_from_slice(&digits[i..]);
    n + digits.len() - i
}

/// The event's line.
fn format(e: Event, out: &mut [u8; 32]) -> usize {
    let (tag, a, b) = match e {
        Event::Motion(x, y) => (b'm', x, Some(y)),
        Event::Absolute(x, y) => (b'a', x, Some(y)),
        Event::Wheel(z) => (b'w', z, None),
        Event::Button(code, pressed) => (if pressed { b'd' } else { b'u' }, i32::from(code), None),
    };
    out[0] = tag;
    out[1] = b' ';
    let mut n = put_int(out, 2, a);
    if let Some(b) = b {
        out[n] = b' ';
        n = put_int(out, n + 1, b);
    }
    out[n] = b'\n';
    n + 1
}

fn root(path: *const u8, len: usize) -> bool {
    path.is_null() || len == 0 || unsafe { core::slice::from_raw_parts(path, len) } == b"."
}

unsafe extern "C" fn mouse_lookup(_: *const u8, _: usize, _: *mut *const u8, _: *mut usize) -> i32 {
    -1
}

unsafe extern "C" fn mouse_stat(path: *const u8, len: usize, out: *mut VfsStatInfo) -> i32 {
    if !root(path, len) {
        return -1;
    }
    unsafe {
        *out = VfsStatInfo { mode: S_IFREG | 0o444, size: 0, ino: 1, nlink: 1, mtime: 0, atime: 0 };
    }
    0
}

unsafe extern "C" fn mouse_listdir(_: *const u8, _: usize, _: *mut u8, _: usize, _: *mut usize) -> i32 {
    -1
}

/// As many whole event lines as fit, or [`MYOS_READ_WAIT`] while none is
/// queued.
unsafe extern "C" fn mouse_read(path: *const u8, len: usize, _pos: usize, buf: *mut u8, cap: usize) -> i32 {
    if !root(path, len) || buf.is_null() {
        return -1;
    }
    let out = unsafe { core::slice::from_raw_parts_mut(buf, cap) };
    if keyboard::pointer_polled() {
        keyboard::pump();
    }
    let mut q = QUEUE.lock();
    if q.len == 0 {
        return MYOS_READ_WAIT;
    }
    let mut n = 0;
    while q.len > 0 {
        let mut line = [0u8; 32];
        let len = format(q.ev[q.head], &mut line);
        if n + len > out.len() {
            break;
        }
        out[n..n + len].copy_from_slice(&line[..len]);
        n += len;
        q.head = (q.head + 1) % q.ev.len();
        q.len -= 1;
    }
    if n != 0 {
        q.changes = q.changes.wrapping_add(1);
    }
    n as i32
}

unsafe extern "C" fn mouse_open(path: *const u8, len: usize, _out: *mut u8, _out_cap: usize) -> i32 {
    if !root(path, len) || HELD.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return -1;
    }
    let mut q = QUEUE.lock();
    q.head = 0;
    q.len = 0;
    q.changes = q.changes.wrapping_add(1);
    0
}

unsafe extern "C" fn mouse_release(path: *const u8, len: usize) -> i32 {
    if root(path, len) {
        HELD.store(false, Ordering::SeqCst);
    }
    0
}

/// Readable while events are queued; every event wakes the pollers. A
/// polled mouse (PS/2 without IRQ 12) is pumped here and re-checked.
unsafe extern "C" fn mouse_poll(_: *const u8, _: usize, changes: *mut u64) -> u32 {
    let polled = keyboard::pointer_polled();
    if polled {
        keyboard::pump();
    }
    let q = QUEUE.lock();
    // SAFETY: the kernel passes a live out-parameter.
    unsafe { *changes = q.changes };
    let recheck = if polled { MYOS_POLL_RECHECK } else { 0 };
    recheck | if q.len > 0 { MYOS_POLLIN } else { 0 }
}

/// Mount `/dev/console/mouse` and publish the [`POINTER_SERVICE`]. 0 ok,
/// negative on error.
pub fn init() -> i32 {
    // Built here, not in a static: aarch64 ET_EXEC modules do not relocate
    // fn pointers in .rodata (see netfs).
    let ops = ModuleVfsOps {
        lookup: mouse_lookup,
        stat: mouse_stat,
        listdir: mouse_listdir,
        register: None,
        read: Some(mouse_read),
        write: None,
        create: None,
        truncate: None,
        mkdir: None,
        rmdir: None,
        unlink: None,
        rename: None,
        symlink: None,
        readlink: None,
        release: Some(mouse_release),
        mmap: None,
        poll: Some(mouse_poll),
        open: Some(mouse_open),
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
    if api().vfs_mount("mouse", "dev/console/mouse", &ops) != 0 {
        return -1;
    }
    api().service_register(POINTER_SERVICE, &OPS)
}
