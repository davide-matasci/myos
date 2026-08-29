//! PS/2 keyboard via the 8042 controller (poll, no PIC IRQ).
//!
//! Works on QEMU i8042 and typical PC hardware (including USB keyboards in
//! legacy PS/2 mode). If probe/init fails we stay on serial-only stdin.
//!
//! Real hardware almost always speaks scancode set 2 on the keyboard wire.
//! The 8042 can translate that to set 1 for the host (configuration bit 6).
//! We enable translation when possible and decode set 1 at the port; if the
//! port still delivers set 2 (translation off), we query the keyboard and
//! auto-switch on the set-2 break prefix (0xF0).

use core::sync::atomic::{AtomicBool, Ordering};

use ps2_scancode::{Decoder, ScancodeSet};
use spin::Mutex;

use crate::console;

const DATA: u16 = 0x60;
const STATUS: u16 = 0x64;

const ST_OUT_FULL: u8 = 1;
const ST_IN_FULL: u8 = 2;
const ST_AUX: u8 = 0x20; // data in 0x60 is from mouse/aux port

/// Controller configuration byte bits (8042 RAM byte 0).
const CFG_KBD_IRQ: u8 = 1 << 0;
const CFG_MOUSE_IRQ: u8 = 1 << 1;
const CFG_KBD_CLK_DISABLE: u8 = 1 << 4;
const CFG_MOUSE_CLK_DISABLE: u8 = 1 << 5;
const CFG_TRANSLATE: u8 = 1 << 6;

static READY: AtomicBool = AtomicBool::new(false);
static DECODER: Mutex<Option<Decoder>> = Mutex::new(None);

pub fn init() {
    if let Some((dec, translate, set)) = probe_and_enable() {
        *DECODER.lock() = Some(dec);
        READY.store(true, Ordering::SeqCst);
        console::write_str("kbd ok (");
        console::write_str(if translate { "xlate" } else { "raw" });
        console::write_str(", set ");
        console::write_str(if set == ScancodeSet::Set1 { "1" } else { "2" });
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
    let mut guard = DECODER.lock();
    let dec = guard.as_mut()?;
    if dec.autodetect_set2_break_prefix(sc) {
        console::write_str("kbd: switched to set 2 (0xF0 prefix)\n");
        return None;
    }
    if dec.autodetect_set2_make(sc) {
        console::write_str("kbd: switched to set 2 (make code)\n");
    }
    dec.feed(sc)
}

fn probe_and_enable() -> Option<(Decoder, bool, ScancodeSet)> {
    flush_output();
    if !write_cmd(0xAD) || !write_cmd(0xA7) {
        return None;
    }
    flush_output();
    if !write_cmd(0x20) {
        return None;
    }
    let cfg = read_data()?;
    // Enable set-2→set-1 translation at the controller (standard PC behavior).
    let cfg = (cfg & !(CFG_KBD_IRQ | CFG_MOUSE_IRQ | CFG_KBD_CLK_DISABLE))
        | CFG_MOUSE_CLK_DISABLE
        | CFG_TRANSLATE;
    if !write_cmd(0x60) || !write_data(cfg) {
        return None;
    }
    if !write_cmd(0x20) {
        return None;
    }
    let verified = read_data()?;
    let translate = verified & CFG_TRANSLATE != 0;
    if !write_cmd(0xAE) {
        return None;
    }
    if !write_data(0xF4) {
        return None;
    }
    if read_data() != Some(0xFA) {
        return None;
    }
    flush_output();
    let set = if translate {
        ScancodeSet::Set1
    } else {
        query_keyboard_set().unwrap_or(ScancodeSet::Set2)
    };
    let mut dec = Decoder::new(set);
    dec.reset_modifiers();
    let _ = write_data(0xF3);
    let _ = read_data();
    let _ = write_data(0x00);
    let _ = read_data();
    flush_output();
    Some((dec, translate, set))
}

/// Keyboard command `F0 00`: which scancode set the device emits (when translation is off).
fn query_keyboard_set() -> Option<ScancodeSet> {
    if !write_data(0xF0) {
        return None;
    }
    if read_data() != Some(0xFA) {
        return None;
    }
    if !write_data(0x00) {
        return None;
    }
    if read_data() != Some(0xFA) {
        return None;
    }
    match read_data()? {
        0x43 => Some(ScancodeSet::Set1),
        0x41 => Some(ScancodeSet::Set2),
        _ => Some(ScancodeSet::Set2),
    }
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
