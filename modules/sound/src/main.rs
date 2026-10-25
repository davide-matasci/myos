//! The sound core: `/dev/sound`, the PCM streams of the sound cards.
//!
//! A card's driver (`virtio_sound`, `usb_audio`) registers the card through
//! the [`SOUND_SERVICE`] table with its streams, each a playback or a
//! capture stream; stream `N` (numbered across the cards, in the order they
//! came) is the directory `/dev/sound/N`, for the sound server (`sndd`,
//! `docs/sound.md`):
//!
//! - `ctl`: what the stream is and does, as lines (`card NAME`,
//!   `direction playback|capture`, `channels MIN-MAX`, `formats u8 s16`,
//!   `rates ...`, `state stopped` or `state running RATE CHANNELS FORMAT
//!   period BYTES buffer BYTES`, `underruns N`, `overruns N`). Writes:
//!   `start RATE CHANNELS FORMAT [PERIOD_FRAMES]` (20 ms periods without
//!   one) and `stop`.
//! - `data`: the PCM, interleaved frames, little endian; held by one
//!   program at a time, whose last close stops the stream. A playback
//!   stream's writes take what fits in its buffer (four periods; `poll`
//!   says writable while a period fits); a capture stream's reads return
//!   what was recorded and wait for some while it runs.
//!
//! Between the file and the driver each stream has its buffer: the driver
//! takes what plays a period at a time (silence for what the server has
//! not written: an underrun) and hands over what it recorded (the oldest
//! dropped when the server does not read it: an overrun).

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering};

use myos_abi::{
    status_fail, status_ok, ApiCell, KernelApi, Lock, ModuleVfsOps, SoundCardOps, SoundOps, SoundParams,
    SoundStreamInfo, StrRef, VfsStatInfo, ABI_VERSION, MYOS_POLLERR, MYOS_POLLHUP, MYOS_POLLIN, MYOS_POLLOUT,
    MYOS_READ_WAIT, SOUND_CAPTURE, SOUND_FORMAT_S16, SOUND_FORMAT_U8, SOUND_PLAYBACK, SOUND_RATES, SOUND_SERVICE,
    SOUND_VERSION,
};

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const MAX_CARDS: usize = 8;
const MAX_STREAMS: usize = 16;
/// Periods a stream's buffer holds.
const PERIODS: u32 = 4;
/// The longest period, and the longest card name kept.
const PERIOD_MAX: u32 = 64 * 1024;
const NAME_MAX: usize = 48;

static API: ApiCell = ApiCell::new();

fn api() -> &'static KernelApi {
    API.get()
}

/// The kernel heap, through the ABI: the streams' buffers.
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

struct Card {
    ops: &'static SoundCardOps,
    ctx: usize,
    name: [u8; NAME_MAX],
    name_len: usize,
    /// Its streams are `first..first + n`.
    first: usize,
    n: usize,
    gone: bool,
}

/// A ring of bytes: `len` of them from `head`.
struct Ring {
    buf: Vec<u8>,
    head: usize,
    len: usize,
}

impl Ring {
    const fn new() -> Ring {
        Ring { buf: Vec::new(), head: 0, len: 0 }
    }

    fn reset(&mut self, cap: usize) {
        self.buf = vec![0; cap];
        self.head = 0;
        self.len = 0;
    }

    fn free(&self) -> usize {
        self.buf.len() - self.len
    }

    /// Up to `data.len()` bytes in at the tail: how many fit.
    fn push(&mut self, data: &[u8]) -> usize {
        let n = data.len().min(self.free());
        let cap = self.buf.len();
        for (i, b) in data[..n].iter().enumerate() {
            self.buf[(self.head + self.len + i) % cap] = *b;
        }
        self.len += n;
        n
    }

    /// Up to `out.len()` bytes from the head: how many.
    fn pop(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.len);
        let cap = self.buf.len();
        for (i, b) in out[..n].iter_mut().enumerate() {
            *b = self.buf[(self.head + i) % cap];
        }
        self.head = (self.head + n) % cap.max(1);
        self.len -= n;
        n
    }

    fn drop_front(&mut self, n: usize) {
        let n = n.min(self.len);
        self.head = (self.head + n) % self.buf.len().max(1);
        self.len -= n;
    }
}

struct Stream {
    card: usize,
    /// Its number among its card's streams.
    index: u32,
    info: SoundStreamInfo,
    /// How it runs, while it does.
    running: Option<SoundParams>,
    ring: Ring,
    /// A program holds `data`.
    held: bool,
    underruns: u64,
    overruns: u64,
}

