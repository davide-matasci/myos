//! Keycode + modifiers → console output bytes, including multi-byte sequences.
//!
//! Hardware drivers (PS/2, virtio-input) decode events to **keycodes** and
//! modifier state, then hand them to [`translate`]. Most keys map to a single
//! byte via the loadable [`crate::keymap`], but a few need reserved handling:
//!
//! - **Esc** (`0x01`) → `0x1b`, regardless of the keymap. Without this, an
//!   Esc key press would be dropped by the cooked line discipline and never
//!   reach `vim` in raw mode.
//! - **Arrow keys** → vt100 CSI sequences `ESC [ A|B|C|D` (3 bytes). These are
//!   the same bytes a real terminal emits, so raw-mode TUIs get working
//!   cursor keys.
//! - **Ctrl + letter** → the control byte (`ch & 0x1f`, e.g. Ctrl+C → `0x03`),
//!   so the kernel's existing `ISIG` ^C path fires in both cooked and raw mode.
//!
//! The byte stream is pushed through a small [`ByteFifo`] in each driver so
//! `poll_byte()` can return multi-byte sequences one byte at a time; the
//! console [`crate::input`] layer drains it.
//!
//! The drivers also hand every press and release to [`raw_key`], in Linux
//! `KEY_*` numbering: while a program holds `/dev/console/kbd`
//! ([`crate::kbdev`]) those events go to it and the tty gets no keys.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::keymap;
use myos_abi::Lock;

pub const KEY_ESC: u8 = 0x01;
pub const KEY_UP: u8 = 0x48;
pub const KEY_DOWN: u8 = 0x50;
pub const KEY_LEFT: u8 = 0x4B;
pub const KEY_RIGHT: u8 = 0x4D;

/// A 0–3 byte output (CSI arrows are 3 bytes; everything else ≤ 1).
#[derive(Clone, Copy)]
pub struct KeyBytes {
    data: [u8; 3],
    len: usize,
}

impl KeyBytes {
    pub fn as_slice(&self) -> &[u8] {
        &self.data[..self.len]
    }
}

/// A tiny FIFO for feeding multi-byte sequences into a byte-oriented
/// `poll_byte()`. Holds up to `MAX_BYTES`·2 entries.
#[derive(Clone, Copy, Default)]
pub struct ByteFifo {
    buf: [u8; 8],
    head: usize,
    tail: usize,
}

impl ByteFifo {
    pub const fn new() -> Self {
        Self {
            buf: [0; 8],
            head: 0,
            tail: 0,
        }
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            let next = (self.tail + 1) % self.buf.len();
            if next == self.head {
                return; // full; drop (key repeat overrun)
            }
            self.buf[self.tail] = b;
            self.tail = next;
        }
    }

    pub fn pop(&mut self) -> Option<u8> {
        if self.head == self.tail {
            return None;
        }
        let b = self.buf[self.head];
        self.head = (self.head + 1) % self.buf.len();
        Some(b)
    }
}

/// Translate a keycode with modifiers into output bytes.
///
/// Reserved keys (Esc, arrows) never consult the loadable keymap. Letter keys
/// with Ctrl held produce the control byte so `^C`-style sequences reach the
/// line discipline. Returns `None` when the key produces no output (modifier
/// makes/breaks, unbound keys, no keymap loaded for character keys).
pub fn translate(keycode: u8, shift: bool, altgr: bool, ctrl: bool) -> Option<KeyBytes> {
    match keycode {
        KEY_ESC => return Some(one(0x1b)),
        KEY_UP => return Some(csi(b'A')),
        KEY_DOWN => return Some(csi(b'B')),
        KEY_RIGHT => return Some(csi(b'C')),
        KEY_LEFT => return Some(csi(b'D')),
        _ => {}
    }
    let ch = keymap::translate(keycode, shift, altgr)?;
    if ctrl && ch.is_ascii_alphabetic() {
        // Ctrl+letter → control byte (e.g. 'c' → 0x03 → ISIG ^C).
        return Some(one(ch & 0x1f));
    }
    Some(one(ch))
}

fn one(b: u8) -> KeyBytes {
    KeyBytes {
        data: [b, 0, 0],
        len: 1,
    }
}

fn csi(final_: u8) -> KeyBytes {
    KeyBytes {
        data: [0x1b, b'[', final_],
        len: 3,
    }
}

