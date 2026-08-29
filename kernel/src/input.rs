//! Stdin: serial + PS/2 keyboard (when detected), shared ring buffer.

use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

use crate::arch;
use crate::console;
use crate::task;

const RING: usize = 256;

static BUF: Mutex<[u8; RING]> = Mutex::new([0; RING]);
static HEAD: AtomicUsize = AtomicUsize::new(0);
static TAIL: AtomicUsize = AtomicUsize::new(0);

pub fn init() {
    HEAD.store(0, Ordering::SeqCst);
    TAIL.store(0, Ordering::SeqCst);
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

fn push_byte(raw: u8) {
    let mut byte = raw;
    if byte == b'\r' {
        byte = b'\n';
    }
    if !acceptable_input(byte) {
        return;
    }
    if byte == 127 || byte == 8 {
        // Always deliver backspace to the reader. Bytes already consumed into
        // userspace are undone in read_line(); the ring is often empty here.
        let h = HEAD.load(Ordering::SeqCst);
        let next = (h + 1) % RING;
        if next == TAIL.load(Ordering::SeqCst) {
            return;
        }
        BUF.lock()[h] = 0x08;
        HEAD.store(next, Ordering::SeqCst);
        console::write_byte(8);
        console::write_byte(b' ');
        console::write_byte(8);
        return;
    }
    let h = HEAD.load(Ordering::SeqCst);
    let next = (h + 1) % RING;
    if next == TAIL.load(Ordering::SeqCst) {
        return;
    }
    BUF.lock()[h] = byte;
    HEAD.store(next, Ordering::SeqCst);
    if byte == b'\n' {
        console::write_byte(b'\n');
    } else {
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
pub fn read(buf: &mut [u8]) -> usize {
    let mut n = 0;
    while n == 0 {
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
            task::yield_now();
        }
    }
    n
}

pub fn keyboard_present() -> bool {
    arch::keyboard_present()
}
