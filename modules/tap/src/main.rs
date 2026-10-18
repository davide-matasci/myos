//! `/dev/tap`: virtual NICs, whose wire is a program. The Wi-Fi daemon
//! hands netd its link this way (issue #495): what netd sends, the program
//! reads; what the program writes, netd receives.
//!
//! A NIC `N` (0 to [`MAX_TAP`] - 1) is there while a program holds its
//! `/dev/tap/N/prog`: opening it makes the NIC (one holder at a time, root
//! only), the last close of it takes the NIC away. Its files:
//!
//! - `prog`: the program's end. A write is one Ethernet frame for netd; a
//!   read is one frame netd sent, and waits for one (`poll` says one is
//!   there).
//! - `data`: the NIC's end, as `/dev/netN/data` is: a write sends a frame,
//!   a read takes one (0 bytes when none waits: it never waits), `poll`
//!   says one is there.
//! - `ctl`: `mac aa:bb:cc:dd:ee:ff`, `irq on` (its `poll` works) and `link
//!   up` or `link down`, as lines. The program writes `mac ADDR` (the NIC's
//!   address), `up` and `down` (the link: a Wi-Fi association); a frame
//!   netd sends while the link is down is dropped.
//!
//! Each direction queues up to [`QUEUE`] frames; past that the newest is
//! dropped, as a full NIC drops.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicU64, Ordering};

use myos_abi::{
    status_ok, status_warn, ApiCell, KernelApi, Lock, ModuleVfsOps, VfsStatInfo, ABI_VERSION, MYOS_POLLIN,
    MYOS_POLLOUT, MYOS_READ_WAIT,
};

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const MAX_TAP: usize = 4;
/// Frames queued per direction.
const QUEUE: usize = 64;
/// The largest frame (an Ethernet frame with a VLAN tag, no FCS).
const FRAME_MAX: usize = 1518;

static API: ApiCell = ApiCell::new();

fn api() -> &'static KernelApi {
    API.get()
}

/// The kernel heap, through the ABI: the queues' frames.
struct KernelHeap;

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        API.get().alloc(layout.size(), layout.align())
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { (API.get().dealloc)(ptr, layout.size(), layout.align()) }
    }
}

#[global_allocator]
static HEAP: KernelHeap = KernelHeap;

struct Tap {
    mac: [u8; 6],
    up: bool,
    /// From the program to netd, and back.
    to_nic: VecDeque<Vec<u8>>,
    to_prog: VecDeque<Vec<u8>>,
}

static TAPS: Lock<[Option<Tap>; MAX_TAP]> = Lock::new([const { None }; MAX_TAP]);
/// Per NIC, `poll`'s change counts of each end.
static NIC_CHANGES: [AtomicU64; MAX_TAP] = [const { AtomicU64::new(0) }; MAX_TAP];
static PROG_CHANGES: [AtomicU64; MAX_TAP] = [const { AtomicU64::new(0) }; MAX_TAP];

#[derive(Clone, Copy, PartialEq, Eq)]
enum File {
    Prog,
    Data,
    Ctl,
}

enum Node {
    Root,
    Dir(usize),
    File(usize, File),
}

fn node(path: *const u8, len: usize) -> Option<Node> {
    let path = if path.is_null() || len == 0 { &[][..] } else { unsafe { core::slice::from_raw_parts(path, len) } };
    let path = path.strip_suffix(b"/").unwrap_or(path);
    if path.is_empty() || path == b"." {
        return Some(Node::Root);
    }
    let (n, rest) = path.split_first()?;
    let i = n.checked_sub(b'0').map(usize::from).filter(|&i| i < MAX_TAP)?;
    match rest {
        b"" => Some(Node::Dir(i)),
        b"/prog" => Some(Node::File(i, File::Prog)),
        b"/data" => Some(Node::File(i, File::Data)),
        b"/ctl" => Some(Node::File(i, File::Ctl)),
        _ => None,
    }
}

/// Whether NIC `i` is there; `prog` is, so that it can be opened.
fn there(taps: &[Option<Tap>; MAX_TAP], n: &Node) -> bool {
    match *n {
        Node::Root | Node::File(_, File::Prog) => true,
        Node::Dir(i) | Node::File(i, _) => taps[i].is_some(),
    }
}

