//! Shared tty core: termios + input line discipline, usable by any tty.
//!
//! The console ([`crate::input`]) and pty slave inputs ([`crate::pty`]) run
//! the same POSIX-shaped line discipline over their own state instance:
//!
//! - canonical mode: printable/edit bytes accumulate in the edit buffer and
//!   are committed to the readable ring on newline; BS/DEL erase (never
//!   delivered); every consumed byte goes to the caller's echo sink.
//! - raw/cbreak (`ICANON` clear via TCSETS): bytes commit immediately.
//! - `ISIG` + ^C consumes the byte, clears any partial edit line (POSIX-ish
//!   NOFLSH), and reports VINTR to the caller (each tty maps it to its own
//!   signal target: console fg group, pty session group).
//!
//! One instance per tty end keeps per-session termios independent — the
//! property ptys need (a shell's raw/cooked state never touches another
//! session or the console).

/// Matches libgloss `<termios.h>` `struct termios` layout (56 bytes).
pub const TERMIOS_LEN: usize = 56;

pub const VINTR: usize = 0;
pub const VERASE: usize = 2;
pub const VEOF: usize = 4;
pub const VTIME: usize = 5;
pub const VMIN: usize = 6;

pub const ICRNL: u32 = 0o000400;

pub const OPOST: u32 = 0o000001;
pub const ONLCR: u32 = 0o000004;

pub const ISIG: u32 = 0o000001;
pub const ICANON: u32 = 0o000002;
pub const ECHO: u32 = 0o000010;
pub const ECHOE: u32 = 0o000020;
pub const ECHOK: u32 = 0o000040;
pub const IEXTEN: u32 = 0o001000;

pub const CS8: u32 = 0o000060;
pub const CREAD: u32 = 0o000200;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_cc: [u8; 32],
    pub c_ispeed: u32,
    pub c_ospeed: u32,
}

impl Termios {
    pub const fn cooked() -> Self {
        let mut cc = [0u8; 32];
        cc[VINTR] = 0x03; // ^C
        cc[VERASE] = 0x7f; // DEL
        cc[VEOF] = 0x04; // ^D
        cc[VMIN] = 1;
        cc[VTIME] = 0;
        Self {
            c_iflag: ICRNL,
            c_oflag: OPOST | ONLCR,
            c_cflag: CS8 | CREAD,
            c_lflag: ISIG | ICANON | ECHO | ECHOE | ECHOK | IEXTEN,
            c_cc: cc,
            c_ispeed: 0,
            c_ospeed: 0,
        }
    }

    pub fn as_bytes(&self) -> [u8; TERMIOS_LEN] {
        let mut buf = [0u8; TERMIOS_LEN];
        buf[0..4].copy_from_slice(&self.c_iflag.to_ne_bytes());
        buf[4..8].copy_from_slice(&self.c_oflag.to_ne_bytes());
        buf[8..12].copy_from_slice(&self.c_cflag.to_ne_bytes());
        buf[12..16].copy_from_slice(&self.c_lflag.to_ne_bytes());
        buf[16..48].copy_from_slice(&self.c_cc);
        buf[48..52].copy_from_slice(&self.c_ispeed.to_ne_bytes());
        buf[52..56].copy_from_slice(&self.c_ospeed.to_ne_bytes());
        buf
    }

    pub fn from_bytes(buf: &[u8; TERMIOS_LEN]) -> Self {
        let u32_at = |off: usize| {
            let mut b = [0u8; 4];
            b.copy_from_slice(&buf[off..off + 4]);
            u32::from_ne_bytes(b)
        };
        let mut cc = [0u8; 32];
        cc.copy_from_slice(&buf[16..48]);
        Self {
            c_iflag: u32_at(0),
            c_oflag: u32_at(4),
            c_cflag: u32_at(8),
            c_lflag: u32_at(12),
            c_cc: cc,
            c_ispeed: u32_at(48),
            c_ospeed: u32_at(52),
        }
    }
}

/// Input ring size (committed, post-discipline bytes).
pub const RING: usize = 256;
/// In-progress cooked line capacity.
pub const EDIT: usize = 128;

/// Input line-discipline state for one tty end.
pub struct TtyIn {
    pub termios: Termios,
    pub buf: [u8; RING],
    pub head: usize,
    pub tail: usize,
    /// In-progress line (not yet readable). Length == echoed columns since NL.
    pub edit: [u8; EDIT],
    pub edit_len: usize,
    /// Canonical `^D` (VEOF with an empty edit line) consumed: the next `pop`
    /// reports end-of-file (empty read) and clears the flag.
    pub eof: bool,
}

impl TtyIn {
    pub const fn new() -> Self {
        Self {
            termios: Termios::cooked(),
            buf: [0; RING],
            head: 0,
            tail: 0,
            edit: [0; EDIT],
            edit_len: 0,
            eof: false,
        }
    }

