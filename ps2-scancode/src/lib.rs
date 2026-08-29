//! PS/2 keyboard scancode set 1 and set 2 → ASCII translation.
//!
//! The kernel picks one active set at init (set 1 preferred). Never fall back
//! between sets: the same byte means different keys in set 1 vs set 2.

#![no_std]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScancodeSet {
    Set1,
    Set2,
}

/// Stateful decoder for one PS/2 scancode stream.
#[derive(Clone, Debug)]
pub struct Decoder {
    set: ScancodeSet,
    shift: bool,
    extended: bool,
    set2_break: bool,
    pause_skip: u8,
}

impl Decoder {
    pub fn new(set: ScancodeSet) -> Self {
        Self {
            set,
            shift: false,
            extended: false,
            set2_break: false,
            pause_skip: 0,
        }
    }

    pub fn set(&self) -> ScancodeSet {
        self.set
    }

    pub fn shift(&self) -> bool {
        self.shift
    }

    pub fn reset_modifiers(&mut self) {
        self.shift = false;
        self.extended = false;
        self.set2_break = false;
        self.pause_skip = 0;
    }

    pub fn switch_set(&mut self, set: ScancodeSet) {
        self.set = set;
        self.reset_modifiers();
    }

    /// If the host port delivers set-2 break prefixes while we decode set 1, switch.
    pub fn autodetect_set2_break_prefix(&mut self, sc: u8) -> bool {
        if self.set == ScancodeSet::Set1 && sc == 0xF0 {
            self.set = ScancodeSet::Set2;
            self.set2_break = true;
            return true;
        }
        false
    }

    /// Raw set-2 make codes while decoding set 1 (translation off / misconfigured).
    pub fn autodetect_set2_make(&mut self, sc: u8) -> bool {
        if self.set != ScancodeSet::Set1 {
            return false;
        }
        if matches!(sc, 0xE0 | 0xE1 | 0xF0) || sc & 0x80 != 0 {
            return false;
        }
        if set1_to_ascii(sc, self.shift).is_none() && set2_to_ascii(sc, self.shift).is_some() {
            self.set = ScancodeSet::Set2;
            return true;
        }
        false
    }

    /// One raw byte from the 8042 data port (after filtering the aux/mouse bit).
    pub fn feed(&mut self, sc: u8) -> Option<u8> {
        if self.pause_skip > 0 {
            self.pause_skip -= 1;
            return None;
        }
        // Controller / device responses during init (not key events).
        if matches!(sc, 0x00 | 0xEE | 0xFA | 0xFC | 0xFD | 0xFE) {
            return None;
        }
        if self.set == ScancodeSet::Set2 && sc == 0xF0 {
            self.set2_break = true;
            return None;
        }
        if self.set2_break {
            self.set2_break = false;
            if self.set == ScancodeSet::Set2 {
                match sc {
                    0x12 | 0x59 => self.shift = false,
                    _ => {}
                }
            }
            return None;
        }
        if sc == 0xE0 {
            self.extended = true;
            return None;
        }
        if sc == 0xE1 {
            self.pause_skip = 6;
            return None;
        }
        let extended = core::mem::replace(&mut self.extended, false);
        if extended {
            return self.decode_extended(sc);
        }
        if self.set == ScancodeSet::Set1 && sc & 0x80 != 0 {
            let code = sc & 0x7F;
            match code {
                0x2A | 0x36 => self.shift = false,
                _ => {}
            }
            return None;
        }
        match self.set {
            ScancodeSet::Set1 => self.decode_set1_make(sc),
            ScancodeSet::Set2 => self.decode_set2_make(sc),
        }
    }

    fn decode_extended(&mut self, sc: u8) -> Option<u8> {
        if self.set == ScancodeSet::Set1 {
            if sc & 0x80 != 0 {
                let code = sc & 0x7F;
                if matches!(code, 0x2A | 0x36) {
                    self.shift = false;
                }
            } else if matches!(sc, 0x2A | 0x36) {
                self.shift = true;
            } else if sc == 0x53 {
                return Some(0x08);
            }
        }
        None
    }

    fn decode_set1_make(&mut self, sc: u8) -> Option<u8> {
        match sc {
            0x2A | 0x36 => {
                self.shift = true;
                None
            }
            _ => set1_to_ascii(sc, self.shift),
        }
    }

    fn decode_set2_make(&mut self, sc: u8) -> Option<u8> {
        match sc {
            0x12 | 0x59 => {
                self.shift = true;
                None
            }
            _ => set2_to_ascii(sc, self.shift),
        }
    }
}