fn hex(out: &mut Vec<u8>, b: u8) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    out.push(DIGITS[(b >> 4) as usize]);
    out.push(DIGITS[(b & 15) as usize]);
}

fn ctl_text(t: &Tap) -> Vec<u8> {
    let mut out = Vec::from(b"mac " as &[u8]);
    for (i, b) in t.mac.iter().enumerate() {
        if i != 0 {
            out.push(b':');
        }
        hex(&mut out, *b);
    }
    out.extend_from_slice(b"\nirq on\nlink ");
    out.extend_from_slice(if t.up { b"up\n" } else { b"down\n" });
    out
}

/// `aa:bb:cc:dd:ee:ff`.
fn parse_mac(text: &[u8]) -> Option<[u8; 6]> {
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut mac = [0u8; 6];
    let mut parts = text.split(|&b| b == b':');
    for byte in mac.iter_mut() {
        let p = parts.next()?;
        if p.len() != 2 {
            return None;
        }
        *byte = hex(p[0])? << 4 | hex(p[1])?;
    }
    parts.next().is_none().then_some(mac)
}

unsafe extern "C" fn tap_lookup(_: *const u8, _: usize, _: *mut *const u8, _: *mut usize) -> i32 {
    -1
}

unsafe extern "C" fn tap_stat(path: *const u8, len: usize, out: *mut VfsStatInfo) -> i32 {
    let taps = TAPS.lock();
    let Some(n) = node(path, len).filter(|n| there(&taps, n)) else {
        return -1;
    };
    let (mode, ino, size) = match n {
        Node::Root => (S_IFDIR | 0o755, 1, 0),
        Node::Dir(i) => (S_IFDIR | 0o755, 16 * (i as u32 + 1), 0),
        Node::File(i, f) => {
            let size = match (&taps[i], f) {
                (Some(t), File::Ctl) => ctl_text(t).len() as u64,
                _ => 0,
            };
            (S_IFREG | 0o600, 16 * (i as u32 + 1) + 1 + f as u32, size)
        }
    };
    let dir = matches!(n, Node::Root | Node::Dir(_));
    unsafe { *out = VfsStatInfo { mode, size, ino, nlink: if dir { 2 } else { 1 }, mtime: 0, atime: 0 } };
    0
}

unsafe extern "C" fn tap_listdir(path: *const u8, len: usize, buf: *mut u8, cap: usize, out_len: *mut usize) -> i32 {
    let taps = TAPS.lock();
    let list: Vec<u8> = match node(path, len) {
        Some(Node::Root) => (0..MAX_TAP).filter(|&i| taps[i].is_some()).flat_map(|i| [b'0' + i as u8, b'\n']).collect(),
        Some(Node::Dir(i)) if taps[i].is_some() => Vec::from(b"prog\ndata\nctl\n" as &[u8]),
        _ => return -1,
    };
    if list.len() > cap {
        return -1;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(list.as_ptr(), buf, list.len());
        *out_len = list.len();
    }
    0
}

/// `prog`: the NIC comes with its program, one at a time, root.
unsafe extern "C" fn tap_open(path: *const u8, len: usize) -> i32 {
    let Some(Node::File(i, f)) = node(path, len) else {
        return 0;
    };
    let mut taps = TAPS.lock();
    match f {
        File::Prog => {
            if taps[i].is_some() || api().current_uid() != 0 {
                return -1;
            }
            taps[i] = Some(Tap { mac: [0x02, 0, 0, 0x7a, 0x70, i as u8], up: false, to_nic: VecDeque::new(), to_prog: VecDeque::new() });
            0
        }
        _ if taps[i].is_none() => -1,
        _ => 0,
    }
}

/// The program's last close takes its NIC away.
unsafe extern "C" fn tap_release(path: *const u8, len: usize) -> i32 {
    if let Some(Node::File(i, File::Prog)) = node(path, len) {
        TAPS.lock()[i] = None;
        NIC_CHANGES[i].fetch_add(1, Ordering::Relaxed);
        api().wake_any();
    }
    0
}