    /// Apply TCSETS. Entering raw/cbreak drops any in-progress cooked edit line
    /// so ESC and other keys are not stuck behind an unfinished buffer.
    pub fn set_termios(&mut self, next: Termios) {
        let was_canon = self.termios.c_lflag & ICANON != 0;
        let now_canon = next.c_lflag & ICANON != 0;
        self.termios = next;
        if was_canon && !now_canon {
            self.edit_len = 0;
        }
    }

    fn push_committed(&mut self, byte: u8) {
        let next = (self.head + 1) % RING;
        if next == self.tail {
            return; // ring full: drop oldest input discipline overflow
        }
        self.buf[self.head] = byte;
        self.head = next;
    }

    /// Push one raw input byte through the discipline.
    ///
    /// `echo` receives each byte that the discipline shows (per `ECHO` and the
    /// ESC rule). Returns `true` when the byte was consumed as VINTR (^C with
    /// `ISIG`) — the caller must then raise SIGINT for this tty's foreground
    /// process(es).
    pub fn push_raw(&mut self, raw: u8, echo: &mut dyn FnMut(u8)) -> bool {
        let lflag = self.termios.c_lflag;
        let iflag = self.termios.c_iflag;
        let mut byte = raw;

        if byte == b'\r' && iflag & ICRNL != 0 {
            byte = b'\n';
        }

        // Canonical ^D / VEOF: with an empty edit line, mark EOF for the
        // next read (POSIX: ^D flushes the pending line, or delivers EOF when
        // there is nothing pending). Echoes nothing.
        if lflag & ICANON != 0 && byte == self.termios.c_cc[VEOF] && byte != 0 {
            if self.edit_len == 0 {
                self.eof = true;
            } else {
                // Commit the pending line without a newline (partial read).
                let len = self.edit_len;
                for i in 0..len {
                    self.push_committed(self.edit[i]);
                }
                self.edit_len = 0;
            }
            return false;
        }

        // ^C / VINTR: honor ISIG in both cooked and raw (takes priority over
        // ESC handling so a ^C during a partial sequence still kills the
        // foreground). Discard any in-progress cooked edit line (POSIX-ish
        // NOFLSH clear) so a partial line cannot leak into the next reader.
        if lflag & ISIG != 0 && byte == 0x03 {
            self.edit_len = 0;
            return true;
        }

        // No CSI/escape special-casing: canonical mode follows termios
        // semantics — every byte that is not an editing character goes into
        // the edit line and is delivered to the reader on newline. Apps that
        // want cursor keys set raw mode (oksh x_mode, vim), which delivers
        // the real ESC [ A/B/C/D bytes.

        if lflag & ICANON == 0 {
            // Raw / cbreak: deliver key bytes immediately (ESC, arrows CSI, …).
            self.push_committed(byte);
            if lflag & ECHO != 0 && byte != 0x1b {
                // Avoid echoing ESC (starts CSI); printable/controls only.
                if byte == b'\n' || byte == b'\t' || (0x20..=0x7e).contains(&byte) {
                    echo(byte);
                }
            }
            return false;
        }

        // Cooked: keep the line-editing discipline.
        if !(byte == b'\n'
            || byte == b'\t'
            || byte == 0x08
            || byte == 127
            || (0x20..=0x7e).contains(&byte))
        {
            return false;
        }

        if byte == 127 || byte == 8 {
            // Only erase when this echo line still has typed columns.
            // Otherwise BS would wipe the shell prompt (`$ `) drawn via write(2).
            if self.edit_len == 0 {
                return false;
            }
            self.edit_len -= 1;
            if lflag & ECHO != 0 {
                echo(8);
                echo(b' ');
                echo(8);
            }
            return false;
        }
        if byte == b'\n' {
            let len = self.edit_len;
            for i in 0..len {
                self.push_committed(self.edit[i]);
            }
            self.edit_len = 0;
            self.push_committed(b'\n');
            if lflag & ECHO != 0 {
                echo(b'\n');
            }
            return false;
        }
        if self.edit_len >= EDIT {
            return false;
        }
        self.edit[self.edit_len] = byte;
        self.edit_len += 1;
        if lflag & ECHO != 0 {
            echo(byte);
        }
        false
    }

    /// Pop one committed byte, if any.
    pub fn pop(&mut self) -> Option<u8> {
        if self.tail == self.head {
            return None;
        }
        let b = self.buf[self.tail];
        self.tail = (self.tail + 1) % RING;
        Some(b)
    }

    /// Consume a pending canonical EOF (^D). Call when the committed ring
    /// is empty: `true` means the read must report end-of-file (0 bytes).
    pub fn take_eof(&mut self) -> bool {
        core::mem::replace(&mut self.eof, false)
    }

    /// Committed bytes available for immediate read.
    pub fn available(&self) -> usize {
        (self.head + RING - self.tail) % RING
    }
}
