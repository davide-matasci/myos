//! A boot keyboard's report (HID 1.11, appendix B.1): the modifiers' bits,
//! a reserved byte, then the usages of up to six keys held (Keyboard/Keypad
//! page, 0x07). What changed from the last report becomes presses and
//! releases, in Linux `KEY_*` codes.

/// The boot report's length.
pub const REPORT_LEN: usize = 8;

/// The modifier bits, from bit 0 up: left Ctrl, Shift, Alt, GUI, right
/// Ctrl, Shift, Alt (AltGr), GUI; as `KEY_*` codes.
const MODIFIERS: [u16; 8] = [29, 42, 56, 125, 97, 54, 100, 126];

/// Usage 1, ErrorRollOver: in every key slot when more keys are held than
/// the report has room for. Such a report says nothing about the keys.
const ROLL_OVER: u8 = 1;

/// The `KEY_*` code of a usage of the Keyboard/Keypad page (HID Usage
/// Tables, chapter 10), or `None` for one the console has no key for.
pub fn key_code(usage: u8) -> Option<u16> {
    // a to z, in the alphabet's order.
    const LETTERS: [u16; 26] =
        [30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44];
    // 0x28 Enter up to 0x38 the slash: Enter, Escape, Backspace, Tab,
    // Space, - = [ ] \, the non-US # (the ISO key left of Enter), ; ' ` , . /
    const PUNCT: [u16; 17] = [28, 1, 14, 15, 57, 12, 13, 26, 27, 43, 43, 39, 40, 41, 51, 52, 53];
    // 0x39 Caps Lock, then F1 to F12.
    const FKEYS: [u16; 13] = [58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88];
    // 0x46 Print Screen up to 0x63 the keypad's dot: Print Screen, Scroll
    // Lock, Pause, Insert, Home, Page Up, Delete, End, Page Down, Right,
    // Left, Down, Up, Num Lock, the keypad's / * - + Enter 1 to 9, 0 and .
    const NAV: [u16; 30] = [
        99, 70, 119, 110, 102, 104, 111, 107, 109, 106, 105, 108, 103, 69, 98, 55, 74, 78, 96, 79, 80, 81, 75, 76, 77,
        71, 72, 73, 82, 83,
    ];
    let u = usize::from(usage);
    match usage {
        0x04..=0x1D => Some(LETTERS[u - 0x04]),
        // 1 to 9, then 0.
        0x1E..=0x27 => Some(2 + (usage - 0x1E) as u16),
        0x28..=0x38 => Some(PUNCT[u - 0x28]),
        0x39..=0x45 => Some(FKEYS[u - 0x39]),
        0x46..=0x63 => Some(NAV[u - 0x46]),
        // The non-US \ (the ISO key right of left Shift), Application.
        0x64 => Some(86),
        0x65 => Some(127),
        _ => None,
    }
}

/// The changes from report `old` to report `new`, each as `(code,
/// pressed)`: the releases first, then the presses.
pub fn changes(old: &[u8; REPORT_LEN], new: &[u8; REPORT_LEN], mut out: impl FnMut(u16, bool)) {
    let (old_keys, new_keys) = (&old[2..], &new[2..]);
    let held = |keys: &[u8], u: u8| u > ROLL_OVER && keys.contains(&u);
    for (bit, code) in MODIFIERS.iter().enumerate() {
        if old[0] & !new[0] & (1 << bit) != 0 {
            out(*code, false);
        }
    }
    for &u in old_keys {
        if held(old_keys, u) && !held(new_keys, u) {
            if let Some(code) = key_code(u) {
                out(code, false);
            }
        }
    }
    for (bit, code) in MODIFIERS.iter().enumerate() {
        if new[0] & !old[0] & (1 << bit) != 0 {
            out(*code, true);
        }
    }
    for &u in new_keys {
        if held(new_keys, u) && !held(old_keys, u) {
            if let Some(code) = key_code(u) {
                out(code, true);
            }
        }
    }
}

/// A report with more keys held than it can name (ErrorRollOver in its
/// key slots): the keys it names are not the ones held.
pub fn rolled_over(r: &[u8; REPORT_LEN]) -> bool {
    r[2..].iter().all(|&u| u == ROLL_OVER)
}
