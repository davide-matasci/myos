//! Loadable keyboard keymap (keycode → character).
//!
//! Empty at boot: PS/2 / virtio key events produce no console ASCII until
//! userspace loads a map via [`KDSKMAP`] on `/dev/console`. Serial stdin is
//! unaffected.
//!
//! v1 levels are single `u8` bytes (ASCII or Latin-1). UTF-8 multi-byte
//! characters are out of scope — see `docs/keymap.md`.

use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

/// `ioctl(console, KDSKMAP, &KeymapIoctl)` — load map text from userspace.
pub const KDSKMAP: usize = 0x5480;
/// `ioctl(console, KDGKMAP, &mut u32)` — write 1 if a map is loaded, else 0.
pub const KDGKMAP: usize = 0x5481;

/// Max keycode slot (set-1 make codes + Delete/ISO extras fit in 0..127).
pub const NKEYS: usize = 128;
/// base / Shift / AltGr / Shift+AltGr
pub const NLEVELS: usize = 4;

/// Zero means "no character for this level".
type Slot = [u8; NLEVELS];

static LOADED: AtomicBool = AtomicBool::new(false);
static MAP: Mutex<[Slot; NKEYS]> = Mutex::new([[0; NLEVELS]; NKEYS]);

pub fn is_loaded() -> bool {
    LOADED.load(Ordering::SeqCst)
}

pub fn clear() {
    *MAP.lock() = [[0; NLEVELS]; NKEYS];
    LOADED.store(false, Ordering::SeqCst);
}

/// Translate a keycode with current modifiers. `None` if no map or empty slot.
pub fn translate(keycode: u8, shift: bool, altgr: bool) -> Option<u8> {
    if !LOADED.load(Ordering::SeqCst) {
        return None;
    }
    let kc = keycode as usize;
    if kc >= NKEYS {
        return None;
    }
    let level = match (altgr, shift) {
        (false, false) => 0,
        (false, true) => 1,
        (true, false) => 2,
        (true, true) => 3,
    };
    let ch = MAP.lock()[kc][level];
    if ch == 0 {
        None
    } else {
        Some(ch)
    }
}

/// Install a map from the text format documented in `docs/keymap.md`.
pub fn load_from_text(text: &[u8]) -> Result<(), &'static str> {
    let mut map = [[0u8; NLEVELS]; NKEYS];
    let mut any = false;
    for line in text.split(|&b| b == b'\n') {
        let line = trim_ascii(line);
        if line.is_empty() || line[0] == b'#' {
            continue;
        }
        parse_line(line, &mut map)?;
        any = true;
    }
    if !any {
        return Err("empty keymap");
    }
    *MAP.lock() = map;
    LOADED.store(true, Ordering::SeqCst);
    Ok(())
}

fn trim_ascii(s: &[u8]) -> &[u8] {
    let mut a = 0;
    let mut b = s.len();
    while a < b && s[a].is_ascii_whitespace() {
        a += 1;
    }
    while b > a && s[b - 1].is_ascii_whitespace() {
        b -= 1;
    }
    &s[a..b]
}

fn parse_line(line: &[u8], map: &mut [Slot; NKEYS]) -> Result<(), &'static str> {
    // keycode <n> = <b> <s> <a> <sa>
    let rest = strip_prefix(line, b"keycode").ok_or("expected keycode")?;
    let rest = trim_ascii(rest);
    let (num, rest) = split_token(rest).ok_or("missing keycode number")?;
    let kc = parse_keycode(num)?;
    let rest = trim_ascii(rest);
    let rest = strip_prefix(rest, b"=").ok_or("expected =")?;
    let rest = trim_ascii(rest);
    let mut levels = [0u8; NLEVELS];
    let mut left = rest;
    for i in 0..NLEVELS {
        let (tok, next) = split_token(left).ok_or("need four level tokens")?;
        levels[i] = parse_binding(tok)?;
        left = trim_ascii(next);
    }
    if !left.is_empty() {
        return Err("trailing junk on keycode line");
    }
    if kc >= NKEYS {
        return Err("keycode out of range");
    }
    map[kc] = levels;
    Ok(())
}

fn strip_prefix<'a>(s: &'a [u8], p: &[u8]) -> Option<&'a [u8]> {
    if s.len() >= p.len() && &s[..p.len()] == p {
        Some(&s[p.len()..])
    } else {
        None
    }
}

fn split_token(s: &[u8]) -> Option<(&[u8], &[u8])> {
    if s.is_empty() {
        return None;
    }
    let mut i = 0;
    while i < s.len() && !s[i].is_ascii_whitespace() {
        i += 1;
    }
    Some((&s[..i], &s[i..]))
}

fn parse_keycode(tok: &[u8]) -> Result<usize, &'static str> {
    if let Some(hex) = strip_prefix(tok, b"0x").or_else(|| strip_prefix(tok, b"0X")) {
        return parse_hex_usize(hex);
    }
    parse_dec_usize(tok)
}

fn parse_dec_usize(tok: &[u8]) -> Result<usize, &'static str> {
    if tok.is_empty() {
        return Err("bad decimal");
    }
    let mut v = 0usize;
    for &b in tok {
        if !b.is_ascii_digit() {
            return Err("bad decimal");
        }
        v = v
            .checked_mul(10)
            .and_then(|v| v.checked_add((b - b'0') as usize))
            .ok_or("overflow")?;
    }
    Ok(v)
}

fn parse_hex_usize(tok: &[u8]) -> Result<usize, &'static str> {
    if tok.is_empty() {
        return Err("bad hex");
    }
    let mut v = 0usize;
    for &b in tok {
        let d = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return Err("bad hex"),
        };
        v = v
            .checked_mul(16)
            .and_then(|v| v.checked_add(d as usize))
            .ok_or("overflow")?;
    }
    Ok(v)
}

fn parse_binding(tok: &[u8]) -> Result<u8, &'static str> {
    match tok {
        b"none" => Ok(0),
        b"\\n" | b"ENTER" | b"enter" => Ok(b'\n'),
        b"\\t" | b"TAB" | b"tab" => Ok(b'\t'),
        b"\\b" | b"BKSP" | b"bksp" | b"BS" => Ok(0x08),
        b"SPACE" | b"space" => Ok(b' '),
        _ => {
            if let Some(hex) = strip_prefix(tok, b"\\x").or_else(|| strip_prefix(tok, b"0x")) {
                let v = parse_hex_usize(hex)?;
                if v > 0xff {
                    return Err("byte out of range");
                }
                return Ok(v as u8);
            }
            // Single printable ASCII / Latin-1 as one UTF-8 byte (ASCII) or
            // `\xNN` for high bytes. Multi-byte UTF-8 is rejected.
            if tok.len() == 1 {
                return Ok(tok[0]);
            }
            Err("bad binding token")
        }
    }
}

#[cfg(test)]
mod tests {
    // Host tests live in ps2-scancode; keymap parse is exercised via load in
    // kernel boot path and documented examples.
}
