//! Stdin: serial + PS/2 keyboard (when detected), shared ring buffer.
//!
//! Interactive input is **canonical** (cooked): printable bytes accumulate in
//! a private edit buffer and are not visible to `read` until newline. Backspace
//! / DEL erase the last edit column (and the console glyph) without ever
//! delivering `0x08` to userspace. That matches oksh's non-`x_init` path, which
//! expects the kernel line discipline to resolve erase before `shf_getse`.

use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

use crate::arch;
use crate::console;
use crate::task;

const RING: usize = 256;
const EDIT: usize = 128;

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
    arch::serial_flush_rx();
    arch::keyboard_init();
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

fn acceptable_input(byte: u8) -> bool {
    byte == b'\n' || byte == b'\t' || byte == 0x08 || byte == 127 || (0x20..=0x7E).contains(&byte)
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
    let mut byte = raw;
    if byte == b'\r' {
        byte = b'\n';
    }
    // ^C must be handled before `acceptable_input` (which drops 0x03).
    if byte == 0x03 {
        crate::signal::handle_ctrl_c();
        return;
    }
    if !acceptable_input(byte) {
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
        console::write_byte(8);
        console::write_byte(b' ');
        console::write_byte(8);
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
        console::write_byte(b'\n');
        return;
    }
    let len = EDIT_LEN.load(Ordering::SeqCst);
    if len >= EDIT {
        return;
    }
    EDIT_BUF.lock()[len] = byte;
    EDIT_LEN.store(len + 1, Ordering::SeqCst);
    console::write_byte(byte);
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
/// Bytes come from completed lines only (canonical / cooked discipline).
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