struct Core {
    cards: [Option<Card>; MAX_CARDS],
    streams: [Option<Stream>; MAX_STREAMS],
}

static CORE: Lock<Core> = Lock::new(Core { cards: [const { None }; MAX_CARDS], streams: [const { None }; MAX_STREAMS] });
/// Per stream, `poll`'s change count of `data`.
static CHANGES: [AtomicU64; MAX_STREAMS] = [const { AtomicU64::new(0) }; MAX_STREAMS];

fn changed(n: usize) {
    CHANGES[n].fetch_add(1, Ordering::Relaxed);
    api().wake_any();
}

/// Bytes a frame of `channels` channels of `format` takes.
fn frame_bytes(channels: u8, format: u8) -> u32 {
    u32::from(channels) * if format == SOUND_FORMAT_S16 { 2 } else { 1 }
}

fn format_name(format: u8) -> &'static str {
    if format == SOUND_FORMAT_S16 { "s16" } else { "u8" }
}

// ---- The drivers' table ------------------------------------------------------

static OPS: SoundOps = SoundOps { version: SOUND_VERSION, card_register, card_remove, playback_take, capture_put };

unsafe extern "C" fn card_register(
    ops: *const SoundCardOps,
    ctx: usize,
    name: StrRef,
    streams: *const SoundStreamInfo,
    n: u32,
) -> i32 {
    // SAFETY: a driver's table lives as long as its module, which stays
    // loaded while its card is registered.
    let Some(ops) = (unsafe { ops.as_ref() }) else {
        return -1;
    };
    if ops.version != SOUND_VERSION || streams.is_null() || n == 0 {
        return -1;
    }
    // SAFETY: the driver hands `n` stream descriptions and its name.
    let infos = unsafe { core::slice::from_raw_parts(streams, n as usize) };
    let name = if name.ptr.is_null() { &[][..] } else { unsafe { core::slice::from_raw_parts(name.ptr, name.len) } };
    let mut core = CORE.lock();
    let Some(card) = core.cards.iter().position(Option::is_none) else {
        return -1;
    };
    // A card's streams are consecutive; the numbers of a removed card's
    // streams are not given out again.
    let first = core.streams.iter().rposition(Option::is_some).map_or(0, |i| i + 1);
    if first + infos.len() > MAX_STREAMS {
        return -1;
    }
    let mut kept = [0u8; NAME_MAX];
    let name_len = name.len().min(NAME_MAX);
    kept[..name_len].copy_from_slice(&name[..name_len]);
    core.cards[card] = Some(Card { ops, ctx, name: kept, name_len, first, n: infos.len(), gone: false });
    for (i, info) in infos.iter().enumerate() {
        core.streams[first + i] = Some(Stream {
            card,
            index: i as u32,
            info: *info,
            running: None,
            ring: Ring::new(),
            held: false,
            underruns: 0,
            overruns: 0,
        });
    }
    card as i32
}

unsafe extern "C" fn card_remove(card: i32) {
    let mut core = CORE.lock();
    let Some(c) = usize::try_from(card).ok().and_then(|i| core.cards.get_mut(i)).and_then(Option::as_mut) else {
        return;
    };
    c.gone = true;
    let (first, n) = (c.first, c.n);
    for i in first..first + n {
        if let Some(s) = core.streams[i].as_mut() {
            s.running = None;
        }
        CHANGES[i].fetch_add(1, Ordering::Relaxed);
    }
    drop(core);
    api().wake_any();
}

/// Stream `stream` of card `card`: its number.
fn stream_of(core: &Core, card: i32, stream: u32) -> Option<usize> {
    let c = core.cards.get(usize::try_from(card).ok()?)?.as_ref()?;
    (stream < c.n as u32).then_some(c.first + stream as usize)
}

unsafe extern "C" fn playback_take(card: i32, stream: u32, buf: *mut u8, len: usize) -> usize {
    if buf.is_null() || len == 0 {
        return 0;
    }
    // SAFETY: the driver hands `len` bytes at `buf`.
    let out = unsafe { core::slice::from_raw_parts_mut(buf, len) };
    let mut core = CORE.lock();
    let Some(n) = stream_of(&core, card, stream) else {
        return 0;
    };
    let Some(s) = core.streams[n].as_mut().filter(|s| s.running.is_some()) else {
        return 0;
    };
    let got = s.ring.pop(out);
    if got < len && s.held {
        s.underruns += 1;
    }
    drop(core);
    if got != 0 {
        changed(n);
    }
    got
}

