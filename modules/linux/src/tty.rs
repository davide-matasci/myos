//! The Linux tty ioctls, served from the terminal's control file.
//!
//! A myos terminal keeps its state as text in its `ctl` file (docs/tty.md);
//! the kernel hands the module that text for an fd (`KernelApi::tty_ctl_read`)
//! and takes it back (`tty_ctl_write`). musl's `struct termios` for the
//! `TCGETS` family is the Linux kernel's: four `u32` flag words, `c_line`,
//! then 19 control characters (36 bytes); `struct winsize` is four `u16`.

use alloc::format;
use alloc::string::String;

use super::abi::*;
use super::sys::R;
use crate::k::task;

pub const TCGETS: usize = 0x5401;
pub const TCSETS: usize = 0x5402;
pub const TCSETSW: usize = 0x5403;
pub const TCSETSF: usize = 0x5404;
pub const TCFLSH: usize = 0x540B;
pub const TIOCSCTTY: usize = 0x540E;
pub const TIOCGWINSZ: usize = 0x5413;
pub const TIOCSWINSZ: usize = 0x5414;
pub const TIOCGPTN: usize = 0x8004_5430;
pub const TIOCSPTLCK: usize = 0x4004_5431;

/// Linux's `struct termios`, as `TCGETS` fills it.
pub const TERMIOS_LEN: usize = 36;
const NCCS: usize = 19;

/// What the ctl text says: the four flag words, the control characters and
/// the window size.
struct State {
    flags: [u32; 4],
    cc: [u8; 32],
    rows: u16,
    cols: u16,
}

fn number(word: &str) -> Option<u32> {
    match word.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => word.parse().ok(),
    }
}

fn parse(text: &[u8]) -> Option<State> {
    let text = core::str::from_utf8(text).ok()?;
    let mut st = State { flags: [0; 4], cc: [0; 32], rows: 0, cols: 0 };
    for line in text.lines() {
        let mut words = line.split_ascii_whitespace();
        let Some(key) = words.next() else { continue };
        match key {
            "iflag" => st.flags[0] = number(words.next()?)?,
            "oflag" => st.flags[1] = number(words.next()?)?,
            "cflag" => st.flags[2] = number(words.next()?)?,
            "lflag" => st.flags[3] = number(words.next()?)?,
            "cc" => {
                for (i, w) in words.take(32).enumerate() {
                    st.cc[i] = u8::from_str_radix(w, 16).ok()?;
                }
            }
            "winsize" => {
                st.rows = u16::try_from(number(words.next()?)?).ok()?;
                st.cols = u16::try_from(number(words.next()?)?).ok()?;
            }
            _ => {}
        }
    }
    Some(st)
}

fn state(fd: usize) -> Result<State, usize> {
    let text = task::tty_ctl_read(fd).ok_or(ENOTTY)?;
    parse(&text).ok_or(EIO)
}

fn write(fd: usize, text: &str) -> R {
    if task::tty_ctl_write(fd, text.as_bytes()) { Ok(0) } else { Err(ENOTTY) }
}

/// `TCGETS`: the Linux termios bytes.
pub fn termios(fd: usize) -> Result<[u8; TERMIOS_LEN], usize> {
    let st = state(fd)?;
    let mut out = [0u8; TERMIOS_LEN];
    for (i, f) in st.flags.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&f.to_ne_bytes());
    }
    // out[16] is c_line, 0.
    out[17..17 + NCCS].copy_from_slice(&st.cc[..NCCS]);
    Ok(out)
}

/// `TCSETS` (and `TCSETSW`, `TCSETSF`: the kernel applies at once): the
/// Linux termios bytes written back as ctl lines. The 19 control characters
/// Linux knows are set; the rest keep their values.
pub fn set_termios(fd: usize, t: &[u8; TERMIOS_LEN]) -> R {
    let word = |i: usize| u32::from_ne_bytes([t[i], t[i + 1], t[i + 2], t[i + 3]]);
    let mut text = format!(
        "iflag 0x{:x}\noflag 0x{:x}\ncflag 0x{:x}\nlflag 0x{:x}\ncc",
        word(0), word(4), word(8), word(12)
    );
    for &c in &t[17..17 + NCCS] {
        text.push_str(&format!(" {c:02x}"));
    }
    text.push('\n');
    write(fd, &text)
}

/// `TIOCGWINSZ`: `struct winsize` bytes.
pub fn winsize(fd: usize) -> Result<[u8; 8], usize> {
    let st = state(fd)?;
    let mut out = [0u8; 8];
    out[0..2].copy_from_slice(&st.rows.to_ne_bytes());
    out[2..4].copy_from_slice(&st.cols.to_ne_bytes());
    Ok(out)
}

pub fn set_winsize(fd: usize, w: &[u8; 8]) -> R {
    let rows = u16::from_ne_bytes([w[0], w[1]]);
    let cols = u16::from_ne_bytes([w[2], w[3]]);
    write(fd, &format!("winsize {rows} {cols}\n"))
}

/// `TCFLSH` with Linux's queue selector.
pub fn flush(fd: usize, queue: usize) -> R {
    let line = match queue {
        0 => "flush in\n",
        1 => "flush out\n",
        2 => "flush both\n",
        _ => return Err(EINVAL),
    };
    write(fd, line)
}

pub fn set_ctty(fd: usize) -> R {
    write(fd, "ctty\n")
}

/// `TIOCGPTN`: the pair's index, from the master's name `/dev/pts/N/master`.
pub fn pty_index(fd: usize) -> Result<u32, usize> {
    let path = task::fd_path(fd).ok_or(ENOTTY)?;
    let path = String::from_utf8(path).map_err(|_| ENOTTY)?;
    let rest = path.strip_prefix("/dev/pts/").ok_or(ENOTTY)?;
    let (index, member) = rest.split_once('/').ok_or(ENOTTY)?;
    if member != "master" {
        return Err(ENOTTY);
    }
    index.parse().map_err(|_| ENOTTY)
}
