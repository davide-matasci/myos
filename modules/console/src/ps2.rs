//! PS/2 keyboard via the 8042 controller: IRQ 1 through the I/O APIC when
//! the kernel can route it, polled otherwise. The interrupt only wakes the
//! console's readers; they read the controller (`poll_byte`, `pump`), which
//! lowers the line for the next byte's edge.
//!
//! Works on QEMU i8042 and typical PC hardware (including USB keyboards in
//! legacy PS/2 mode). If probe/init fails we stay on serial-only stdin.
//!
//! Real hardware almost always speaks scancode set 2 on the keyboard wire.
//! The 8042 can translate that to set 1 for the host (configuration bit 6).
//! We enable translation when possible and always decode set 1 at the port.
//!
//! Scancodes become **keycodes** in `ps2-scancode`; ASCII comes from the
//! loadable kernel keymap (empty until one is loaded through the console's
//! control file, `docs/keymap.md`).

use core::sync::atomic::{AtomicBool, Ordering};

use ps2_scancode::{Decoder, RawDecoder, ScancodeSet};
use crate::kbd::{self, ByteFifo};
use myos_abi::Lock as Mutex;
use crate::{status_fail, status_ok};

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
/// IRQ 1 is on ([`interrupt`]).
static IRQ: AtomicBool = AtomicBool::new(false);
static DECODER: Mutex<Option<Decoder>> = Mutex::new(None);
/// The same bytes as every press and release (`/dev/console/kbd`).
static RAW_DECODER: Mutex<Option<RawDecoder>> = Mutex::new(None);
/// Multi-byte sequences (CSI arrows) ready to drain from `poll_byte`.
static FIFO: Mutex<ByteFifo> = Mutex::new(ByteFifo::new());
/// Held from the status read to the decode of the byte taken: the kernel's
/// console-input thread and a `/dev/console/kbd` reader drain the
/// controller at the same time. Unserialized, both saw the same byte
/// waiting; one took it, the other read the emptied data port, which gives
/// the last byte again, and decoded a second press (or decoded them out of
/// order).
static CONTROLLER: Mutex<()> = Mutex::new(());

pub fn init() {
    // The handler first: the controller raises IRQ 1 as soon as the probe
    // enables it.
    let irq = crate::api().irq_enable(1, "ps2 keyboard", interrupt, core::ptr::null_mut()) == 0;
    if let Some((dec, translate)) = probe_and_enable(irq) {
        IRQ.store(irq, Ordering::SeqCst);
        *DECODER.lock() = Some(dec);
        *RAW_DECODER.lock() = Some(RawDecoder::new(ScancodeSet::Set1));
        READY.store(true, Ordering::SeqCst);
        status_ok(if translate { "keyboard (xlate, set 1)" } else { "keyboard (raw, set 1)" });
        if ps2_scancode::self_test() {
            status_ok("keyboard decode");
        } else {
            status_fail("keyboard decode");
        }
    }
}

pub fn present() -> bool {
    READY.load(Ordering::SeqCst)
}

pub fn irq() -> bool {
    IRQ.load(Ordering::SeqCst)
}

/// IRQ 1: a byte waits in the controller. The readers take it.
unsafe extern "C" fn interrupt(_ctx: *mut core::ffi::c_void) {
    crate::api().console_input();
}

/// Non-blocking: one keyboard byte from the keyboard, if any.
///
/// Drains the multi-byte FIFO first, then decodes fresh scancodes. Returns
/// `None` while no keymap is loaded or no key is pending (serial still works).
pub fn poll_byte() -> Option<u8> {
    if !READY.load(Ordering::SeqCst) {
        return None;
    }
    for _ in 0..PUMP_MAX {
        if let Some(b) = FIFO.lock().pop() {
            return Some(b);
        }
        if !feed_one() {
            break;
        }
    }
    FIFO.lock().pop()
}

/// Take the controller's pending bytes (into the tty FIFO, or the raw queue
/// while `/dev/console/kbd` is held).
pub fn pump() {
    if !READY.load(Ordering::SeqCst) {
        return;
    }
    for _ in 0..PUMP_MAX {
        if !feed_one() {
            break;
        }
    }
}

/// Bytes one call takes from the controller at most.
const PUMP_MAX: usize = 64;

/// Read and decode one byte from the controller; false when none waits.
fn feed_one() -> bool {
    let _controller = CONTROLLER.lock();
    let status = inb(STATUS);
    if status & ST_OUT_FULL == 0 {
        return false;
    }
    if status & ST_AUX != 0 {
        let _ = inb(DATA);
        return true;
    }
    let sc = inb(DATA);
    if let Some((code, pressed)) = RAW_DECODER.lock().as_mut().and_then(|d| d.feed(sc)) {
        kbd::raw_key(code, pressed);
    }
    let mut guard = DECODER.lock();
    let Some(dec) = guard.as_mut() else {
        return true;
    };
    // The character decoder sees every byte too, so its modifiers stay
    // right; held by `/dev/console/kbd`, the tty gets no characters.
    let Some(kc) = dec.feed(sc) else {
        return true;
    };
    if kbd::grabbed() {
        return true;
    }
    if let Some(kb) = kbd::translate(kc, dec.shift(), dec.altgr(), dec.ctrl()) {
        FIFO.lock().push_bytes(kb.as_slice());
    }
    true
}

/// Find and set up the keyboard; with `irq`, the controller raises IRQ 1
/// when a keyboard byte waits.
fn probe_and_enable(irq: bool) -> Option<(Decoder, bool)> {
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
        | CFG_TRANSLATE
        | if irq { CFG_KBD_IRQ } else { 0 };
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
    let mut dec = Decoder::new(ScancodeSet::Set1);
    dec.reset_modifiers();
    let _ = write_data(0xF3);
    let _ = read_data();
    let _ = write_data(0x00);
    let _ = read_data();
    flush_output();
    Some((dec, translate))
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
