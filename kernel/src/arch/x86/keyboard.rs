//! PS/2 keyboard via the 8042 controller (poll, no PIC IRQ).
//!
//! Works on QEMU i8042 and typical PC hardware (including USB keyboards in
//! legacy PS/2 mode). If probe/init fails we stay on serial-only stdin.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::console;

const DATA: u16 = 0x60;
const STATUS: u16 = 0x64;

const ST_OUT_FULL: u8 = 1;
const ST_IN_FULL: u8 = 2;
const ST_AUX: u8 = 0x20; // data in 0x60 is from mouse/aux port

static READY: AtomicBool = AtomicBool::new(false);
static SHIFT: AtomicBool = AtomicBool::new(false);
static EXTENDED: AtomicBool = AtomicBool::new(false);
static PAUSE_SKIP: AtomicUsize = AtomicUsize::new(0);

pub fn init() {
    if probe_and_enable() {
        READY.store(true, Ordering::SeqCst);
        console::write_str("kbd ok\n");
    }
}

pub fn present() -> bool {
    READY.load(Ordering::SeqCst)
}

/// Non-blocking: one translated byte from the keyboard, if any.
pub fn poll_byte() -> Option<u8> {
    if !READY.load(Ordering::SeqCst) {
        return None;
    }
    let status = inb(STATUS);
    if status & ST_OUT_FULL == 0 {
        return None;
    }
    // Ignore auxiliary (mouse) bytes — mis-decoding them floods the console.
    if status & ST_AUX != 0 {
        let _ = inb(DATA);
        return None;
    }
    let sc = inb(DATA);
    decode_scancode(sc)
}

fn decode_scancode(sc: u8) -> Option<u8> {
    let skip = PAUSE_SKIP.load(Ordering::SeqCst);
    if skip > 0 {
        PAUSE_SKIP.store(skip - 1, Ordering::SeqCst);
        return None;
    }
    // Controller / device responses during init (not key break codes).
    if matches!(sc, 0x00 | 0xEE | 0xFA | 0xFC | 0xFD | 0xFE) {
        return None;
    }
    if sc == 0xE0 {
        EXTENDED.store(true, Ordering::SeqCst);
        return None;
    }
    if sc == 0xE1 {
        // Pause/Break: six follow-up bytes after E1.
        PAUSE_SKIP.store(6, Ordering::SeqCst);
        return None;
    }
    let extended = EXTENDED.swap(false, Ordering::SeqCst);
    if extended {
        if sc & 0x80 != 0 {
            let code = sc & 0x7F;
            if matches!(code, 0x2A | 0x36) {
                SHIFT.store(false, Ordering::SeqCst);
            }
        } else if matches!(sc, 0x2A | 0x36) {
            SHIFT.store(true, Ordering::SeqCst);
        }
        return None;
    }
    if sc & 0x80 != 0 {
        let code = sc & 0x7F;
        match code {
            0x2A | 0x36 => SHIFT.store(false, Ordering::SeqCst),
            _ => {}
        }
        return None;
    }
    match sc {
        0x2A | 0x36 => {
            SHIFT.store(true, Ordering::SeqCst);
            None
        }
        _ => scancode_to_ascii(sc, SHIFT.load(Ordering::SeqCst)),
    }
}

fn scancode_to_ascii(sc: u8, shift: bool) -> Option<u8> {
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
        0x1D => return None, // ctrl
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
        0x37 => return None, // keypad *
        0x39 => return Some(b' '),
        0x0E => return Some(0x08), // backspace
        0x0F => return Some(b'\t'),
        _ => return None,
    };
    Some(if shift { pair.1 } else { pair.0 })
}

fn probe_and_enable() -> bool {
    flush_output();
    // Disable keyboard and mouse ports while configuring.
    if !write_cmd(0xAD) || !write_cmd(0xA7) {
        return false;
    }
    flush_output();
    if !write_cmd(0x20) {
        return false;
    }
    let Some(cfg) = read_data() else {
        return false;
    };
    // Enable keyboard clock, disable mouse clock and IRQs (we poll).
    let cfg = (cfg & !0x03 & !0x10) | 0x20;
    if !write_cmd(0x60) || !write_data(cfg) {
        return false;
    }
    if !write_cmd(0xAE) {
        return false;
    }
    // Enable scanning on the keyboard device.
    if !write_data(0xF4) {
        return false;
    }
    if read_data() != Some(0xFA) {
        return false;
    }
    // Slow typematic to a minimum (avoid repeat floods on stuck keys).
    let _ = write_data(0xF3);
    let _ = read_data();
    let _ = write_data(0x00);
    let _ = read_data();
    let _ = write_data(0x00);
    let _ = read_data();
    flush_output();
    SHIFT.store(false, Ordering::SeqCst);
    EXTENDED.store(false, Ordering::SeqCst);
    PAUSE_SKIP.store(0, Ordering::SeqCst);
    true
}

fn flush_output() {
    for _ in 0..32 {
        if inb(STATUS) & ST_OUT_FULL == 0 {
            return;
        }
        let _ = inb(DATA);
    }
}

fn wait_input_clear() -> bool {
    for _ in 0..100_000 {
        if inb(STATUS) & ST_IN_FULL == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

fn write_cmd(val: u8) -> bool {
    if !wait_input_clear() {
        return false;
    }
    outb(STATUS, val);
    true
}

fn write_data(val: u8) -> bool {
    if !wait_input_clear() {
        return false;
    }
    outb(DATA, val);
    true
}

fn read_data() -> Option<u8> {
    for _ in 0..100_000 {
        if inb(STATUS) & ST_OUT_FULL != 0 {
            return Some(inb(DATA));
        }
        core::hint::spin_loop();
    }
    None
}

#[inline]
fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!(
            "in al, dx",
            in("dx") port,
            out("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}

#[inline]
fn outb(port: u16, value: u8) {
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}
