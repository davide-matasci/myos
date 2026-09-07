//! Stdin: serial + PS/2 keyboard (when detected), shared ring buffer.
//!
//! Line discipline follows the console termios (`ICANON` / `ECHO` / `ISIG` /
//! `ICRNL`). Default is **canonical** (cooked): printable bytes accumulate in
//! a private edit buffer and are not visible to `read` until newline. Backspace
//! / DEL erase the last edit column (and the console glyph) without ever
//! delivering `0x08` to userspace. That matches oksh's non-`x_init` path, which
//! expects the kernel line discipline to resolve erase before `shf_getse`.
//!
//! When userspace clears `ICANON` via `TCSETS` / `tcsetattr` (vim raw/cbreak),
//! bytes are pushed straight into the readable ring — including ESC — so TUIs
//! get non-canonical input.

use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

use crate::arch;
use crate::console;
use crate::task;

const RING: usize = 256;
const EDIT: usize = 128;

/// Matches libgloss `<termios.h>` `struct termios` layout (56 bytes).
pub const TERMIOS_LEN: usize = 56;

const VINTR: usize = 0;
const VERASE: usize = 2;
const VEOF: usize = 4;
const VTIME: usize = 5;
const VMIN: usize = 6;

const ICRNL: u32 = 0o000400;

const OPOST: u32 = 0o000001;
const ONLCR: u32 = 0o000004;

const ISIG: u32 = 0o000001;
const ICANON: u32 = 0o000002;
const ECHO: u32 = 0o000010;
const ECHOE: u32 = 0o000020;
const ECHOK: u32 = 0o000040;
const IEXTEN: u32 = 0o001000;

