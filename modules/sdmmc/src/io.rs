//! `/dev/sdio`: SDIO cards for drivers that run as programs (the Raspberry
//! Pi's Wi-Fi chip, driven by the wifi daemon's `bwfm` crate, issue #547).
//!
//! A card the card layer brought up ([`sdio::Io`]) gets a directory,
//! `/dev/sdio/N`:
//!
//! - `info`: `id MMMM:PPPP` (the CIS's manufacturer and product),
//!   `functions N`, `bus 1|4`, `clock HZ`, the controller (`sdhci mmio
//!   0x...`), `block F N` per function with a block size, `irq
//!   on|off|none`, and `held` while a program drives it.
//! - `ctl`: held by one program at a time (root only) from its open on;
//!   reads as `info`. Writes: `blocksize F N` (function F's block size,
//!   into its FBR, for block-mode transfers), `irq on` and `irq off` (the
//!   card's interrupt at the controller). The files below are there while
//!   it is held; letting it go masks the interrupt.
//! - `fN`: function N's registers, the file position the register address
//!   (17 bits). A read or write of one byte is a CMD52, of more a CMD53
//!   with the address incrementing: byte mode up to 512 bytes, block mode
//!   for a whole number of the function's blocks (more than one).
//! - `fNfifo`: the same at a fixed address (a FIFO).
//! - `irq`: a read waits for the card's interrupt and returns how many
//!   came since the last read (8 bytes, little endian); `poll` says one is
//!   pending. The interrupt stays masked at the controller from the one
//!   that fired until a write to `irq` (a byte at least), once the driver
//!   quietened the card.
//!
//! The card's own interrupt enables (the CCCR's) are the driver's, through
//! `f0`. A transfer moves through the controller's buffer by the CPU, so a
//! program reaches no memory through the card.

use core::fmt::Write;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use myos_abi::{KernelApi, Lock, MYOS_POLLIN, MYOS_READ_WAIT, ModuleVfsOps, VfsStatInfo, status_fail};
use sdio::{Error, Io};

use crate::{Controller, Place, api, sdhci};

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
/// Cards listed.
pub const MAX_IO: usize = 2;
/// Bytes one read or write moves at most.
const TRANSFER_MAX: usize = 64 * 1024;

/// An SDIO card and the controller it is on.
pub struct Slot {
    pub place: Place,
    pub controller: Controller,
    pub io: Io,
    /// `ctl` is open.
    held: bool,
    /// The controller's interrupt (`irq_enable` took it), and whether the
    /// card's interrupt is unmasked there.
    irq: bool,
    irq_on: bool,
}

/// The cards, a lock per slot: a transfer holds its card's from the
/// command to its end. A slot is never emptied.
pub static SLOTS: [Lock<Option<Slot>>; MAX_IO] = [const { Lock::new(None) }; MAX_IO];
/// Per slot: card interrupts not read yet, and ever (`poll`'s change
/// count); the controller's registers, for the handler.
static PENDING: [AtomicU64; MAX_IO] = [const { AtomicU64::new(0) }; MAX_IO];
static TOTAL: [AtomicU64; MAX_IO] = [const { AtomicU64::new(0) }; MAX_IO];
static REGS: [AtomicUsize; MAX_IO] = [const { AtomicUsize::new(0) }; MAX_IO];

/// The card's interrupt: masked at the controller until the driver has
/// seen to it (a write to `irq`), counted, the reader woken.
unsafe extern "C" fn irq_handler(ctx: *mut core::ffi::c_void) {
    let i = ctx as usize;
    if i >= MAX_IO {
        return;
    }
    let regs = REGS[i].load(Ordering::Relaxed);
    if regs == 0 || !sdhci::card_irq_pending(regs) {
        return;
    }
    sdhci::card_irq_enable(regs, false);
    PENDING[i].fetch_add(1, Ordering::Relaxed);
    TOTAL[i].fetch_add(1, Ordering::Relaxed);
    api().wake_any();
}

pub fn known(place: Place) -> bool {
    SLOTS.iter().any(|s| s.lock().as_ref().is_some_and(|s| s.place == place))
}

