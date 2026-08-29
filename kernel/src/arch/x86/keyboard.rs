//! PS/2 keyboard via the 8042 controller (poll, no PIC IRQ).
//!
//! Works on QEMU i8042 and typical PC hardware (including USB keyboards in
//! legacy PS/2 mode). If probe/init fails we stay on serial-only stdin.

use core::sync::atomic::{AtomicBool, Ordering};

use ps2_scancode::{Decoder, ScancodeSet};
use spin::Mutex;

use crate::console;

const DATA: u16 = 0x60;
const STATUS: u16 = 0x64;

const ST_OUT_FULL: u8 = 1;
const ST_IN_FULL: u8 = 2;
const ST_AUX: u8 = 0x20; // data in 0x60 is from mouse/aux port

static READY: AtomicBool = AtomicBool::new(false);
static DECODER: Mutex<Option<Decoder>> = Mutex::new(None);

pub fn init() {
    if let Some((dec, set)) = probe_and_enable() {
        *DECODER.lock() = Some(dec);
        READY.store(true, Ordering::SeqCst);
        console::write_str("kbd ok (set ");
        console::write_str(if set == ScancodeSet::Set1 {
            "1"
        } else {
            "2"
        });
        console::write_str(")\n");
        if ps2_scancode::self_test() {
            console::write_str("kbd decode ok\n");
        } else {
            console::write_str("kbd decode FAIL\n");
        }
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
    if status & ST_AUX != 0 {
        let _ = inb(DATA);
        return None;
    }
    let sc = inb(DATA);
    DECODER.lock().as_mut()?.feed(sc)
}

fn probe_and_enable() -> Option<(Decoder, ScancodeSet)> {
    flush_output();
    if !write_cmd(0xAD) || !write_cmd(0xA7) {
        return None;
    }
    flush_output();
    if !write_cmd(0x20) {
        return None;
    }
    let cfg = read_data()?;
    let cfg = (cfg & !0x03 & !0x10) | 0x20;
    if !write_cmd(0x60) || !write_data(cfg) {
        return None;
    }
    if !write_cmd(0xAE) {
        return None;
    }
    if !write_data(0xF4) {
        return None;
    }
    if read_data() != Some(0xFA) {
        return None;
    }
    let set = if select_scancode_set_1() {
        ScancodeSet::Set1
    } else {
        ScancodeSet::Set2
    };
    let mut dec = Decoder::new(set);
    dec.reset_modifiers();
    let _ = write_data(0xF3);
    let _ = read_data();
    let _ = write_data(0x00);
    let _ = read_data();
    let _ = write_data(0x00);
    let _ = read_data();
    flush_output();
    Some((dec, set))
}

fn select_scancode_set_1() -> bool {
    if !write_data(0xF0) {
        return false;
    }
    if read_data() != Some(0xFA) {
        return false;
    }
    if !write_data(0x01) {
        return false;
    }
    read_data() == Some(0xFA)
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
