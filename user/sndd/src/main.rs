//! `sndd`: the sound server (`docs/sound.md`). It owns the PCM streams
//! of `/dev/sound` (the sound core's, one per card stream) and serves
//! programs over the unix socket `sndd` (`/net/unix`,
//! `docs/sockets-unix.md`). A connection starts with one line:
//!
//! - `play RATE CHANNELS FORMAT [STREAM]`: the program then writes PCM
//!   (interleaved frames, `u8` or `s16` little endian) until it closes. Every
//!   program playing on a stream (the first playback one without
//!   `STREAM`) is mixed into it: each converted to the stream's rate,
//!   channels and format.
//! - `record RATE CHANNELS FORMAT [STREAM]`: the server writes what the
//!   capture stream (the first one without `STREAM`) records, converted,
//!   until the program closes; what the program does not read in time is
//!   dropped.
//! - `devices`: one line per stream, `N playback|capture CARD`.
//!
//! The answer is `ok` or `error WHY`, a line. A stream runs while a
//! program uses it (at 48 kHz stereo `s16` where it can), and stops half a
//! second after the last one went.

#![no_std]
#![no_main]

extern crate alloc;

mod unix;

use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use myos_user::{
    close, exit_code, heap_init, listdir, monotonic_ns, open_flags, poll, read, write, write_fd, Heap, PollFd,
    LISTDIR_BUF, O_RDWR, POLLHUP, POLLIN, POLLOUT,
};
use unix::Conv;

#[global_allocator]
static GLOBAL: Heap = Heap;

myos_user::x86_start!(main);

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: usize, argv: *const usize) -> ! {
    unsafe { myos_user::args::init_from_regs(argc, argv) };
    main()
}

/// The socket's name.
const NAME: &[u8] = b"sndd";
/// What a playing program may have buffered here before the server stops
/// reading it (its writes then wait in the socket).
const CLIENT_BUF: usize = 64 * 1024;
/// How long a stream nobody uses keeps running (ns).
const LINGER_NS: u64 = 500_000_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    U8,
    S16,
}

impl Format {
    fn parse(w: &[u8]) -> Option<Format> {
        match w {
            b"u8" => Some(Format::U8),
            b"s16" => Some(Format::S16),
            _ => None,
        }
    }

    fn name(self) -> &'static [u8] {
        match self {
            Format::U8 => b"u8",
            Format::S16 => b"s16",
        }
    }

    fn bytes(self) -> usize {
        match self {
            Format::U8 => 1,
            Format::S16 => 2,
        }
    }
}

/// How PCM is laid out: frames per second, channels, sample format.
#[derive(Clone, Copy)]
struct Shape {
    rate: u32,
    channels: usize,
    format: Format,
}

impl Shape {
    fn frame(&self) -> usize {
        self.channels * self.format.bytes()
    }

    /// Sample `c` of the frame at `bytes`, as a 16-bit value.
    fn sample(&self, bytes: &[u8], c: usize) -> i32 {
        let c = c.min(self.channels - 1);
        match self.format {
            Format::U8 => (i32::from(bytes[c]) - 128) << 8,
            Format::S16 => i32::from(i16::from_le_bytes([bytes[2 * c], bytes[2 * c + 1]])),
        }
    }

    fn put(&self, out: &mut Vec<u8>, v: i32) {
        let v = v.clamp(-32768, 32767);
        match self.format {
            Format::U8 => out.push(((v >> 8) + 128) as u8),
            Format::S16 => out.extend_from_slice(&(v as i16).to_le_bytes()),
        }
    }
}

/// The frame `frame` (as `from` lays it out) as `to_channels` 16-bit
/// samples: a mono frame on every channel, more channels than wanted
/// averaged into the ones there are.
fn convert_frame(from: &Shape, frame: &[u8], to_channels: usize, out: &mut [i32]) {
    if from.channels == to_channels {
        for (c, o) in out.iter_mut().enumerate().take(to_channels) {
            *o = from.sample(frame, c);
        }
    } else if from.channels == 1 {
        let v = from.sample(frame, 0);
        out[..to_channels].fill(v);
    } else if to_channels == 1 {
        let sum: i32 = (0..from.channels).map(|c| from.sample(frame, c)).sum();
        out[0] = sum / from.channels as i32;
    } else {
        for (c, o) in out.iter_mut().enumerate().take(to_channels) {
            *o = from.sample(frame, c);
        }
    }
}