/// Bring up the SDIO card behind `controller` into a free slot, as
/// `/dev/sdio/<slot>`; `irq` the controller's interrupt (0: none). Whether
/// one came up.
pub fn attach(api: &KernelApi, place: Place, mut controller: Controller, irq: u32) -> bool {
    let io = match Io::init(controller.host()) {
        Ok(io) => io,
        Err(_) => return false,
    };
    let Some((n, mut free)) = SLOTS.iter().enumerate().find_map(|(i, s)| {
        let s = s.lock();
        s.is_none().then_some((i, s))
    }) else {
        return false;
    };
    let regs = controller.sdhci_regs().unwrap_or(0);
    REGS[n].store(regs, Ordering::Relaxed);
    // The card interrupt is masked until a driver asks for it.
    if regs != 0 {
        sdhci::card_irq_enable(regs, false);
    }
    let irq = regs != 0 && irq != 0 && api.irq_enable(irq, "sdmmc", irq_handler, n as *mut _) == 0;
    *free = Some(Slot { place, controller, io, held: false, irq, irq_on: false });
    true
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum File {
    Info,
    Ctl,
    /// Function N's registers, incrementing or at a fixed address.
    Func(u8, bool),
    Irq,
}

enum Node {
    Root,
    Dir(usize),
    File(usize, File),
}

fn parse_file(s: &[u8], slot: &Slot) -> Option<File> {
    match s {
        b"info" => Some(File::Info),
        b"ctl" => Some(File::Ctl),
        b"irq" => Some(File::Irq),
        _ => {
            let rest = s.strip_prefix(b"f")?;
            let (digit, fifo) = match rest {
                [d] => (*d, false),
                [d, b'f', b'i', b'f', b'o'] => (*d, true),
                _ => return None,
            };
            let f = digit.checked_sub(b'0')?;
            (f <= slot.io.functions).then_some(File::Func(f, fifo))
        }
    }
}

fn node(path: *const u8, len: usize) -> Option<Node> {
    let path = if path.is_null() || len == 0 { &[][..] } else { unsafe { core::slice::from_raw_parts(path, len) } };
    let path = path.strip_suffix(b"/").unwrap_or(path);
    if path.is_empty() || path == b"." {
        return Some(Node::Root);
    }
    let (dir, file) = match path.iter().position(|&c| c == b'/') {
        Some(i) => (&path[..i], Some(&path[i + 1..])),
        None => (path, None),
    };
    let i = match dir {
        [d @ b'0'..=b'9'] => usize::from(d - b'0'),
        _ => return None,
    };
    let slot = SLOTS.get(i)?.lock();
    let slot = slot.as_ref()?;
    match file {
        None => Some(Node::Dir(i)),
        Some(f) => Some(Node::File(i, parse_file(f, slot)?)),
    }
}

/// A text buffer for `info` and the listings.
struct Text {
    buf: [u8; 512],
    len: usize,
}

impl Text {
    fn new() -> Self {
        Text { buf: [0; 512], len: 0 }
    }
    fn bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl Write for Text {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

fn info_text(i: usize, s: &Slot) -> Text {
    let mut t = Text::new();
    let io = &s.io;
    let _ = writeln!(t, "id {:04x}:{:04x}", io.manufacturer, io.product);
    let _ = writeln!(t, "functions {}", io.functions);
    let _ = writeln!(t, "bus {}", if io.wide { 4 } else { 1 });
    let _ = writeln!(t, "clock {}", if io.high_speed { sdio::CLOCK_HIGH } else { sdio::CLOCK_DEFAULT });
    let _ = match s.place {
        Place::Pci(bus, slot, func) => writeln!(t, "controller {} pci {bus:02x}:{slot:02x}.{func:x}", s.controller.name()),
        Place::Mmio(phys) => writeln!(t, "controller {} mmio {phys:#x}", s.controller.name()),
    };
    for (f, &b) in io.block.iter().enumerate().filter(|(_, b)| **b != 0) {
        let _ = writeln!(t, "block {f} {b}");
    }
    let _ = writeln!(t, "irq {}", if !s.irq { "none" } else if s.irq_on { "on" } else { "off" });
    if s.held {
        let _ = writeln!(t, "held");
    }
    let _ = i;
    t
}

/// The card's interrupt unmasked (`on`) or masked at the controller.
fn set_irq(i: usize, s: &mut Slot, on: bool) -> bool {
    if !s.irq {
        return false;
    }
    let regs = REGS[i].load(Ordering::Relaxed);
    s.irq_on = on;
    sdhci::card_irq_enable(regs, on);
    true
}

unsafe extern "C" fn sdio_lookup(_: *const u8, _: usize, _: *mut *const u8, _: *mut usize) -> i32 {
    -1
}

unsafe extern "C" fn sdio_stat(path: *const u8, len: usize, out: *mut VfsStatInfo) -> i32 {
    let Some(n) = node(path, len) else {
        return -1;
    };
    let (mode, size, ino) = match n {
        Node::Root => (S_IFDIR | 0o755, 0, 1),
        Node::Dir(i) => (S_IFDIR | 0o755, 0, 64 * (i as u64 + 1)),
        Node::File(i, f) => {
            let slot = SLOTS[i].lock();
            let s = slot.as_ref().unwrap();
            let (k, mode, size) = match f {
                File::Info => (0, 0o444, info_text(i, s).len as u64),
                File::Ctl => (1, 0o600, info_text(i, s).len as u64),
                File::Irq => (2, 0o600, 8),
                File::Func(f, fifo) => (3 + 2 * u64::from(f) + u64::from(fifo), 0o600, 0x20000),
            };
            (S_IFREG | mode, size, 64 * (i as u64 + 1) + k)
        }
    };
    let dir = matches!(n, Node::Root | Node::Dir(_));
    unsafe { *out = VfsStatInfo { mode, size, ino: ino as u32, nlink: if dir { 2 } else { 1 }, mtime: 0, atime: 0 } };
    0
}

unsafe extern "C" fn sdio_listdir(path: *const u8, len: usize, buf: *mut u8, cap: usize, out_len: *mut usize) -> i32 {
    let mut t = Text::new();
    match node(path, len) {
        Some(Node::Root) => {
            for (i, s) in SLOTS.iter().enumerate() {
                if s.lock().is_some() {
                    let _ = writeln!(t, "{i}");
                }
            }
        }
        Some(Node::Dir(i)) => {
            let slot = SLOTS[i].lock();
            let s = slot.as_ref().unwrap();
            let _ = write!(t, "info\nctl\n");
            if s.held {
                let _ = write!(t, "irq\n");
                for f in 0..=s.io.functions {
                    let _ = write!(t, "f{f}\nf{f}fifo\n");
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

/// `ctl`: one holder, root.
unsafe extern "C" fn sdio_open(path: *const u8, len: usize, _out: *mut u8, _out_cap: usize) -> i32 {
    match node(path, len) {
        Some(Node::File(i, File::Ctl)) => {
            let mut slot = SLOTS[i].lock();
            let s = slot.as_mut().unwrap();
            if s.held || api().current_uid() != 0 {
                return -1;
            }
            s.held = true;
            PENDING[i].store(0, Ordering::Relaxed);
            0
        }
        Some(Node::File(i, _)) => {
            if SLOTS[i].lock().as_ref().is_some_and(|s| s.held) { 0 } else { -1 }
        }
        _ => 0,
    }
}

/// The holder let go: the interrupt masked.
unsafe extern "C" fn sdio_release(path: *const u8, len: usize) -> i32 {
    if let Some(Node::File(i, File::Ctl)) = node(path, len) {
        let mut slot = SLOTS[i].lock();
        let s = slot.as_mut().unwrap();
        s.held = false;
        set_irq(i, s, false);
    }
    0
}

/// A register access from `fN` or `fNfifo`: `buf` at `pos`, `write` or
/// read. The bytes moved, or -1.
fn access(s: &mut Slot, f: u8, fifo: bool, pos: usize, buf: &mut [u8], write: bool) -> i32 {
    if pos >= 0x20000 || buf.is_empty() {
        return if write { -1 } else { 0 };
    }
    let addr = pos as u32;
    let n = buf.len().min(TRANSFER_MAX);
    let r: Result<(), Error> = if n == 1 {
        if write {
            s.io.write_byte(s.controller.host(), f, addr, buf[0])
        } else {
            s.io.read_byte(s.controller.host(), f, addr).map(|b| buf[0] = b)
        }
    } else {
        s.io.transfer(s.controller.host(), f, addr, !fifo, &mut buf[..n], write)
    };
    if r.is_err() { -1 } else { n as i32 }
}

unsafe extern "C" fn sdio_read(path: *const u8, len: usize, pos: usize, buf: *mut u8, cap: usize) -> i32 {
    let Some(Node::File(i, f)) = node(path, len) else {
        return -1;
    };
    let mut slot = SLOTS[i].lock();
    let s = slot.as_mut().unwrap();
    let buf = unsafe { core::slice::from_raw_parts_mut(buf, cap) };
    match f {
        File::Info | File::Ctl => {
            let t = info_text(i, s);
            let n = cap.min(t.len.saturating_sub(pos));
            buf[..n].copy_from_slice(&t.bytes()[pos..pos + n]);
            n as i32
        }
        File::Irq => {
            if !s.held {
                return -1;
            }
            let n = PENDING[i].swap(0, Ordering::Relaxed);
            if n == 0 {
                return MYOS_READ_WAIT;
            }
            if cap < 8 {
                PENDING[i].fetch_add(n, Ordering::Relaxed);
                return -1;
            }
            buf[..8].copy_from_slice(&n.to_le_bytes());
            8
        }
        File::Func(f, fifo) => {
            if !s.held {
                return -1;
            }
            access(s, f, fifo, pos, buf, false)
        }
    }
}

/// A decimal number.
fn number(s: &[u8]) -> Option<u32> {
    if s.is_empty() || s.len() > 9 {
        return None;
    }
    s.iter().try_fold(0u32, |v, &c| c.is_ascii_digit().then(|| v * 10 + u32::from(c - b'0')))
}

unsafe extern "C" fn sdio_write(path: *const u8, len: usize, pos: usize, buf: *const u8, n: usize) -> i32 {
    let Some(Node::File(i, f)) = node(path, len) else {
        return -1;
    };
    let mut slot = SLOTS[i].lock();
    let s = slot.as_mut().unwrap();
    let data = unsafe { core::slice::from_raw_parts(buf, n) };
    match f {
        File::Ctl => {
            let line = data.trim_ascii();
            let mut words = line.split(|&c| c == b' ').filter(|w| !w.is_empty());
            let ok = match (words.next(), words.next(), words.next(), words.next()) {
                (Some(b"blocksize"), Some(f), Some(size), None) => match (number(f), number(size)) {
                    (Some(f), Some(size)) if f < 8 && size <= 2048 => {
                        s.io.set_block_size(s.controller.host(), f as u8, size as u16).is_ok()
                    }
                    _ => false,
                },
                (Some(b"irq"), Some(b"on"), None, None) => set_irq(i, s, true),
                (Some(b"irq"), Some(b"off"), None, None) => set_irq(i, s, false),
                _ => false,
            };
            if ok { n as i32 } else { -1 }
        }
        File::Irq => {
            // The driver has quietened the card: its interrupt may fire again.
            if !s.held || !set_irq(i, s, true) {
                return -1;
            }
            n as i32
        }
        File::Func(f, fifo) => {
            if !s.held {
                return -1;
            }
            // `access` reads from `buf` only when writing.
            let data = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, n) };
            access(s, f, fifo, pos, data, true)
        }
        File::Info => -1,
    }
}

/// A shell's `>` truncates first: nothing to do but let it.
unsafe extern "C" fn sdio_truncate(path: *const u8, len: usize) -> i32 {
    match node(path, len) {
        Some(Node::File(_, File::Ctl | File::Irq | File::Func(..))) => 0,
        _ => -1,
    }
}

unsafe extern "C" fn sdio_poll(path: *const u8, len: usize, changes: *mut u64) -> u32 {
    let Some(Node::File(i, File::Irq)) = node(path, len) else {
        return 0;
    };
    if !changes.is_null() {
        unsafe { *changes = TOTAL[i].load(Ordering::Relaxed) };
    }
    if PENDING[i].load(Ordering::Relaxed) != 0 { MYOS_POLLIN } else { 0 }
}

/// `/dev/sdio`, empty until a card comes up.
pub fn mount(api: &KernelApi) {
    // Built here, not in a static: aarch64 ET_EXEC modules do not relocate
    // fn pointers in .rodata (see netfs).
    let ops = ModuleVfsOps {
        lookup: sdio_lookup,
        stat: sdio_stat,
        listdir: sdio_listdir,
        register: None,
        read: Some(sdio_read),
        write: Some(sdio_write),
        create: None,
        truncate: Some(sdio_truncate),
        mkdir: None,
        rmdir: None,
        unlink: None,
        rename: None,
        symlink: None,
        readlink: None,
        release: Some(sdio_release),
        mmap: None,
        poll: Some(sdio_poll),
        open: Some(sdio_open),
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
    if api.vfs_mount("sdio", "dev/sdio", &ops) != 0 {
        status_fail(api, "sdmmc: /dev/sdio");
    }
}
