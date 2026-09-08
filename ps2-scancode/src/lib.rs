//! PS/2 keyboard scancode set 1 and set 2 → stable keycodes (not ASCII).
//!
//! The kernel picks one active set at init (set 1 preferred). Never fall back
//! between sets: the same byte means different keys in set 1 vs set 2.
//!
//! Keycodes are **PS/2 set-1 make codes** (and a few extended make codes such
//! as Delete `0x53` or the arrow keys `0x48/0x50/0x4B/0x4D`). Layout →
//! character translation is the kernel's loadable keymap — this crate only
//! tracks modifiers and emits key positions.

#![no_std]

/// Esc (set-1 make `0x01`, set-2 `0x76`).
pub const KEY_ESC: u8 = 0x01;
/// Up arrow — canonical PS/2 set-1 make code.
pub const KEY_UP: u8 = 0x48;
/// Down arrow — canonical PS/2 set-1 make code.
pub const KEY_DOWN: u8 = 0x50;
/// Left arrow — canonical PS/2 set-1 make code.
pub const KEY_LEFT: u8 = 0x4B;
/// Right arrow — canonical PS/2 set-1 make code.
pub const KEY_RIGHT: u8 = 0x4D;

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
    altgr: bool,
    ctrl: bool,
    extended: bool,
    set2_break: bool,
    pause_skip: u8,
}