/// Rate conversion: where in the source the next output frame falls, in
/// 1/2^32 of a frame, and how far each output frame moves it.
struct Resample {
    phase: u64,
    step: u64,
}

impl Resample {
    fn new(from: u32, to: u32) -> Resample {
        Resample { phase: 0, step: (u64::from(from) << 32) / u64::from(to.max(1)) }
    }
}

/// A program playing: what it sent, not mixed yet.
struct Player {
    conv: Conv,
    shape: Shape,
    pending: VecDeque<u8>,
    resample: Resample,
    /// The program closed its end: it goes once `pending` is played.
    done: bool,
}

/// A program recording.
struct Recorder {
    conv: Conv,
    shape: Shape,
    resample: Resample,
    gone: bool,
}

/// One of `/dev/sound`'s streams, while it runs.
struct Open {
    data: usize,
    shape: Shape,
    period: usize,
    /// When the last program left (the stream keeps running a moment).
    idle_since: Option<u64>,
}

struct Stream {
    n: u32,
    capture: bool,
    card: Vec<u8>,
    /// What the stream can do: its `ctl`'s lines.
    channels: (usize, usize),
    formats: Vec<Format>,
    rates: Vec<u32>,
    open: Option<Open>,
    players: Vec<Player>,
    recorders: Vec<Recorder>,
}

fn sound_path(n: u32, file: &[u8]) -> Vec<u8> {
    let mut p = Vec::from(b"/dev/sound/" as &[u8]);
    push_number(&mut p, u64::from(n));
    p.push(b'/');
    p.extend_from_slice(file);
    p
}

fn push_number(out: &mut Vec<u8>, mut v: u64) {
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
    out.extend_from_slice(&d[i..]);
}

fn parse_u32(w: &[u8]) -> Option<u32> {
    if w.is_empty() || !w.iter().all(u8::is_ascii_digit) {
        return None;
    }
    core::str::from_utf8(w).ok()?.parse().ok()
}

fn slurp(path: &[u8]) -> Option<Vec<u8>> {
    let fd = open_flags(path, O_RDWR)?;
    let mut out = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        let n = read(fd, &mut buf);
        if n == 0 || n == usize::MAX {
            break;
        }
        out.extend_from_slice(&buf[..n]);
    }
    close(fd);
    Some(out)
}

/// The streams `/dev/sound` has now.
fn scan() -> Vec<Stream> {
    let mut names = [0u8; LISTDIR_BUF];
    let k = listdir(b"/dev/sound", &mut names);
    let mut out = Vec::new();
    if k == usize::MAX {
        return out;
    }
    for name in names[..k].split(|&c| c == b'\n' || c == 0) {
        let Some(n) = parse_u32(name) else {
            continue;
        };
        let Some(ctl) = slurp(&sound_path(n, b"ctl")) else {
            continue;
        };
        let mut s = Stream {
            n,
            capture: false,
            card: Vec::new(),
            channels: (1, 1),
            formats: Vec::new(),
            rates: Vec::new(),
            open: None,
            players: Vec::new(),
            recorders: Vec::new(),
        };
        for line in ctl.split(|&c| c == b'\n') {
            let mut words = line.split(|&c| c == b' ');
            let key = words.next().unwrap_or(b"");
            let rest = &line[key.len().min(line.len())..].trim_ascii();
            match key {
                b"card" => s.card = Vec::from(*rest),
                b"direction" => s.capture = *rest == b"capture",
                b"channels" => {
                    let mut lim = rest.split(|&c| c == b'-').filter_map(parse_u32);
                    let lo = lim.next().unwrap_or(1) as usize;
                    s.channels = (lo, lim.next().map_or(lo, |h| h as usize));
                }
                b"formats" => s.formats = words.filter_map(Format::parse).collect(),
                b"rates" => s.rates = words.filter_map(parse_u32).collect(),
                _ => {}
            }
        }
        out.push(s);
    }
    out
}

