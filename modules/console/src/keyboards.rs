//! The keyboards other modules drive (`usb_hid`'s), through the
//! [`KEYBOARD_SERVICE`] table: their presses and releases, in Linux
//! `KEY_*` codes, type into the same tty, switch the same screens and fill
//! the same `/dev/console/kbd` as the console's own keyboard
//! ([`kbd::LinuxKeys`]). All of them are one keyboard: their modifiers are
//! shared.
//!
//! A USB keyboard reports what is held, not repeats: a key held down is
//! repeated here, [`REPEAT_DELAY_MS`] after its press and every
//! [`REPEAT_EVERY_MS`] after that, by a thread of the module, until it is
//! released or another key is pressed. Only the tty sees the repeats (the
//! raw keyboard's reader repeats keys itself).

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use myos_abi::{KEYBOARD_SERVICE, KEYBOARD_VERSION, KeyboardOps, LED_CAPS_LOCK, Lock};

use crate::kbd::{self, ByteFifo, LinuxKeys};
use crate::{api, kbdev, status_fail};

/// A held key starts repeating this long after its press.
const REPEAT_DELAY_MS: u64 = 500;
/// Then it repeats this often.
const REPEAT_EVERY_MS: u64 = 33;

static KEYS: Lock<LinuxKeys> = Lock::new(LinuxKeys::new());
/// What the keys typed, for the console's `keyboard_poll`.
static FIFO: Lock<ByteFifo> = Lock::new(ByteFifo::new());
/// The keyboards attached now.
static ATTACHED: AtomicU32 = AtomicU32::new(0);
/// The repeat thread runs.
static STARTED: AtomicBool = AtomicBool::new(false);

/// The key held for repeating, and when it repeats next.
struct Repeat {
    code: Option<u16>,
    next_ns: u64,
}

static REPEAT: Lock<Repeat> = Lock::new(Repeat { code: None, next_ns: 0 });

/// What the repeat thread blocks on: [`REPEAT`]'s address.
fn repeat_key() -> usize {
    &REPEAT as *const Lock<Repeat> as usize
}

fn ms(n: u64) -> u64 {
    n * 1_000_000
}

/// One event of a keyboard, and the console's readers woken for it.
fn event(code: u16, value: u32) {
    KEYS.lock().event(code, value, &mut FIFO.lock());
    api().console_input();
}

unsafe extern "C" fn attach() {
    if ATTACHED.fetch_add(1, Ordering::SeqCst) == 0 && !crate::keyboard::present() && kbdev::mount() != 0 {
        status_fail("console: /dev/console/kbd");
    }
    if !STARTED.swap(true, Ordering::SeqCst) {
        if api().thread_spawn("kbd-repeat", repeat_thread, core::ptr::null_mut()) != 0 {
            STARTED.store(false, Ordering::SeqCst);
            status_fail("console: key repeat");
        }
    }
}

unsafe extern "C" fn detach() {
    ATTACHED.fetch_sub(1, Ordering::SeqCst);
}

unsafe extern "C" fn key(code: u16, pressed: bool) {
    event(code, u32::from(pressed));
    if kbd::modifier(code) {
        return;
    }
    let mut r = REPEAT.lock();
    if pressed {
        r.code = Some(code);
        r.next_ns = api().monotonic_ns() + ms(REPEAT_DELAY_MS);
    } else if r.code == Some(code) {
        r.code = None;
    } else {
        return;
    }
    drop(r);
    api().wake(repeat_key());
}

unsafe extern "C" fn leds() -> u8 {
    if kbd::caps_lock() { LED_CAPS_LOCK } else { 0 }
}

/// Repeats the held key, while there is one; sleeps otherwise.
unsafe extern "C" fn repeat_thread(_ctx: *mut core::ffi::c_void) {
    loop {
        let seq = api().wait_seq();
        let now = api().monotonic_ns();
        let due = {
            let mut r = REPEAT.lock();
            match r.code {
                Some(code) if now >= r.next_ns => {
                    r.next_ns = now + ms(REPEAT_EVERY_MS);
                    Some(code)
                }
                _ => None,
            }
        };
        if let Some(code) = due {
            event(code, 2);
        }
        let deadline = {
            let r = REPEAT.lock();
            if r.code.is_some() { r.next_ns } else { 0 }
        };
        api().block_until(repeat_key(), seq, deadline);
    }
}

static OPS: KeyboardOps = KeyboardOps { version: KEYBOARD_VERSION, attach, detach, key, leds };

/// A keyboard of another module's is attached.
pub fn attached() -> bool {
    ATTACHED.load(Ordering::SeqCst) != 0
}

/// The next byte the keys typed.
pub fn poll_byte() -> Option<u8> {
    FIFO.lock().pop()
}

/// Publish the [`KEYBOARD_SERVICE`]. 0 ok, negative on error.
pub fn init() -> i32 {
    api().service_register(KEYBOARD_SERVICE, &OPS)
}
