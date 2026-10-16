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
//!
//! The controller's second (aux) port is the PS/2 mouse: three-byte
//! packets, four with an IntelliMouse wheel, into the console's pointer
//! (`/dev/console/mouse`, [`crate::mouse`]). Its bytes share the one output
//! buffer with the keyboard's, so an unread mouse byte holds keyboard bytes
//! back: the port is enabled only where its IRQ 12 is routed too, or when
//! the whole controller is polled anyway.

use core::sync::atomic::{AtomicBool, Ordering};

use ps2_scancode::{Decoder, RawDecoder, ScancodeSet};
use crate::kbd::{self, ByteFifo};
use myos_abi::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, Lock as Mutex};
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
/// The aux port's mouse is reporting; [`MOUSE`] assembles its packets.
static MOUSE_ON: AtomicBool = AtomicBool::new(false);
/// IRQ 12 is on.
static MOUSE_IRQ: AtomicBool = AtomicBool::new(false);
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

/// The mouse's packet so far and the buttons the last one held.
struct Packet {
    bytes: [u8; 4],
    len: usize,
    /// 4 with a wheel, 3 without.
    size: usize,
    buttons: u8,
}

static MOUSE: Mutex<Packet> = Mutex::new(Packet { bytes: [0; 4], len: 0, size: 3, buttons: 0 });

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
        mouse_init(irq);
    }
}

/// Bring up the aux port's mouse, after the keyboard: its defaults, the
/// knock for the IntelliMouse wheel, then reporting on. Without IRQ 12 an
/// interrupt-driven keyboard would wait behind the mouse's bytes, so the
/// port stays off then.
fn mouse_init(kbd_irq: bool) {
    let irq = crate::api().irq_enable(12, "ps2 mouse", interrupt, core::ptr::null_mut()) == 0;
    if kbd_irq && !irq {
        return;
    }
    let _controller = CONTROLLER.lock();
    if !write_cmd(0xA8) || !write_cmd(0x20) {
        return;
    }
    let Some(cfg) = read_data() else {
        return;
    };
    let cfg = (cfg & !CFG_MOUSE_CLK_DISABLE) | if irq { CFG_MOUSE_IRQ } else { 0 };
    // Set defaults (it answers with an ACK), then the sample rates 200,
    // 100 and 80: a wheel mouse then says it is one (id 3).
    if !write_cmd(0x60) || !write_data(cfg) || !aux_cmd(0xF6) {
        mouse_off();
        return;
    }
    let wheel = aux_cmd(0xF3) && aux_cmd(200) && aux_cmd(0xF3) && aux_cmd(100) && aux_cmd(0xF3) && aux_cmd(80)
        && aux_cmd(0xF2) && read_aux() == Some(3);
    if !aux_cmd(0xF4) {
        mouse_off();
        return;
    }
    MOUSE.lock().size = if wheel { 4 } else { 3 };
    MOUSE_IRQ.store(irq, Ordering::SeqCst);
    MOUSE_ON.store(true, Ordering::SeqCst);
    status_ok(if wheel { "mouse (ps2, wheel)" } else { "mouse (ps2)" });
}

/// The aux port's clock and interrupt off again (no mouse answered).
fn mouse_off() {
    flush_output();
    if write_cmd(0x20) {
        if let Some(cfg) = read_data() {
            let _ = write_cmd(0x60) && write_data((cfg & !CFG_MOUSE_IRQ) | CFG_MOUSE_CLK_DISABLE);
        }
    }
    let _ = write_cmd(0xA7);
}

/// `val` to the mouse (through the controller's 0xD4); true on its ACK.
fn aux_cmd(val: u8) -> bool {
    write_cmd(0xD4) && write_data(val) && read_aux() == Some(0xFA)
}

/// The next byte from the mouse; a keyboard byte in between is dropped (a
/// key pressed during the boot's mouse setup).
fn read_aux() -> Option<u8> {
    for _ in 0..100_000 {
        let status = inb(STATUS);
        if status & ST_OUT_FULL != 0 {
            let b = inb(DATA);
            if status & ST_AUX != 0 {
                return Some(b);
            }
        }
        core::hint::spin_loop();
    }
    None
}

/// The mouse is polled (no IRQ 12): its readers pump the controller.
pub fn pointer_polled() -> bool {
    MOUSE_ON.load(Ordering::SeqCst) && !MOUSE_IRQ.load(Ordering::SeqCst)
}

/// A byte from the mouse: a whole packet goes to the pointer. Bit 3 of a
/// packet's first byte is always set: a byte without it cannot start one,
/// which brings a packet that lost a byte back in step.
fn mouse_byte(b: u8) {
    let mut p = MOUSE.lock();
    if p.len == 0 && b & 0x08 == 0 {
        return;
    }
    let len = p.len;
    p.bytes[len] = b;
    p.len += 1;
    if p.len < p.size {
        return;
    }
    p.len = 0;
    let [flags, x, y, z] = p.bytes;
    // 9-bit motion, the sign in the first byte; PS/2's Y is up. A packet
    // that overflowed is dropped, as Linux does.
    if flags & 0xC0 == 0 {
        let dx = i32::from(x) - (i32::from(flags & 0x10) << 4);
        let dy = i32::from(y) - (i32::from(flags & 0x20) << 3);
        crate::mouse::pointer().motion(dx, -dy);
    }
    if p.size == 4 {
        // The wheel's 4 bits, signed; positive is towards the user.
        let dz = i32::from(((z & 0x0F) << 4) as i8 >> 4);
        crate::mouse::pointer().wheel(-dz);
    }
    let held = flags & 0x07;
    for (bit, code) in [(0, BTN_LEFT), (1, BTN_RIGHT), (2, BTN_MIDDLE)] {
        if (held ^ p.buttons) & (1 << bit) != 0 {
            crate::mouse::pointer().button(code, held & (1 << bit) != 0);
        }
    }
    p.buttons = held;
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
        let b = inb(DATA);
        if MOUSE_ON.load(Ordering::SeqCst) {
            mouse_byte(b);
        }
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
    if kbd::grabbed() || kbd::screen_key(kc, dec.alt()) {
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