unsafe extern "C" fn capture_put(card: i32, stream: u32, buf: *const u8, len: usize) {
    if buf.is_null() || len == 0 {
        return;
    }
    // SAFETY: the driver hands `len` bytes at `buf`.
    let data = unsafe { core::slice::from_raw_parts(buf, len) };
    let mut core = CORE.lock();
    let Some(n) = stream_of(&core, card, stream) else {
        return;
    };
    let Some(s) = core.streams[n].as_mut().filter(|s| s.running.is_some()) else {
        return;
    };
    let data = &data[data.len().saturating_sub(s.ring.buf.len())..];
    if data.len() > s.ring.free() {
        s.ring.drop_front(data.len() - s.ring.free());
        s.overruns += 1;
    }
    s.ring.push(data);
    drop(core);
    changed(n);
}

// ---- Starting and stopping ---------------------------------------------------

/// `start RATE CHANNELS FORMAT [PERIOD_FRAMES]` on stream `n`, if the card
/// can: 0, or negative.
fn start(n: usize, args: &[u8]) -> i32 {
    let mut words = args.split(|b| b.is_ascii_whitespace()).filter(|w| !w.is_empty());
    let number = |w: Option<&[u8]>| core::str::from_utf8(w?).ok()?.parse::<u32>().ok();
    let (Some(rate), Some(channels), Some(format)) = (number(words.next()), number(words.next()), words.next()) else {
        return -1;
    };
    let format = match format {
        b"u8" => SOUND_FORMAT_U8,
        b"s16" => SOUND_FORMAT_S16,
        _ => return -1,
    };
    let period_frames = match words.next() {
        Some(w) => match number(Some(w)) {
            Some(f) if f != 0 => f,
            _ => return -1,
        },
        None => (rate / 50).max(1),
    };
    let mut core = CORE.lock();
    let Some(s) = core.streams[n].as_ref() else {
        return -1;
    };
    let info = s.info;
    let Ok(channels) = u8::try_from(channels) else {
        return -1;
    };
    let rate_ok = SOUND_RATES.iter().enumerate().any(|(i, &r)| r == rate && info.rates & 1 << i != 0);
    if !rate_ok || !(info.channels_min..=info.channels_max).contains(&channels) || info.formats & 1 << format == 0 {
        return -1;
    }
    let period_bytes = period_frames.saturating_mul(frame_bytes(channels, format));
    if period_bytes > PERIOD_MAX || s.running.is_some() {
        return -1;
    }
    let params = SoundParams { rate, channels, format, _pad: 0, period_bytes, buffer_bytes: period_bytes * PERIODS };
    let Some(card) = core.cards[s.card].as_ref().filter(|c| !c.gone) else {
        return -1;
    };
    let (ops, ctx, index) = (card.ops, card.ctx, s.index);
    let s = core.streams[n].as_mut().unwrap();
    s.ring.reset(params.buffer_bytes as usize);
    s.running = Some(params);
    drop(core);
    // SAFETY: the card's driver is loaded while its card is registered.
    if unsafe { (ops.start)(ctx, index, &params) } != 0 {
        if let Some(s) = CORE.lock().streams[n].as_mut() {
            s.running = None;
        }
        return -1;
    }
    changed(n);
    0
}

/// Stop stream `n` if it runs.
fn stop(n: usize) {
    let mut core = CORE.lock();
    let Some(s) = core.streams[n].as_mut() else {
        return;
    };
    if s.running.take().is_none() {
        return;
    }
    let (card, index) = (s.card, s.index);
    let target = core.cards[card].as_ref().filter(|c| !c.gone).map(|c| (c.ops, c.ctx));
    drop(core);
    if let Some((ops, ctx)) = target {
        // SAFETY: as in `start`.
        unsafe { (ops.stop)(ctx, index) };
    }
    changed(n);
}

// ---- /dev/sound ----------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum File {
    Ctl,
    Data,
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
    let (num, rest) = match path.iter().position(|&b| b == b'/') {
        Some(i) => path.split_at(i),
        None => (path, &[][..]),
    };
    let n: usize = core::str::from_utf8(num).ok()?.parse().ok().filter(|&n| n < MAX_STREAMS)?;
    match rest {
        b"" => Some(Node::Dir(n)),
        b"/ctl" => Some(Node::File(n, File::Ctl)),
        b"/data" => Some(Node::File(n, File::Data)),
        _ => None,
    }
}