pub fn set1_to_ascii(sc: u8, shift: bool) -> Option<u8> {
    let pair = match sc {
        0x02 => (b'1', b'!'),
        0x03 => (b'2', b'@'),
        0x04 => (b'3', b'#'),
        0x05 => (b'4', b'$'),
        0x06 => (b'5', b'%'),
        0x07 => (b'6', b'^'),
        0x08 => (b'7', b'&'),
        0x09 => (b'8', b'*'),
        0x0A => (b'9', b'('),
        0x0B => (b'0', b')'),
        0x0C => (b'-', b'_'),
        0x0D => (b'=', b'+'),
        0x10 => (b'q', b'Q'),
        0x11 => (b'w', b'W'),
        0x12 => (b'e', b'E'),
        0x13 => (b'r', b'R'),
        0x14 => (b't', b'T'),
        0x15 => (b'y', b'Y'),
        0x16 => (b'u', b'U'),
        0x17 => (b'i', b'I'),
        0x18 => (b'o', b'O'),
        0x19 => (b'p', b'P'),
        0x1A => (b'[', b'{'),
        0x1B => (b']', b'}'),
        0x1C => return Some(b'\n'),
        0x1D => return None,
        0x1E => (b'a', b'A'),
        0x1F => (b's', b'S'),
        0x20 => (b'd', b'D'),
        0x21 => (b'f', b'F'),
        0x22 => (b'g', b'G'),
        0x23 => (b'h', b'H'),
        0x24 => (b'j', b'J'),
        0x25 => (b'k', b'K'),
        0x26 => (b'l', b'L'),
        0x27 => (b';', b':'),
        0x28 => (b'\'', b'"'),
        0x29 => (b'`', b'~'),
        0x2B => (b'\\', b'|'),
        0x2C => (b'z', b'Z'),
        0x2D => (b'x', b'X'),
        0x2E => (b'c', b'C'),
        0x2F => (b'v', b'V'),
        0x30 => (b'b', b'B'),
        0x31 => (b'n', b'N'),
        0x32 => (b'm', b'M'),
        0x33 => (b',', b'<'),
        0x34 => (b'.', b'>'),
        0x35 => (b'/', b'?'),
        0x37 => return None,
        0x39 => return Some(b' '),
        0x0E => return Some(0x08),
        0x0F => return Some(b'\t'),
        _ => return None,
    };
    Some(if shift { pair.1 } else { pair.0 })
}

pub fn set2_to_ascii(sc: u8, shift: bool) -> Option<u8> {
    let pair = match sc {
        0x16 => (b'1', b'!'),
        0x1E => (b'2', b'@'),
        0x26 => (b'3', b'#'),
        0x25 => (b'4', b'$'),
        0x2E => (b'5', b'%'),
        0x36 => (b'6', b'^'),
        0x3D => (b'7', b'&'),
        0x3E => (b'8', b'*'),
        0x46 => (b'9', b'('),
        0x45 => (b'0', b')'),
        0x15 => (b'q', b'Q'),
        0x1D => (b'w', b'W'),
        0x24 => (b'e', b'E'),
        0x2D => (b'r', b'R'),
        0x2C => (b't', b'T'),
        0x35 => (b'y', b'Y'),
        0x3C => (b'u', b'U'),
        0x43 => (b'i', b'I'),
        0x44 => (b'o', b'O'),
        0x4D => (b'p', b'P'),
        0x1C => (b'a', b'A'),
        0x1B => (b's', b'S'),
        0x23 => (b'd', b'D'),
        0x2B => (b'f', b'F'),
        0x34 => (b'g', b'G'),
        0x33 => (b'h', b'H'),
        0x3B => (b'j', b'J'),
        0x42 => (b'k', b'K'),
        0x4B => (b'l', b'L'),
        0x1A => (b'z', b'Z'),
        0x22 => (b'x', b'X'),
        0x21 => (b'c', b'C'),
        0x2A => (b'v', b'V'),
        0x32 => (b'b', b'B'),
        0x31 => (b'n', b'N'),
        0x3A => (b'm', b'M'),
        0x58 => return Some(b'\n'),
        0x66 => return Some(0x08),
        0x29 => return Some(b' '),
        0x0D => return Some(b'\t'),
        _ => return None,
    };
    Some(if shift { pair.1 } else { pair.0 })
}

/// Decode a byte sequence and append translated bytes to `out`.
pub fn decode_sequence(set: ScancodeSet, bytes: &[u8], out: &mut allocless::Vec) {
    let mut dec = Decoder::new(set);
    for &b in bytes {
        if let Some(ch) = dec.feed(b) {
            out.push(ch);
        }
    }
}