unsafe extern "C" fn tap_read(path: *const u8, len: usize, pos: usize, buf: *mut u8, cap: usize) -> i32 {
    let Some(Node::File(i, f)) = node(path, len) else {
        return -1;
    };
    let mut taps = TAPS.lock();
    let Some(t) = taps[i].as_mut() else {
        return -1;
    };
    let frame = match f {
        File::Ctl => {
            let text = ctl_text(t);
            let n = cap.min(text.len().saturating_sub(pos));
            if n != 0 {
                unsafe { core::ptr::copy_nonoverlapping(text[pos..].as_ptr(), buf, n) };
            }
            return n as i32;
        }
        File::Prog => match t.to_prog.pop_front() {
            Some(frame) => frame,
            None => return MYOS_READ_WAIT,
        },
        File::Data => match t.to_nic.pop_front() {
            Some(frame) => frame,
            None => return 0,
        },
    };
    // A frame is read whole or not at all.
    if frame.len() > cap {
        return -1;
    }
    unsafe { core::ptr::copy_nonoverlapping(frame.as_ptr(), buf, frame.len()) };
    frame.len() as i32
}

unsafe extern "C" fn tap_write(path: *const u8, len: usize, _pos: usize, buf: *const u8, n: usize) -> i32 {
    let Some(Node::File(i, f)) = node(path, len) else {
        return -1;
    };
    let data = unsafe { core::slice::from_raw_parts(buf, n) };
    let mut taps = TAPS.lock();
    let Some(t) = taps[i].as_mut() else {
        return -1;
    };
    let (queue, changes) = match f {
        File::Ctl => {
            let cmd = data.trim_ascii();
            match cmd {
                b"up" => t.up = true,
                b"down" => t.up = false,
                _ => match cmd.strip_prefix(b"mac ").and_then(parse_mac) {
                    Some(mac) => t.mac = mac,
                    None => return -1,
                },
            }
            NIC_CHANGES[i].fetch_add(1, Ordering::Relaxed);
            return n as i32;
        }
        File::Prog => (&mut t.to_nic, &NIC_CHANGES[i]),
        // netd's frames go nowhere while the link is down.
        File::Data if !t.up => return n as i32,
        File::Data => (&mut t.to_prog, &PROG_CHANGES[i]),
    };
    if n == 0 || n > FRAME_MAX {
        return -1;
    }
    if queue.len() < QUEUE {
        queue.push_back(Vec::from(data));
        changes.fetch_add(1, Ordering::Relaxed);
        drop(taps);
        api().wake_any();
    }
    n as i32
}

unsafe extern "C" fn tap_poll(path: *const u8, len: usize, changes: *mut u64) -> u32 {
    let Some(Node::File(i, f)) = node(path, len) else {
        return 0;
    };
    let taps = TAPS.lock();
    let Some(t) = taps[i].as_ref() else {
        return 0;
    };
    let (waiting, count) = match f {
        File::Prog => (!t.to_prog.is_empty(), &PROG_CHANGES[i]),
        File::Data => (!t.to_nic.is_empty(), &NIC_CHANGES[i]),
        File::Ctl => (false, &NIC_CHANGES[i]),
    };
    if !changes.is_null() {
        unsafe { *changes = count.load(Ordering::Relaxed) };
    }
    MYOS_POLLOUT | if waiting { MYOS_POLLIN } else { 0 }
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
    // Built here, not in a static: aarch64 ET_EXEC modules do not relocate
    // fn pointers in .rodata (see netfs).
    let ops = ModuleVfsOps {
        lookup: tap_lookup,
        stat: tap_stat,
        listdir: tap_listdir,
        register: None,
        read: Some(tap_read),
        write: Some(tap_write),
        create: None,
        truncate: None,
        mkdir: None,
        rmdir: None,
        unlink: None,
        rename: None,
        symlink: None,
        readlink: None,
        release: Some(tap_release),
        mmap: None,
        poll: Some(tap_poll),
        open: Some(tap_open),
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
    if api.vfs_mount("tap", "dev/tap", &ops) != 0 {
        status_warn(api, "tap: /dev/tap");
        return -3;
    }
    status_ok(api, "tap");
    0
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

// So `cargo build --bin tap` links. The kernel never jumps here.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
