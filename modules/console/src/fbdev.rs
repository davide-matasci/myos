//! `/dev/fb`: the screen for programs that draw on it themselves.
//!
//! - `ctl` reads as one line: `width height depth chan pitch mode`, e.g.
//!   `1280 800 32 x8r8g8b8 5120 text`. `chan` names the pixel layout from
//!   the most significant bits down, as Plan 9 does (`x` is unused bits).
//!   Writing `graphics` takes the screen: this module stops painting text
//!   and blinking the cursor on it. Writing `text`, or closing the last fd
//!   of `ctl` (a program's exit too), gives it back, cleared.
//! - `data` is the pixels: `read`/`write` at an offset (`cat data` is a
//!   screenshot), or `mmap(MAP_SHARED)`, which maps the framebuffer itself
//!   (the kernel asks [`fb_mmap`] for each page).
//!
//! One framebuffer, at the mode the bootloader set; no mode setting.

use core::sync::atomic::{AtomicBool, Ordering};

use myos_abi::{FramebufferInfo, Lock, ModuleVfsOps, VfsStatInfo};

use crate::{FB, api};

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const PAGE: usize = 4096;

/// The screen belongs to a program (`graphics` written to `ctl`).
static GRAPHICS: AtomicBool = AtomicBool::new(false);
/// The framebuffer, from [`mount`] on.
static INFO: Lock<Option<FramebufferInfo>> = Lock::new(None);

/// Whether the text console must leave the screen alone.
pub fn graphics() -> bool {
    GRAPHICS.load(Ordering::Relaxed)
}

fn info() -> Option<FramebufferInfo> {
    *INFO.lock()
}

/// Bytes of pixel memory (`pitch * height`).
fn fb_len(fb: &FramebufferInfo) -> usize {
    (fb.pitch * fb.height) as usize
}

/// Pixel memory from `pos` on, for a copy of up to `fb_len(fb) - pos`
/// bytes: `read_pixels` and `write_pixels` check that. Through a raw
/// pointer, never a slice: the text console paints the same memory, and a
/// program's `mmap` of it is not this module's to see.
fn pixels(fb: &FramebufferInfo, pos: usize) -> *mut u8 {
    (fb.addr as *mut u8).wrapping_add(pos)
}

/// `cap` bytes of pixel memory from `pos` into `buf`, fewer at the end;
/// how many.
fn read_pixels(fb: &FramebufferInfo, pos: usize, buf: *mut u8, cap: usize) -> usize {
    let n = cap.min(fb_len(fb).saturating_sub(pos));
    // SAFETY: the kernel mapped `fb.addr..fb.addr + fb_len(fb)` for the
    // module (`framebuffer_info`), `pos + n` is within it, and `buf` is
    // the kernel's buffer of `cap` bytes.
    unsafe { core::ptr::copy_nonoverlapping(pixels(fb, pos), buf, n) };
    n
}

/// `len` bytes of `buf` into pixel memory at `pos`, fewer at the end; how
/// many.
fn write_pixels(fb: &FramebufferInfo, pos: usize, buf: *const u8, len: usize) -> usize {
    let n = len.min(fb_len(fb).saturating_sub(pos));
    // SAFETY: as for `read_pixels`, with `buf` the kernel's `len` bytes.
    unsafe { core::ptr::copy_nonoverlapping(buf, pixels(fb, pos), n) };
    n
}