/// Someone holds `/dev/console/kbd`: keys go there, not to the tty.
static GRAB: AtomicBool = AtomicBool::new(false);
/// Modifiers as the raw events left them (for an event's character).
static RAW_SHIFT: AtomicBool = AtomicBool::new(false);
static RAW_ALTGR: AtomicBool = AtomicBool::new(false);

/// One key event for `/dev/console/kbd`: Linux `KEY_*` code, pressed, and
/// the loaded keymap's character for it (0: none).
#[derive(Clone, Copy)]
struct RawEvent {
    code: u16,
    pressed: bool,
    ch: u8,
}

/// Events not read yet. When it is full new events are dropped (a reader
/// that stopped reading loses what it did not take).
struct RawQueue {
    ev: [RawEvent; 128],
    head: usize,
    len: usize,
}

static RAW: Lock<RawQueue> = Lock::new(RawQueue {
    ev: [RawEvent { code: 0, pressed: false, ch: 0 }; 128],
    head: 0,
    len: 0,
});

/// Take the keyboard for `/dev/console/kbd`; false if someone holds it.
pub fn grab() -> bool {
    let taken = GRAB.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok();
    if taken {
        let mut q = RAW.lock();
        q.head = 0;
        q.len = 0;
    }
    taken
}

/// Give the keyboard back to the tty.
pub fn ungrab() {
    GRAB.store(false, Ordering::SeqCst);
}

pub fn grabbed() -> bool {
    GRAB.load(Ordering::SeqCst)
}

/// A key event from a driver (Linux `KEY_*` code). Queued for
/// `/dev/console/kbd` while it is held; the driver keeps it from the tty
/// then ([`grabbed`]).
pub fn raw_key(code: u16, pressed: bool) {
    // Linux codes of the modifiers that choose a keymap level.
    match code {
        42 | 54 => RAW_SHIFT.store(pressed, Ordering::SeqCst),
        100 => RAW_ALTGR.store(pressed, Ordering::SeqCst),
        _ => {}
    }
    if !grabbed() {
        return;
    }
    // Codes 1..=0x58 are also the keymap's (set-1) key positions.
    let ch = if pressed && code < 0x59 {
        keymap::translate(code as u8, RAW_SHIFT.load(Ordering::SeqCst), RAW_ALTGR.load(Ordering::SeqCst))
            .unwrap_or(0)
    } else {
        0
    };
    let mut q = RAW.lock();
    if q.len < q.ev.len() {
        let i = (q.head + q.len) % q.ev.len();
        q.ev[i] = RawEvent { code, pressed, ch };
        q.len += 1;
    }
}

/// Events are waiting for `/dev/console/kbd`.
pub fn raw_pending() -> bool {
    RAW.lock().len > 0
}

/// As many whole event lines as fit in `out` (`d 30 a`, `u 30`); bytes
/// written.
pub fn raw_read(out: &mut [u8]) -> usize {
    let mut q = RAW.lock();
    let mut n = 0;
    while q.len > 0 {
        let mut line = [0u8; 16];
        let len = format_event(q.ev[q.head], &mut line);
        if n + len > out.len() {
            break;
        }
        out[n..n + len].copy_from_slice(&line[..len]);
        n += len;
        q.head = (q.head + 1) % q.ev.len();
        q.len -= 1;
    }
    n
}

/// `d <code>[ <char>]\n` or `u <code>\n`. The character is the keymap's
/// Latin-1 byte as UTF-8, left out for space and control characters.
fn format_event(e: RawEvent, out: &mut [u8; 16]) -> usize {
    out[0] = if e.pressed { b'd' } else { b'u' };
    out[1] = b' ';
    let mut n = 2;
    let mut digits = [0u8; 5];
    let mut i = digits.len();
    let mut v = e.code;
    loop {
        i -= 1;
        digits[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    out[n..n + digits.len() - i].copy_from_slice(&digits[i..]);
    n += digits.len() - i;
    let printable = matches!(e.ch, 0x21..=0x7E | 0xA1..=0xFF);
    if printable {
        out[n] = b' ';
        n += 1;
        if e.ch < 0x80 {
            out[n] = e.ch;
            n += 1;
        } else {
            out[n] = 0xC0 | (e.ch >> 6);
            out[n + 1] = 0x80 | (e.ch & 0x3F);
            n += 2;
        }
    }
    out[n] = b'\n';
    n + 1
}
