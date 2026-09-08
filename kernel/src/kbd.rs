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

use crate::keymap;

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