/// The way stream `s` runs: 48 kHz (else 44.1, else its fastest), stereo
/// where it can, `s16` where it can.
fn device_shape(s: &Stream) -> Option<Shape> {
    let rate = [48000, 44100].into_iter().find(|r| s.rates.contains(r)).or_else(|| s.rates.iter().copied().max())?;
    let format = if s.formats.contains(&Format::S16) { Format::S16 } else { *s.formats.first()? };
    Some(Shape { rate, channels: 2.clamp(s.channels.0, s.channels.1.max(s.channels.0)), format })
}

/// Start stream `s` if it is not running: hold its `data`, `start` it.
fn open_stream(s: &mut Stream) -> bool {
    if let Some(o) = s.open.as_mut() {
        o.idle_since = None;
        return true;
    }
    let Some(shape) = device_shape(s) else {
        return false;
    };
    let Some(data) = open_flags(&sound_path(s.n, b"data"), O_RDWR) else {
        return false;
    };
    let Some(ctl) = open_flags(&sound_path(s.n, b"ctl"), O_RDWR) else {
        close(data);
        return false;
    };
    let mut cmd = Vec::from(b"start " as &[u8]);
    push_number(&mut cmd, u64::from(shape.rate));
    cmd.push(b' ');
    push_number(&mut cmd, shape.channels as u64);
    cmd.push(b' ');
    cmd.extend_from_slice(shape.format.name());
    let started = write_fd(ctl, &cmd) == cmd.len();
    close(ctl);
    if !started {
        close(data);
        return false;
    }
    // The core's periods are 20 ms.
    let period = (shape.rate as usize / 50).max(1) * shape.frame();
    s.open = Some(Open { data, shape, period, idle_since: None });
    true
}

fn close_stream(s: &mut Stream) {
    if let Some(o) = s.open.take() {
        close(o.data);
    }
}

/// One period of the playing programs, mixed, in the stream's shape (or
/// `None` when nobody is playing and nothing is left to play).
fn mix(s: &mut Stream) -> Option<Vec<u8>> {
    let o = s.open.as_ref()?;
    let dev = o.shape;
    let frames = o.period / dev.frame();
    if s.players.is_empty() {
        return None;
    }
    let mut acc = vec![0i32; frames * dev.channels];
    let mut sample = [0i32; 8];
    for p in s.players.iter_mut() {
        let fb = p.shape.frame();
        let mut frame = vec![0u8; fb];
        for f in 0..frames {
            // The source frame this output frame falls on (nearest
            // earlier): drop the ones passed.
            let skip = (p.resample.phase >> 32) as usize;
            if skip > 0 {
                let drop = (skip * fb).min(p.pending.len() / fb * fb);
                p.pending.drain(..drop);
                p.resample.phase -= ((drop / fb) as u64) << 32;
                if drop / fb < skip {
                    break;
                }
            }
            if p.pending.len() < fb {
                break;
            }
            for (i, b) in frame.iter_mut().enumerate() {
                *b = p.pending[i];
            }
            convert_frame(&p.shape, &frame, dev.channels, &mut sample);
            for c in 0..dev.channels {
                acc[f * dev.channels + c] += sample[c];
            }
            p.resample.phase += p.resample.step;
        }
    }
    let mut out = Vec::with_capacity(o.period);
    for v in acc {
        dev.put(&mut out, v);
    }
    Some(out)
}