/// A stream whose card is still there.
fn live(core: &Core, n: usize) -> Option<&Stream> {
    let s = core.streams[n].as_ref()?;
    core.cards[s.card].as_ref().filter(|c| !c.gone).map(|_| s)
}

/// `buf` as text, for `ctl`.
struct Text(Vec<u8>);

impl Write for Text {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

fn ctl_text(core: &Core, n: usize) -> Option<Vec<u8>> {
    let s = live(core, n)?;
    let card = core.cards[s.card].as_ref()?;
    let mut t = Text(Vec::new());
    let name = core::str::from_utf8(&card.name[..card.name_len]).unwrap_or("?");
    let dir = if s.info.direction == SOUND_CAPTURE { "capture" } else { "playback" };
    let _ = writeln!(t, "card {name}\nstream {}\ndirection {dir}", s.index);
    let _ = writeln!(t, "channels {}-{}", s.info.channels_min, s.info.channels_max);
    let _ = write!(t, "formats");
    for format in [SOUND_FORMAT_U8, SOUND_FORMAT_S16] {
        if s.info.formats & 1 << format != 0 {
            let _ = write!(t, " {}", format_name(format));
        }
    }
    let _ = write!(t, "\nrates");
    for (i, rate) in SOUND_RATES.iter().enumerate() {
        if s.info.rates & 1 << i != 0 {
            let _ = write!(t, " {rate}");
        }
    }
    let _ = match s.running {
        Some(p) => writeln!(
            t,
            "\nstate running {} {} {} period {} buffer {}",
            p.rate,
            p.channels,
            format_name(p.format),
            p.period_bytes,
            p.buffer_bytes
        ),
        None => writeln!(t, "\nstate stopped"),
    };
    let _ = writeln!(t, "underruns {}\noverruns {}", s.underruns, s.overruns);
    Some(t.0)
}

unsafe extern "C" fn sound_lookup(_: *const u8, _: usize, _: *mut *const u8, _: *mut usize) -> i32 {
    -1
}

unsafe extern "C" fn sound_stat(path: *const u8, len: usize, out: *mut VfsStatInfo) -> i32 {
    let core = CORE.lock();
    let (mode, ino, size) = match node(path, len) {
        Some(Node::Root) => (S_IFDIR | 0o755, 1, 0),
        Some(Node::Dir(n)) if live(&core, n).is_some() => (S_IFDIR | 0o755, 16 * (n as u64 + 1), 0),
        Some(Node::File(n, f)) if live(&core, n).is_some() => {
            let size = if f == File::Ctl { ctl_text(&core, n).map_or(0, |t| t.len() as u64) } else { 0 };
            (S_IFREG | 0o600, 16 * (n as u64 + 1) + 1 + f as u64, size)
        }
        _ => return -1,
    };
    let dir = mode & S_IFDIR != 0;
    unsafe { *out = VfsStatInfo { mode, size, ino: ino as u32, nlink: if dir { 2 } else { 1 }, mtime: 0, atime: 0 } };
    0
}

unsafe extern "C" fn sound_listdir(path: *const u8, len: usize, buf: *mut u8, cap: usize, out_len: *mut usize) -> i32 {
    let core = CORE.lock();
    let mut list = Text(Vec::new());
    match node(path, len) {
        Some(Node::Root) => {
            for n in (0..MAX_STREAMS).filter(|&n| live(&core, n).is_some()) {
                let _ = writeln!(list, "{n}");
            }
        }
        Some(Node::Dir(n)) if live(&core, n).is_some() => list.0.extend_from_slice(b"ctl\ndata\n"),
        _ => return -1,
    }
    if list.0.len() > cap {
        return -1;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(list.0.as_ptr(), buf, list.0.len());
        *out_len = list.0.len();
    }
    0
}

/// `data`: one holder at a time.
unsafe extern "C" fn sound_open(path: *const u8, len: usize, _out: *mut u8, _out_cap: usize) -> i32 {
    let mut core = CORE.lock();
    match node(path, len) {
        Some(Node::File(n, File::Data)) => {
            if live(&core, n).is_none_or(|s| s.held) {
                return -1;
            }
            core.streams[n].as_mut().unwrap().held = true;
            0
        }
        Some(Node::File(n, _)) | Some(Node::Dir(n)) if live(&core, n).is_none() => -1,
        _ => 0,
    }
}

/// The holder's last close stops the stream.
unsafe extern "C" fn sound_release(path: *const u8, len: usize) -> i32 {
    if let Some(Node::File(n, File::Data)) = node(path, len) {
        stop(n);
        if let Some(s) = CORE.lock().streams[n].as_mut() {
            s.held = false;
        }
    }
    0
}

unsafe extern "C" fn sound_read(path: *const u8, len: usize, pos: usize, buf: *mut u8, cap: usize) -> i32 {
    let Some(Node::File(n, f)) = node(path, len) else {
        return -1;
    };
    // SAFETY: the kernel hands `cap` bytes at `buf`.
    let out = unsafe { core::slice::from_raw_parts_mut(buf, cap) };
    let mut core = CORE.lock();
    if f == File::Ctl {
        let Some(text) = ctl_text(&core, n) else {
            return -1;
        };
        let k = cap.min(text.len().saturating_sub(pos));
        out[..k].copy_from_slice(&text[pos..pos + k]);
        return k as i32;
    }
    if live(&core, n).is_none() {
        return -1;
    }
    let s = core.streams[n].as_mut().unwrap();
    if s.info.direction != SOUND_CAPTURE {
        return -1;
    }
    let got = s.ring.pop(&mut out[..cap.min(i32::MAX as usize)]);
    if got == 0 {
        return if s.running.is_some() { MYOS_READ_WAIT } else { 0 };
    }
    drop(core);
    CHANGES[n].fetch_add(1, Ordering::Relaxed);
    got as i32
}

unsafe extern "C" fn sound_write(path: *const u8, len: usize, _pos: usize, buf: *const u8, n_bytes: usize) -> i32 {
    let Some(Node::File(n, f)) = node(path, len) else {
        return -1;
    };
    // SAFETY: the kernel hands `n_bytes` bytes at `buf`.
    let data = unsafe { core::slice::from_raw_parts(buf, n_bytes) };
    if f == File::Ctl {
        let cmd = data.trim_ascii();
        let ok = if cmd == b"stop" {
            stop(n);
            0
        } else if let Some(args) = cmd.strip_prefix(b"start ") {
            start(n, args)
        } else {
            -1
        };
        return if ok == 0 { n_bytes as i32 } else { -1 };
    }
    let mut core = CORE.lock();
    if live(&core, n).is_none() {
        return -1;
    }
    let s = core.streams[n].as_mut().unwrap();
    if s.info.direction != SOUND_PLAYBACK || s.running.is_none() {
        return -1;
    }
    // Whole frames only, so the stream never shifts by a sample.
    let p = s.running.unwrap();
    let frame = frame_bytes(p.channels, p.format) as usize;
    let fit = data.len().min(s.ring.free()) / frame * frame;
    let put = s.ring.push(&data[..fit.min(i32::MAX as usize)]);
    drop(core);
    CHANGES[n].fetch_add(1, Ordering::Relaxed);
    put as i32
}

unsafe extern "C" fn sound_poll(path: *const u8, len: usize, changes: *mut u64) -> u32 {
    let Some(Node::File(n, f)) = node(path, len) else {
        return 0;
    };
    if !changes.is_null() {
        unsafe { *changes = CHANGES[n].load(Ordering::Relaxed) };
    }
    let core = CORE.lock();
    let Some(s) = live(&core, n) else {
        return MYOS_POLLHUP | MYOS_POLLERR;
    };
    if f == File::Ctl {
        return MYOS_POLLIN | MYOS_POLLOUT;
    }
    match s.running {
        Some(p) if s.info.direction == SOUND_PLAYBACK => {
            if s.ring.free() >= p.period_bytes as usize { MYOS_POLLOUT } else { 0 }
        }
        Some(_) if s.ring.len != 0 => MYOS_POLLIN,
        Some(_) => 0,
        // Stopped: a read or write fails or ends at once.
        None => MYOS_POLLIN | MYOS_POLLOUT,
    }
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
        lookup: sound_lookup,
        stat: sound_stat,
        listdir: sound_listdir,
        register: None,
        read: Some(sound_read),
        write: Some(sound_write),
        create: None,
        truncate: None,
        mkdir: None,
        rmdir: None,
        unlink: None,
        rename: None,
        symlink: None,
        readlink: None,
        release: Some(sound_release),
        mmap: None,
        poll: Some(sound_poll),
        open: Some(sound_open),
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
    if api.vfs_mount("sound", "dev/sound", &ops) != 0 {
        status_fail(api, "sound: /dev/sound");
        return -3;
    }
    if api.service_register(SOUND_SERVICE, &OPS) != 0 {
        status_fail(api, "sound: service");
        return -3;
    }
    status_ok(api, "sound");
    0
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

// So `cargo build --bin sound` links. The kernel never jumps here.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