/// Give the screen back to the text console: cleared, since it cannot
/// repaint what was drawn over it.
fn leave_graphics() {
    if GRAPHICS.swap(false, Ordering::Relaxed) {
        if let Some(w) = FB.lock().as_mut() {
            w.clear();
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Node {
    Root,
    Ctl,
    Data,
}

fn node(path: *const u8, len: usize) -> Option<Node> {
    let path = if path.is_null() || len == 0 {
        &[][..]
    } else {
        unsafe { core::slice::from_raw_parts(path, len) }
    };
    match path {
        b"" | b"." => Some(Node::Root),
        b"ctl" => Some(Node::Ctl),
        b"data" => Some(Node::Data),
        _ => None,
    }
}

/// Small text buffer for the `ctl` line.
struct Line {
    buf: [u8; 64],
    len: usize,
}

impl Line {
    fn push(&mut self, s: &[u8]) {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s[..n]);
        self.len += n;
    }

    fn num(&mut self, mut v: u64) {
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
}

/// The `chan` word: channels from the most significant bit down (`x8r8g8b8`).
fn chan(fb: &FramebufferInfo, out: &mut Line) {
    let mut ch = [
        (fb.r_shift, fb.r_size, b'r'),
        (fb.g_shift, fb.g_size, b'g'),
        (fb.b_shift, fb.b_size, b'b'),
    ];
    ch.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    let mut top = fb.bpp as u64;
    for (shift, size, name) in ch {
        let end = shift as u64 + size as u64;
        if end < top {
            out.push(b"x");
            out.num(top - end);
        }
        out.push(&[name]);
        out.num(size as u64);
        top = shift as u64;
    }
    if top > 0 {
        out.push(b"x");
        out.num(top);
    }
}

fn ctl_line(fb: &FramebufferInfo) -> Line {
    let mut l = Line { buf: [0; 64], len: 0 };
    for v in [fb.width, fb.height, fb.bpp as u64] {
        l.num(v);
        l.push(b" ");
    }
    chan(fb, &mut l);
    l.push(b" ");
    l.num(fb.pitch);
    l.push(if graphics() { b" graphics\n" } else { b" text\n" });
    l
}

unsafe extern "C" fn fb_lookup(_: *const u8, _: usize, _: *mut *const u8, _: *mut usize) -> i32 {
    -1
}

unsafe extern "C" fn fb_stat(path: *const u8, len: usize, out: *mut VfsStatInfo) -> i32 {
    let (Some(node), Some(fb)) = (node(path, len), info()) else {
        return -1;
    };
    let (mode, size, ino) = match node {
        Node::Root => (S_IFDIR | 0o755, 0, 1),
        Node::Ctl => (S_IFREG | 0o666, ctl_line(&fb).len, 2),
        Node::Data => (S_IFREG | 0o666, fb_len(&fb), 3),
    };
    unsafe {
        *out = VfsStatInfo {
            mode,
            size: size as u32,
            ino,
            nlink: if node == Node::Root { 2 } else { 1 },
            mtime: 0,
            atime: 0,
            size_hi: 0,
        };
    }
    0
}

unsafe extern "C" fn fb_listdir(
    path: *const u8,
    len: usize,
    buf: *mut u8,
    cap: usize,
    out_len: *mut usize,
) -> i32 {
    const LIST: &[u8] = b"ctl\ndata\n";
    if node(path, len) != Some(Node::Root) || cap < LIST.len() {
        return -1;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(LIST.as_ptr(), buf, LIST.len());
        *out_len = LIST.len();
    }
    0
}

unsafe extern "C" fn fb_read(path: *const u8, len: usize, pos: usize, buf: *mut u8, cap: usize) -> i32 {
    let (Some(node), Some(fb)) = (node(path, len), info()) else {
        return -1;
    };
    match node {
        Node::Root => -1,
        Node::Ctl => {
            let line = ctl_line(&fb);
            let n = cap.min(line.len.saturating_sub(pos));
            if n != 0 {
                unsafe { core::ptr::copy_nonoverlapping(line.buf[pos..].as_ptr(), buf, n) };
            }
            n as i32
        }
        Node::Data => read_pixels(&fb, pos, buf, cap) as i32,
    }
}

unsafe extern "C" fn fb_write(path: *const u8, len: usize, pos: usize, buf: *const u8, n: usize) -> i32 {
    let (Some(node), Some(fb)) = (node(path, len), info()) else {
        return -1;
    };
    match node {
        Node::Root => -1,
        Node::Ctl => match unsafe { core::slice::from_raw_parts(buf, n) }.trim_ascii() {
            b"graphics" => {
                GRAPHICS.store(true, Ordering::Relaxed);
                n as i32
            }
            b"text" => {
                leave_graphics();
                n as i32
            }
            _ => -1,
        },
        Node::Data => write_pixels(&fb, pos, buf, n) as i32,
    }
}

/// The last fd of `ctl` closed: whoever held the screen is done with it.
unsafe extern "C" fn fb_release(path: *const u8, len: usize) -> i32 {
    if node(path, len) == Some(Node::Ctl) {
        leave_graphics();
    }
    0
}

/// `mmap` of `data`: the framebuffer page at `offset` (its physical address).
unsafe extern "C" fn fb_mmap(path: *const u8, len: usize, offset: usize) -> u64 {
    let (Some(Node::Data), Some(fb)) = (node(path, len), info()) else {
        return 0;
    };
    let Some(base) = fb.addr.checked_sub(api().hhdm_offset()) else {
        return 0;
    };
    if base % PAGE as u64 != 0 || offset % PAGE != 0 || offset >= fb_len(&fb).next_multiple_of(PAGE) {
        return 0;
    }
    base + offset as u64
}

/// Mount `/dev/fb` over the framebuffer `fb`. 0 ok, negative on error.
pub fn mount(fb: FramebufferInfo) -> i32 {
    *INFO.lock() = Some(fb);
    // Built here, not in a static: aarch64 ET_EXEC modules do not relocate
    // fn pointers in .rodata (see netfs).
    let ops = ModuleVfsOps {
        lookup: fb_lookup,
        stat: fb_stat,
        listdir: fb_listdir,
        register: None,
        read: Some(fb_read),
        write: Some(fb_write),
        create: None,
        truncate: None,
        mkdir: None,
        rmdir: None,
        unlink: None,
        rename: None,
        symlink: None,
        readlink: None,
        release: Some(fb_release),
        mmap: Some(fb_mmap),
        poll: None,
        open: None,
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
    };
    api().vfs_mount("fb", "dev/fb", &ops)
}