/// Minimal growable buffer for no_std self-test (fixed cap).
pub mod allocless {
    pub struct Vec {
        buf: [u8; 32],
        len: usize,
    }

    impl Vec {
        pub fn new() -> Self {
            Self {
                buf: [0; 32],
                len: 0,
            }
        }

        pub fn push(&mut self, b: u8) {
            if self.len < self.buf.len() {
                self.buf[self.len] = b;
                self.len += 1;
            }
        }

        pub fn as_slice(&self) -> &[u8] {
            &self.buf[..self.len]
        }
    }
}

/// Built-in vectors for kernel boot and host `cargo test`.
pub fn self_test() -> bool {
    regression_set2_ok() && regression_set1_ok() && regression_no_cross_decode()
}

fn regression_set2_ok() -> bool {
    // Set 2 make codes for o, k, Enter (real-hardware default before F0 01).
    let bytes = [0x44, 0x42, 0x58];
    let mut out = allocless::Vec::new();
    decode_sequence(ScancodeSet::Set2, &bytes, &mut out);
    out.as_slice() == b"ok\n"
}

fn regression_set1_ok() -> bool {
    let bytes = [0x18, 0x25, 0x1C];
    let mut out = allocless::Vec::new();
    decode_sequence(ScancodeSet::Set1, &bytes, &mut out);
    out.as_slice() == b"ok\n"
}

fn regression_no_cross_decode() -> bool {
    // 0x44 is 'o' in set 2 but unmapped in set 1 — must not decode when set 1 is active.
    let mut dec = Decoder::new(ScancodeSet::Set1);
    dec.feed(0x44).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(set: ScancodeSet, bytes: &[u8]) -> allocless::Vec {
        let mut out = allocless::Vec::new();
        decode_sequence(set, bytes, &mut out);
        out
    }

    #[test]
    fn set2_ok_enter() {
        assert_eq!(decode_all(ScancodeSet::Set2, &[0x44, 0x42, 0x58]).as_slice(), b"ok\n");
    }

    #[test]
    fn set1_ok_enter() {
        assert_eq!(decode_all(ScancodeSet::Set1, &[0x18, 0x25, 0x1C]).as_slice(), b"ok\n");
    }

    #[test]
    fn set1_does_not_use_set2_table() {
        let mut dec = Decoder::new(ScancodeSet::Set1);
        assert_eq!(dec.feed(0x44), None);
        assert_eq!(dec.feed(0x18), Some(b'o'));
    }

    #[test]
    fn set2_shift_release_prefix() {
        let mut dec = Decoder::new(ScancodeSet::Set2);
        assert_eq!(dec.feed(0x12), None);
        assert!(dec.shift());
        assert_eq!(dec.feed(0xF0), None);
        assert_eq!(dec.feed(0x12), None);
        assert!(!dec.shift());
    }

    #[test]
    fn set1_shift_release_bit7() {
        let mut dec = Decoder::new(ScancodeSet::Set1);
        assert_eq!(dec.feed(0x2A), None);
        assert!(dec.shift());
        assert_eq!(dec.feed(0xAA), None);
        assert!(!dec.shift());
    }

    #[test]
    fn set2_0x12_is_shift_not_e() {
        let mut dec = Decoder::new(ScancodeSet::Set2);
        assert_eq!(dec.feed(0x12), None);
        assert_eq!(dec.feed(0x24), Some(b'E'));
    }

    #[test]
    fn self_test_passes() {
        assert!(self_test());
    }

    #[test]
    fn autodetect_set2_break_prefix() {
        let mut dec = Decoder::new(ScancodeSet::Set1);
        assert!(dec.autodetect_set2_break_prefix(0xF0));
        assert_eq!(dec.set(), ScancodeSet::Set2);
        assert_eq!(dec.feed(0x12), None); // shift break
        assert_eq!(dec.feed(0x44), Some(b'o'));
    }

    #[test]
    fn autodetect_set2_make() {
        let mut dec = Decoder::new(ScancodeSet::Set1);
        assert!(dec.autodetect_set2_make(0x44));
        assert_eq!(dec.set(), ScancodeSet::Set2);
        assert_eq!(dec.feed(0x44), Some(b'o'));
    }

    #[test]
    fn dual_decode_regression() {
        // Old bug: set1 decoder fell through to set2 for 0x44 → spurious 'o'.
        let mut dec = Decoder::new(ScancodeSet::Set1);
        let wrongly = set2_to_ascii(0x44, false);
        assert_eq!(wrongly, Some(b'o'));
        assert_ne!(dec.feed(0x44), wrongly);
    }
}