const CS8: u32 = 0o000060;
const CREAD: u32 = 0o000200;

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
    const fn cooked() -> Self {
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

    fn as_bytes(&self) -> [u8; TERMIOS_LEN] {
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

    fn from_bytes(buf: &[u8; TERMIOS_LEN]) -> Self {
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

static TERMIOS: Mutex<Termios> = Mutex::new(Termios::cooked());

static BUF: Mutex<[u8; RING]> = Mutex::new([0; RING]);
static HEAD: AtomicUsize = AtomicUsize::new(0);
static TAIL: AtomicUsize = AtomicUsize::new(0);
/// In-progress line (not yet readable). Length == echoed columns since NL.
static EDIT_BUF: Mutex<[u8; EDIT]> = Mutex::new([0; EDIT]);
static EDIT_LEN: AtomicUsize = AtomicUsize::new(0);

pub fn init() {
    HEAD.store(0, Ordering::SeqCst);
    TAIL.store(0, Ordering::SeqCst);
    EDIT_LEN.store(0, Ordering::SeqCst);
    *TERMIOS.lock() = Termios::cooked();
    arch::serial_flush_rx();
    arch::keyboard_init();
}

pub fn termios_get_bytes() -> [u8; TERMIOS_LEN] {
    TERMIOS.lock().as_bytes()
}

pub fn termios_set_bytes(buf: &[u8; TERMIOS_LEN]) {
    let next = Termios::from_bytes(buf);
    let mut t = TERMIOS.lock();
    let was_canon = t.c_lflag & ICANON != 0;
    let now_canon = next.c_lflag & ICANON != 0;
    *t = next;
    drop(t);
    // Entering raw/cbreak: drop any in-progress cooked edit line so ESC and
    // other keys are not stuck behind an unfinished buffer.
    if was_canon && !now_canon {
        EDIT_LEN.store(0, Ordering::SeqCst);
    }
}

fn lflag() -> u32 {
    TERMIOS.lock().c_lflag
}

fn iflag() -> u32 {
    TERMIOS.lock().c_iflag
}

/// Drain UART and keyboard into the ring (call with interrupts enabled).
pub fn poll() {
    while let Some(b) = arch::serial_read_byte() {
        push_byte(b);
    }
    while let Some(b) = arch::keyboard_poll_byte() {
        push_byte(b);
    }
}

fn push_committed(byte: u8) -> bool {
    let h = HEAD.load(Ordering::SeqCst);
    let next = (h + 1) % RING;
    if next == TAIL.load(Ordering::SeqCst) {
        return false;
    }
    BUF.lock()[h] = byte;
    HEAD.store(next, Ordering::SeqCst);
    true
}

fn push_byte(raw: u8) {
    let lflag = lflag();
    let iflag = iflag();
    let mut byte = raw;

    if byte == b'\r' && iflag & ICRNL != 0 {
        byte = b'\n';
    }

    // ^C / VINTR: honor ISIG in both cooked and raw.
    if lflag & ISIG != 0 && byte == 0x03 {
        crate::signal::handle_ctrl_c();
        return;
    }

    if lflag & ICANON == 0 {
        // Raw / cbreak: deliver key bytes immediately (ESC, arrows CSI, …).
        let _ = push_committed(byte);
        if lflag & ECHO != 0 && byte != 0x1b {
            // Avoid echoing ESC (starts CSI); printable/controls only.
            if byte == b'\n' || byte == b'\t' || (0x20..=0x7e).contains(&byte) {
                console::write_byte(byte);
            }
        }
        return;
    }

    // Cooked: keep the prior line-editing discipline.
    if !(byte == b'\n'
        || byte == b'\t'
        || byte == 0x08
        || byte == 127
        || (0x20..=0x7e).contains(&byte))
    {
        return;
    }

    if byte == 127 || byte == 8 {
        // Only erase when this kernel echo line still has typed columns.
        // Otherwise BS would wipe the shell prompt (`$ `) drawn via write(2).
        let len = EDIT_LEN.load(Ordering::SeqCst);
        if len == 0 {
            return;
        }
        EDIT_LEN.store(len - 1, Ordering::SeqCst);
        if lflag & ECHO != 0 {
            console::write_byte(8);
            console::write_byte(b' ');
            console::write_byte(8);
        }
        return;
    }
    if byte == b'\n' {
        let len = EDIT_LEN.load(Ordering::SeqCst);
        {
            let edit = EDIT_BUF.lock();
            for i in 0..len {
                if !push_committed(edit[i]) {
                    break;
                }
            }
        }
        EDIT_LEN.store(0, Ordering::SeqCst);
        let _ = push_committed(b'\n');
        if lflag & ECHO != 0 {
            console::write_byte(b'\n');
        }
        return;
    }
    let len = EDIT_LEN.load(Ordering::SeqCst);
    if len >= EDIT {
        return;
    }
    EDIT_BUF.lock()[len] = byte;
    EDIT_LEN.store(len + 1, Ordering::SeqCst);
    if lflag & ECHO != 0 {
        console::write_byte(byte);
    }
}

fn pop_byte() -> Option<u8> {
    let t = TAIL.load(Ordering::SeqCst);
    if t == HEAD.load(Ordering::SeqCst) {
        return None;
    }
    let b = BUF.lock()[t];
    TAIL.store((t + 1) % RING, Ordering::SeqCst);
    Some(b)
}

/// Read up to `len` bytes. Blocks until at least one byte is available.
///
/// In canonical mode, bytes come from completed lines only. In raw mode,
/// whatever has been pushed (including ESC) is returned immediately.
pub fn read(buf: &mut [u8]) -> usize {
    crate::signal::enter_input_read();
    let mut n = 0;
    while n == 0 {
        // Pending fatal/actionable signal: break so deliver_due can exit.
        if crate::signal::current_should_wake() {
            break;
        }
        poll();
        while n < buf.len() {
            match pop_byte() {
                Some(b) => {
                    buf[n] = b;
                    n += 1;
                }
                None => break,
            }
        }
        if n == 0 {
            if crate::signal::current_should_wake() {
                break;
            }
            task::yield_now();
        }
    }
    crate::signal::leave_input_read();
    // If we woke for a signal with no bytes, still return 0 so the syscall
    // path can run `deliver_due` and terminate with 128+sig.
    n
}

pub fn keyboard_present() -> bool {
    arch::keyboard_present()
}