/// What the stream recorded, to each program recording, in its shape.
fn hand_out(s: &mut Stream, pcm: &[u8]) {
    let Some(o) = s.open.as_ref() else {
        return;
    };
    let dev = o.shape;
    let fb = dev.frame();
    let mut sample = [0i32; 8];
    let frames = pcm.len() / fb;
    for r in s.recorders.iter_mut() {
        // `phase` is where the next frame for the program falls, counted
        // from the start of this chunk.
        let mut out = Vec::new();
        while ((r.resample.phase >> 32) as usize) < frames {
            let i = (r.resample.phase >> 32) as usize;
            convert_frame(&dev, &pcm[i * fb..(i + 1) * fb], r.shape.channels, &mut sample);
            for &v in sample.iter().take(r.shape.channels) {
                r.shape.put(&mut out, v);
            }
            r.resample.phase += r.resample.step;
        }
        r.resample.phase -= (frames as u64) << 32;
        // What the program's socket does not take is dropped.
        if !out.is_empty() && write_fd(r.conv.data, &out) == usize::MAX {
            r.gone = true;
        }
    }
}

fn reply(conv: &Conv, text: &[u8]) {
    conv.write(text);
}

/// A new connection's request, answered; the connection kept when it is a
/// player's or a recorder's.
fn serve(streams: &mut Vec<Stream>, conv: Conv) {
    let mut line = Vec::new();
    let mut buf = [0u8; 128];
    while !line.contains(&b'\n') && line.len() < 256 {
        let n = conv.read(&mut buf, 2000);
        if n == 0 {
            conv.close();
            return;
        }
        line.extend_from_slice(&buf[..n]);
    }
    let end = line.iter().position(|&c| c == b'\n').unwrap_or(line.len());
    let rest: Vec<u8> = line.get(end + 1..).map(Vec::from).unwrap_or_default();
    let words: Vec<&[u8]> = line[..end].trim_ascii().split(|&c| c == b' ').filter(|w| !w.is_empty()).collect();
    match words.first().copied() {
        Some(b"devices") => {
            *streams = rescan(core::mem::take(streams));
            let mut out = Vec::new();
            for s in streams.iter() {
                push_number(&mut out, u64::from(s.n));
                out.extend_from_slice(if s.capture { b" capture " } else { b" playback " });
                out.extend_from_slice(&s.card);
                out.push(b'\n');
            }
            reply(&conv, &out);
            conv.close();
        }
        Some(verb @ (b"play" | b"record")) => {
            let capture = verb == b"record";
            let shape = match (words.get(1).and_then(|w| parse_u32(w)), words.get(2).and_then(|w| parse_u32(w)), words.get(3).and_then(|w| Format::parse(w))) {
                (Some(rate), Some(ch), Some(format)) if (1..=8).contains(&ch) && (1000..=384000).contains(&rate) => {
                    Shape { rate, channels: ch as usize, format }
                }
                _ => {
                    reply(&conv, b"error usage: play|record RATE CHANNELS u8|s16 [STREAM]\n");
                    conv.close();
                    return;
                }
            };
            *streams = rescan(core::mem::take(streams));
            let wanted = words.get(4).and_then(|w| parse_u32(w));
            let found = streams.iter().position(|s| s.capture == capture && wanted.is_none_or(|n| n == s.n));
            let Some(i) = found else {
                reply(&conv, b"error no such stream\n");
                conv.close();
                return;
            };
            let s = &mut streams[i];
            if !open_stream(s) {
                reply(&conv, b"error the stream does not start\n");
                conv.close();
                return;
            }
            reply(&conv, b"ok\n");
            let dev = s.open.as_ref().unwrap().shape;
            if capture {
                s.recorders.push(Recorder { conv, shape, resample: Resample::new(dev.rate, shape.rate), gone: false });
            } else {
                let mut pending = VecDeque::new();
                pending.extend(rest);
                s.players.push(Player { conv, shape, pending, resample: Resample::new(shape.rate, dev.rate), done: false });
            }
        }
        _ => {
            reply(&conv, b"error usage: play|record RATE CHANNELS u8|s16 [STREAM] | devices\n");
            conv.close();
        }
    }
}

/// The streams `/dev/sound` has now, the running ones kept as they are.
fn rescan(old: Vec<Stream>) -> Vec<Stream> {
    let mut fresh = scan();
    let mut kept: Vec<Stream> = Vec::new();
    for s in old {
        if s.open.is_some() || !s.players.is_empty() || !s.recorders.is_empty() {
            fresh.retain(|f| f.n != s.n);
            kept.push(s);
        }
    }
    kept.extend(fresh);
    kept.sort_by_key(|s| s.n);
    kept
}