impl Decoder {
    pub fn new(set: ScancodeSet) -> Self {
        Self {
            set,
            shift: false,
            altgr: false,
            ctrl: false,
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

    pub fn altgr(&self) -> bool {
        self.altgr
    }

    /// True while a Ctrl key (left or right) is held down.
    pub fn ctrl(&self) -> bool {
        self.ctrl
    }

    pub fn reset_modifiers(&mut self) {
        self.shift = false;
        self.altgr = false;
        self.ctrl = false;
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
        // Known set-2 letter make that is unmapped as a set-1 make.
        if set1_make_to_keycode(sc).is_none() && set2_make_to_keycode(sc).is_some() {
            self.set = ScancodeSet::Set2;
            return true;
        }
        false
    }

    /// One raw byte from the 8042 data port (after filtering the aux/mouse bit).
    ///
    /// Returns a **keycode** on key make for keys that may produce characters
    /// (letters, digits, Enter, Tab, Backspace, Space, ISO 102nd, Delete, …).
    /// Modifier makes/breaks update internal state and return `None`.
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
            let extended = core::mem::replace(&mut self.extended, false);
            if self.set == ScancodeSet::Set2 {
                if extended {
                    match sc {
                        0x11 => self.altgr = false,
                        0x14 => self.ctrl = false, // Right Ctrl
                        _ => {}
                    }
                } else {
                    match sc {
                        0x12 | 0x59 => self.shift = false,
                        0x14 => self.ctrl = false, // Left Ctrl
                        _ => {}
                    }
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
                0x1D => self.ctrl = false, // Left Ctrl
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
        match self.set {
            ScancodeSet::Set1 => {
                if sc & 0x80 != 0 {
                    let code = sc & 0x7F;
                    match code {
                        0x2A | 0x36 => self.shift = false,
                        0x38 => self.altgr = false,
                        0x1D => self.ctrl = false, // Right Ctrl (E0 9D)
                        _ => {}
                    }
                    None
                } else {
                    match sc {
                        0x2A | 0x36 => {
                            self.shift = true;
                            None
                        }
                        0x38 => {
                            self.altgr = true;
                            None
                        }
                        0x1D => {
                            self.ctrl = true; // Right Ctrl (E0 1D)
                            None
                        }
                        // Delete → keycode 0x53 (map typically binds to BS).
                        0x53 => Some(0x53),
                        // Arrow keys → canonical set-1 keycodes (vt100 CSI).
                        0x48 => Some(KEY_UP),
                        0x50 => Some(KEY_DOWN),
                        0x4B => Some(KEY_LEFT),
                        0x4D => Some(KEY_RIGHT),
                        _ => None,
                    }
                }
            }
            ScancodeSet::Set2 => {
                match sc {
                    0x11 => {
                        self.altgr = true;
                        None
                    }
                    0x14 => {
                        self.ctrl = true; // Right Ctrl (E0 14)
                        None
                    }
                    // Extended Delete (E0 71).
                    0x71 => Some(0x53),
                    // Arrow keys → canonical set-1 keycodes.
                    0x75 => Some(KEY_UP),
                    0x72 => Some(KEY_DOWN),
                    0x6B => Some(KEY_LEFT),
                    0x74 => Some(KEY_RIGHT),
                    _ => None,
                }
            }
        }
    }

    fn decode_set1_make(&mut self, sc: u8) -> Option<u8> {
        match sc {
            0x2A | 0x36 => {
                self.shift = true;
                None
            }
            // Left Alt — not AltGr; ignore for character path.
            0x38 => None,
            0x1D => {
                self.ctrl = true; // Left Ctrl
                None
            }
            0x3A => None, // Caps
            _ => set1_make_to_keycode(sc),
        }
    }

    fn decode_set2_make(&mut self, sc: u8) -> Option<u8> {
        match sc {
            0x12 | 0x59 => {
                self.shift = true;
                None
            }
            0x11 => None, // Left Alt (non-extended)
            0x14 => {
                self.ctrl = true; // Left Ctrl
                None
            }
            0x58 => None, // Caps
            _ => set2_make_to_keycode(sc),
        }
    }
}

/// Set-1 make → keycode (identity for the keys we care about).
pub fn set1_make_to_keycode(sc: u8) -> Option<u8> {
    match sc {
        0x01..=0x0D => Some(sc), // Esc + number row
        0x0E => Some(0x0E),      // Backspace
        0x0F => Some(0x0F),      // Tab
        0x10..=0x1C => Some(sc), // q–Enter
        0x1E..=0x29 => Some(sc), // a–grave
        0x2B => Some(0x2B),      // \ |
        0x2C..=0x35 => Some(sc), // z–/
        0x37 => None,            // keypad *
        0x39 => Some(0x39),      // Space
        0x56 => Some(0x56),      // ISO 102nd (< > on CH)
        _ => None,
    }
}

/// Set-2 make → set-1-style keycode.
pub fn set2_make_to_keycode(sc: u8) -> Option<u8> {
    Some(match sc {
        0x76 => 0x01, // Esc
        0x16 => 0x02,
        0x1E => 0x03,
        0x26 => 0x04,
        0x25 => 0x05,
        0x2E => 0x06,
        0x36 => 0x07,
        0x3D => 0x08,
        0x3E => 0x09,
        0x46 => 0x0A,
        0x45 => 0x0B,
        0x4E => 0x0C,
        0x55 => 0x0D,
        0x66 => 0x0E, // Backspace
        0x0D => 0x0F, // Tab
        0x15 => 0x10, // q
        0x1D => 0x11,
        0x24 => 0x12,
        0x2D => 0x13,
        0x2C => 0x14,
        0x35 => 0x15,
        0x3C => 0x16,
        0x43 => 0x17,
        0x44 => 0x18,
        0x4D => 0x19,
        0x54 => 0x1A,
        0x5B => 0x1B,
        0x5A => 0x1C, // Enter
        0x1C => 0x1E, // a
        0x1B => 0x1F,
        0x23 => 0x20,
        0x2B => 0x21,
        0x34 => 0x22,
        0x33 => 0x23,
        0x3B => 0x24,
        0x42 => 0x25,
        0x4B => 0x26,
        0x4C => 0x27,
        0x52 => 0x28,
        0x0E => 0x29, // `
        0x5D => 0x2B, // \ |
        0x1A => 0x2C, // z
        0x22 => 0x2D,
        0x21 => 0x2E,
        0x2A => 0x2F,
        0x32 => 0x30,
        0x31 => 0x31,
        0x3A => 0x32,
        0x41 => 0x33,
        0x49 => 0x34,
        0x4A => 0x35,
        0x29 => 0x39, // Space
        0x61 => 0x56, // ISO 102nd
        _ => return None,
    })
}

/// Decode a byte sequence and append keycodes to `out`.
pub fn decode_sequence(set: ScancodeSet, bytes: &[u8], out: &mut allocless::Vec) {
    let mut dec = Decoder::new(set);
    for &b in bytes {
        if let Some(kc) = dec.feed(b) {
            out.push(kc);
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
    regression_set2_ok() && regression_set1_ok() && regression_no_cross_decode() && regression_altgr()
}

fn regression_set2_ok() -> bool {
    // Set 2 make codes for o, k, Enter → keycodes 0x18, 0x25, 0x1C.
    let bytes = [0x44, 0x42, 0x5A];
    let mut out = allocless::Vec::new();
    decode_sequence(ScancodeSet::Set2, &bytes, &mut out);
    out.as_slice() == [0x18, 0x25, 0x1C]
}

fn regression_set1_ok() -> bool {
    let bytes = [0x18, 0x25, 0x1C];
    let mut out = allocless::Vec::new();
    decode_sequence(ScancodeSet::Set1, &bytes, &mut out);
    out.as_slice() == [0x18, 0x25, 0x1C]
}

fn regression_no_cross_decode() -> bool {
    // 0x44 is 'o' make in set 2 but unmapped in set 1.
    let mut dec = Decoder::new(ScancodeSet::Set1);
    dec.feed(0x44).is_none()
}

fn regression_altgr() -> bool {
    let mut dec = Decoder::new(ScancodeSet::Set1);
    if dec.feed(0xE0).is_some() || dec.feed(0x38).is_some() {
        return false;
    }
    if !dec.altgr() {
        return false;
    }
    if dec.feed(0x03) != Some(0x03) {
        return false;
    }
    if dec.feed(0xE0).is_some() || dec.feed(0xB8).is_some() {
        return false;
    }
    !dec.altgr()
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
    fn set2_ok_enter_keycodes() {
        assert_eq!(
            decode_all(ScancodeSet::Set2, &[0x44, 0x42, 0x5A]).as_slice(),
            &[0x18, 0x25, 0x1C]
        );
    }

    #[test]
    fn set1_ok_enter_keycodes() {
        assert_eq!(
            decode_all(ScancodeSet::Set1, &[0x18, 0x25, 0x1C]).as_slice(),
            &[0x18, 0x25, 0x1C]
        );
    }

    #[test]
    fn set1_does_not_use_set2_table() {
        let mut dec = Decoder::new(ScancodeSet::Set1);
        assert_eq!(dec.feed(0x44), None);
        assert_eq!(dec.feed(0x18), Some(0x18));
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
    fn set2_0x12_is_shift_not_keycode() {
        let mut dec = Decoder::new(ScancodeSet::Set2);
        assert_eq!(dec.feed(0x12), None);
        assert!(dec.shift());
        assert_eq!(dec.feed(0x24), Some(0x12)); // e keycode
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
        assert_eq!(dec.feed(0x44), Some(0x18));
    }

    #[test]
    fn autodetect_set2_make() {
        let mut dec = Decoder::new(ScancodeSet::Set1);
        assert!(dec.autodetect_set2_make(0x44));
        assert_eq!(dec.set(), ScancodeSet::Set2);
        assert_eq!(dec.feed(0x44), Some(0x18));
    }

    #[test]
    fn altgr_set1() {
        let mut dec = Decoder::new(ScancodeSet::Set1);
        assert_eq!(dec.feed(0xE0), None);
        assert_eq!(dec.feed(0x38), None);
        assert!(dec.altgr());
        assert_eq!(dec.feed(0xE0), None);
        assert_eq!(dec.feed(0xB8), None);
        assert!(!dec.altgr());
    }

    #[test]
    fn altgr_set2() {
        let mut dec = Decoder::new(ScancodeSet::Set2);
        assert_eq!(dec.feed(0xE0), None);
        assert_eq!(dec.feed(0x11), None);
        assert!(dec.altgr());
        assert_eq!(dec.feed(0xE0), None);
        assert_eq!(dec.feed(0xF0), None);
        assert_eq!(dec.feed(0x11), None);
        assert!(!dec.altgr());
    }

    #[test]
    fn set1_ctrl_tracking() {
        let mut dec = Decoder::new(ScancodeSet::Set1);
        // Left Ctrl make (0x1D) sets ctrl, no keycode.
        assert_eq!(dec.feed(0x1D), None);
        assert!(dec.ctrl());
        // c follows through while ctrl held.
        assert_eq!(dec.feed(0x2E), Some(0x2E));
        assert!(dec.ctrl());
        // Left Ctrl break (0x9D) clears it.
        assert_eq!(dec.feed(0x9D), None);
        assert!(!dec.ctrl());
    }

    #[test]
    fn set1_right_ctrl_tracking() {
        let mut dec = Decoder::new(ScancodeSet::Set1);
        // Right Ctrl extended make (E0 1D) sets ctrl.
        assert_eq!(dec.feed(0xE0), None);
        assert_eq!(dec.feed(0x1D), None);
        assert!(dec.ctrl());
        assert_eq!(dec.feed(0x2E), Some(0x2E));
        // Right Ctrl extended break (E0 9D) clears it.
        assert_eq!(dec.feed(0xE0), None);
        assert_eq!(dec.feed(0x9D), None);
        assert!(!dec.ctrl());
    }

    #[test]
    fn set2_ctrl_tracking() {
        let mut dec = Decoder::new(ScancodeSet::Set2);
        // Left Ctrl set-2 make (0x14).
        assert_eq!(dec.feed(0x14), None);
        assert!(dec.ctrl());
        assert_eq!(dec.feed(0x24), Some(0x12)); // e
        // Break: F0 14.
        assert_eq!(dec.feed(0xF0), None);
        assert_eq!(dec.feed(0x14), None);
        assert!(!dec.ctrl());
    }

    #[test]
    fn set2_right_ctrl_extended_tracking() {
        let mut dec = Decoder::new(ScancodeSet::Set2);
        // Right Ctrl: E0 14.
        assert_eq!(dec.feed(0xE0), None);
        assert_eq!(dec.feed(0x14), None);
        assert!(dec.ctrl());
        // Break: E0 F0 14.
        assert_eq!(dec.feed(0xE0), None);
        assert_eq!(dec.feed(0xF0), None);
        assert_eq!(dec.feed(0x14), None);
        assert!(!dec.ctrl());
    }

    #[test]
    fn set1_arrows_extended() {
        // Set-1 extended arrows: Up/Down/Left/Right = E0 48/50/4B/4D.
        let mut dec = Decoder::new(ScancodeSet::Set1);
        assert_eq!(dec.feed(0xE0), None);
        assert_eq!(dec.feed(0x48), Some(KEY_UP));
        assert_eq!(dec.feed(0xE0), None);
        assert_eq!(dec.feed(0x50), Some(KEY_DOWN));
        assert_eq!(dec.feed(0xE0), None);
        assert_eq!(dec.feed(0x4B), Some(KEY_LEFT));
        assert_eq!(dec.feed(0xE0), None);
        assert_eq!(dec.feed(0x4D), Some(KEY_RIGHT));
    }

    #[test]
    fn set2_arrows_extended() {
        // Set-2 extended arrows: Up/Down/Left/Right = E0 75/72/6B/74.
        let mut dec = Decoder::new(ScancodeSet::Set2);
        assert_eq!(dec.feed(0xE0), None);
        assert_eq!(dec.feed(0x75), Some(KEY_UP));
        assert_eq!(dec.feed(0xE0), None);
        assert_eq!(dec.feed(0x72), Some(KEY_DOWN));
        assert_eq!(dec.feed(0xE0), None);
        assert_eq!(dec.feed(0x6B), Some(KEY_LEFT));
        assert_eq!(dec.feed(0xE0), None);
        assert_eq!(dec.feed(0x74), Some(KEY_RIGHT));
    }

    #[test]
    fn ctrl_c_produces_c_control_byte_via_keymap_path() {
        // Ctrl+C in set-1: ctrl held, then 'c' key (0x2E). The kernel maps the
        // resulting char with ctrl held to 0x03 (see kbd.rs). Here we assert the
        // decoder tracks ctrl through the letter so the kernel sees both.
        let mut dec = Decoder::new(ScancodeSet::Set1);
        assert_eq!(dec.feed(0x1D), None);
        assert!(dec.ctrl());
        assert_eq!(dec.feed(0x2E), Some(0x2E));
    }
}