/// What `fds[at]` reported.
fn revents(fds: &[PollFd], at: usize) -> i16 {
    fds.get(at).map_or(0, |f| f.revents)
}

fn main() -> ! {
    heap_init();
    let mut cmd = Vec::from(b"announce " as &[u8]);
    cmd.extend_from_slice(NAME);
    let Some(listener) = Conv::new(&cmd) else {
        write(b"sndd: cannot announce the socket\n");
        exit_code(1);
    };
    let mut streams = scan();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        // What to wait for: the listener; per running stream its `data`
        // (writable for playback, readable for capture); each player while
        // its buffer has room, each recorder (its close).
        let mut fds = Vec::from([PollFd { fd: listener.data as i32, events: POLLIN, revents: 0 }]);
        for s in streams.iter() {
            if let Some(o) = s.open.as_ref() {
                fds.push(PollFd { fd: o.data as i32, events: if s.capture { POLLIN } else { POLLOUT }, revents: 0 });
            }
            for p in s.players.iter() {
                let room = !p.done && p.pending.len() < CLIENT_BUF;
                fds.push(PollFd { fd: if room { p.conv.data as i32 } else { -1 }, events: POLLIN, revents: 0 });
            }
            for r in s.recorders.iter() {
                fds.push(PollFd { fd: r.conv.data as i32, events: POLLIN, revents: 0 });
            }
        }
        poll(&mut fds, 100);
        if revents(&fds, 0) & POLLIN != 0 {
            while let Some(conv) = listener.accept() {
                serve(&mut streams, conv);
            }
        }
        let mut at = 1;
        let now = monotonic_ns();
        for s in streams.iter_mut() {
            if s.open.is_some() {
                let ready = revents(&fds, at);
                at += 1;
                if s.capture {
                    if ready & POLLIN != 0 {
                        let data = s.open.as_ref().unwrap().data;
                        let n = read(data, &mut buf);
                        if n != usize::MAX && n != 0 {
                            let pcm = buf[..n].to_vec();
                            hand_out(s, &pcm);
                        }
                    }
                } else if ready & POLLOUT != 0 {
                    let data = s.open.as_ref().unwrap().data;
                    if let Some(period) = mix(s) {
                        write_fd(data, &period);
                    }
                }
            }
            for p in s.players.iter_mut() {
                let ready = revents(&fds, at);
                at += 1;
                if ready & (POLLIN | POLLHUP) != 0 {
                    let room = CLIENT_BUF - p.pending.len();
                    let n = read(p.conv.data, &mut buf[..room.min(16 * 1024)]);
                    if n == 0 || n == usize::MAX {
                        p.done = true;
                    } else {
                        p.pending.extend(&buf[..n]);
                    }
                }
            }
            for r in s.recorders.iter_mut() {
                let ready = revents(&fds, at);
                at += 1;
                if ready & (POLLIN | POLLHUP) != 0 {
                    // A recorder sends nothing: anything readable is its close.
                    let n = read(r.conv.data, &mut buf[..64]);
                    if n == 0 || n == usize::MAX {
                        r.gone = true;
                    }
                }
            }
            // The programs that are done go.
            let mut i = 0;
            while i < s.players.len() {
                let p = &s.players[i];
                if p.done && p.pending.len() < p.shape.frame() {
                    s.players.swap_remove(i).conv.close();
                } else {
                    i += 1;
                }
            }
            let mut i = 0;
            while i < s.recorders.len() {
                if s.recorders[i].gone {
                    s.recorders.swap_remove(i).conv.close();
                } else {
                    i += 1;
                }
            }
            // A stream nobody uses stops after a while (playback first plays
            // out what is buffered).
            if let Some(o) = s.open.as_mut() {
                if s.players.is_empty() && s.recorders.is_empty() {
                    let since = *o.idle_since.get_or_insert(now);
                    if now.saturating_sub(since) > LINGER_NS {
                        close_stream(s);
                    }
                } else {
                    o.idle_since = None;
                }
            }
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
