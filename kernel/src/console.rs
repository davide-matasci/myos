//! Kernel console: the kernel's messages go to serial and, once the
//! `console` module has registered, to its framebuffer text screen 0;
//! keyboard input and the loadable keymap come from the same module.
//!
//! The module has text screens (`docs/tty.md`): 0 is the console's, 1–9
//! the ttys' ([`crate::pty`], [`write_screen`]); one is shown at a time
//! ([`show`]). What processes write to `/dev/console/data` goes to screen 0,
//! and to serial as well until `ttyS0` takes the serial port
//! ([`write_user`]). Boot status lines use a
//! structured `[ TAG ] label` form: serial gets plain text, the module
//! colours the tags. Output written before the module loads is kept in a
//! small buffer and replayed to it on registration, so the screen shows the
//! whole boot.
//!
//! All console output takes a single `OUT` mutex for the whole operation so
//! concurrent tasks cannot interleave serial bytes or split a status line.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicUsize, Ordering};
use myos_abi::{
    CONSOLE_BANNER, CONSOLE_INFO, CONSOLE_KEY_SCREEN, CONSOLE_STATUS_FAIL, CONSOLE_STATUS_INFO,
    CONSOLE_STATUS_OK, CONSOLE_STATUS_WARN, CONSOLE_TEXT, FramebufferInfo, ModuleConsoleOps,
};
use spin::{Mutex, Once};

use alloc::string::String;

use crate::arch::SerialPort;

/// The file the loaded keymap came from (`keymap` on the console's ctl).
static KEYMAP_PATH: Mutex<Option<String>> = Mutex::new(None);
/// Longest keymap text (`docs/keymap.md`).
const KEYMAP_MAX: usize = 8192;

/// The boot framebuffer Limine handed over (for the module), if any.
static FB_INFO: Once<FramebufferInfo> = Once::new();
/// The console module's screen and keyboards, once registered.
static OPS: Once<ModuleConsoleOps> = Once::new();
/// Serializes every console write end-to-end (serial + screen).
static OUT: Mutex<()> = Mutex::new(());
/// The text screen shown ([`show`]).
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// Boot output before the module loads: `(kind, text)` records, replayed on
/// registration. Status lines are stored as their serial text; the module
/// colours `[ TAG ]` prefixes itself.
const EARLY_CAP: usize = 16 * 1024;
struct Early {
    buf: [u8; EARLY_CAP],
    len: usize,
    /// The module registered: nothing more is buffered.
    done: bool,
}
static EARLY: Mutex<Early> = Mutex::new(Early {
    buf: [0; EARLY_CAP],
    len: 0,
    done: false,
});

/// Record the boot framebuffer (before any output). The screen itself is
/// painted by the console module once it loads.
pub fn set_framebuffer(info: FramebufferInfo) {
    FB_INFO.call_once(|| info);
}

/// The boot framebuffer for `KernelApi::framebuffer_info`.
pub fn framebuffer_info() -> Option<FramebufferInfo> {
    FB_INFO.get().copied()
}

/// The console module registers its screen and keyboards (once per boot).
/// The buffered boot output is replayed to it first.
pub fn register(ops: ModuleConsoleOps) -> bool {
    if OPS.get().is_some() {
        return false;
    }
    let _guard = OUT.lock();
    let mut early = EARLY.lock();
    early.done = true;
    OPS.call_once(|| ops);
    // Records: kind byte, u16 LE length, bytes.
    let mut i = 0;
    while i + 3 <= early.len {
        let kind = u32::from(early.buf[i]);
        let len = usize::from(u16::from_le_bytes([early.buf[i + 1], early.buf[i + 2]]));
        i += 3;
        let end = (i + len).min(early.len);
        unsafe { (ops.write_kind)(0, early.buf[i..end].as_ptr(), end - i, kind) };
        i = end;
    }
    early.len = 0;
    true
}

/// Keep `s` for the replay (dropped once the buffer is full).
fn early_record(kind: u32, s: &[u8]) {
    let mut early = EARLY.lock();
    if early.done || s.is_empty() {
        return;
    }
    let len = s.len().min(u16::MAX as usize);
    if early.len + 3 + len > EARLY_CAP {
        return;
    }
    let at = early.len;
    early.buf[at] = kind as u8;
    early.buf[at + 1..at + 3].copy_from_slice(&(len as u16).to_le_bytes());
    early.buf[at + 3..at + 3 + len].copy_from_slice(&s[..len]);
    early.len += 3 + len;
}

/// Write `s` to the console's screen, 0 (caller holds `OUT`).
fn screen_write(kind: u32, s: &[u8]) {
    match OPS.get() {
        Some(ops) => unsafe { (ops.write_kind)(0, s.as_ptr(), s.len(), kind) },
        None => early_record(kind, s),
    }
}

/// A tty's output: to its text screen `n` (1–9), painted when shown.
pub fn write_screen(n: usize, s: &[u8]) {
    if s.is_empty() {
        return;
    }
    let _guard = OUT.lock();
    if let Some(ops) = OPS.get() {
        unsafe { (ops.write_kind)(n as u32, s.as_ptr(), s.len(), CONSOLE_TEXT) };
    }
}

/// `ttyS0`'s output: to the serial port.
pub fn write_serial(s: &[u8]) {
    let _guard = OUT.lock();
    let mut serial = SerialPort::new();
    for &b in s {
        serial.write_byte(b);
    }
}

/// A process's write to `/dev/console/data`: the console's screen, and the
/// serial port as well until `ttyS0` has it (the boot's lines, init's and
/// the programs' it starts, are on both).
pub fn write_user(s: &[u8]) {
    let _guard = OUT.lock();
    if !crate::pty::serial_taken() {
        let mut serial = SerialPort::new();
        for &b in s {
            serial.write_byte(b);
        }
    }
    screen_write(CONSOLE_TEXT, s);
}

/// Show text screen `n` (`screen N` on `/dev/console/ctl`, Alt+N on the
/// keyboard): its tty gets the keyboard. False when there is no screen.
pub fn show(n: usize) -> bool {
    let Some(ops) = OPS.get() else {
        return false;
    };
    let _guard = OUT.lock();
    if unsafe { (ops.show)(n as u32) } != 0 {
        return false;
    }
    ACTIVE.store(n, Ordering::Relaxed);
    true
}

/// The text screen shown.
pub fn active() -> usize {
    ACTIVE.load(Ordering::Relaxed)
}

/// Timer-IRQ blink for the framebuffer block cursor (the module skips the
/// tick if a paint holds its lock).
pub fn cursor_blink() {
    if let Some(ops) = OPS.get() {
        unsafe { (ops.blink)() };
    }
}

/// Character-cell winsize for tty `TIOCGWINSZ`.
///
/// The screen's geometry when the console module has one; otherwise the
/// serial console geometry. A serial console has no intrinsic width, so stay
/// consistent with the framebuffer boots (x86 CI reports 160×100) instead of
/// the old fixed 80-char default: 80 made oksh's emacs editor wrap long
/// commands (the interactive curl line) with redraw artifacts.
pub fn winsize() -> (u16, u16) {
    if let Some(ops) = OPS.get() {
        let (mut rows, mut cols) = (0u16, 0u16);
        if unsafe { (ops.winsize)(&mut rows, &mut cols) } == 0 {
            return (rows, cols);
        }
    }
    (100, 160)
}

/// A local keyboard (PS/2, virtio-input) was found by the console module.
pub fn keyboard_present() -> bool {
    OPS.get().is_some_and(|ops| unsafe { (ops.keyboard_present)() } != 0)
}

/// The keyboard interrupts on input: its readers need not re-poll it.
pub fn keyboard_irq() -> bool {
    OPS.get().is_some_and(|ops| unsafe { (ops.keyboard_irq)() } != 0)
}

/// The screen shows the blinking cursor ([`cursor_blink`] does something):
/// CPU 0 wakes for it while idle.
pub fn cursor_blinks() -> bool {
    OPS.get().is_some_and(|ops| unsafe { (ops.cursor_blinks)() } != 0)
}

/// What the keyboard gave.
pub enum Key {
    /// A byte, keymap-translated.
    Byte(u8),
    /// Alt+digit: show that screen.
    Screen(usize),
}

/// The keyboard's next key, if one is pending.
pub fn keyboard_poll() -> Option<Key> {
    let ops = OPS.get()?;
    let b = unsafe { (ops.keyboard_poll)() };
    match b {
        0..=255 => Some(Key::Byte(b as u8)),
        _ if b >= CONSOLE_KEY_SCREEN => Some(Key::Screen((b - CONSOLE_KEY_SCREEN) as usize)),
        _ => None,
    }
}

/// Install a keymap from its text form (see `docs/keymap.md`). The module
/// reports a parse error itself.
fn keymap_load(text: &[u8]) -> bool {
    OPS.get().is_some_and(|ops| unsafe { (ops.keymap_load)(text.as_ptr(), text.len()) } == 0)
}

/// `keymap PATH` written to the console's control file (`docs/keymap.md`):
/// load the map text in the file at `path`, in the caller's view of the
/// tree. False when the file cannot be read, is empty or too long, or the
/// module refuses it; the loaded map stays in those cases.
pub fn keymap_load_file(path: &str) -> bool {
    let mut real = [0u8; crate::fs::vfs::PATH_MAX];
    let Some(n) = crate::fs::resolve_user_path(path, &mut real) else {
        return false;
    };
    let Ok(real) = core::str::from_utf8(&real[..n]) else {
        return false;
    };
    // Read for the caller: the policy decides, as for any file it opens.
    if !crate::sec::allowed(real, crate::sec::Rights::READ) {
        return false;
    }
    let Some(text) = crate::fs::read_all(real, KEYMAP_MAX + 1) else {
        return false;
    };
    if text.is_empty() || text.len() > KEYMAP_MAX || !keymap_load(&text) {
        return false;
    }
    *KEYMAP_PATH.lock() = Some(String::from(path));
    true
}

/// The file the loaded keymap came from, if one was loaded this boot.
pub fn keymap_path() -> Option<String> {
    KEYMAP_PATH.lock().clone()
}

pub fn write_str(s: &str) {
    let _ = Console.write_str(s);
}

/// `s` (UTF-8 or not) to serial and the screen in one hold of `OUT`: what
/// a module writes ([`crate::modules`]'s `write_str`) cannot be split by
/// another writer. A byte at a time, a module's status line and a getty's
/// prompt on another CPU came out mixed (`[P OK ] asswolinuxrd`, issue
/// #632).
pub fn write_bytes(s: &[u8]) {
    if s.is_empty() {
        return;
    }
    let _guard = OUT.lock();
    let mut serial = SerialPort::new();
    for &b in s {
        serial.write_byte(b);
    }
    screen_write(CONSOLE_TEXT, s);
}

/// Banner line (e.g. `Hello from myos`) in accent color on the screen.
pub fn write_banner(s: &str) {
    let _guard = OUT.lock();
    SerialPort::new().write_str(s).ok();
    screen_write(CONSOLE_BANNER, s.as_bytes());
}

/// Dim informational text (stdin hints, etc.).
pub fn write_info(s: &str) {
    let _guard = OUT.lock();
    SerialPort::new().write_str(s).ok();
    screen_write(CONSOLE_INFO, s.as_bytes());
}

/// `[ OK ] label` — tag in green on the screen, plain text on serial.
pub fn status_ok(label: &str) {
    write_status("OK", CONSOLE_STATUS_OK, label);
}

/// `[ FAIL ] label` — tag in red on the screen.
pub fn status_fail(label: &str) {
    write_status("FAIL", CONSOLE_STATUS_FAIL, label);
}

/// `[ INFO ] label` — informational status (blue tag on the screen).
pub fn status_info(label: &str) {
    write_status("INFO", CONSOLE_STATUS_INFO, label);
}

/// `[ WARN ] label` — warning (amber tag on the screen).
pub fn status_warn(label: &str) {
    write_status("WARN", CONSOLE_STATUS_WARN, label);
}

/// `[ WARN ] label` from an interrupt that must not wait
/// (`task::lockup_check`): false, and nothing written anywhere, while
/// another writer holds the console or the module holds its screen (a
/// stuck CPU may hold either; the caller tries again later). The screen
/// goes first, through the module's non-blocking op; before the module
/// is there, serial alone (the early buffer's lock is nobody's to wait on
/// from an interrupt).
pub fn status_warn_now(label: &str) -> bool {
    let Some(_guard) = OUT.try_lock() else {
        return false;
    };
    if let Some(ops) = OPS.get() {
        let tag = "WARN";
        let shown = unsafe {
            (ops.status_line_try)(tag.as_ptr(), tag.len(), CONSOLE_STATUS_WARN, label.as_ptr(), label.len())
        };
        if shown == 0 {
            return false;
        }
    }
    serial_status("WARN", label);
    true
}

/// `[ .. ] label` — in-progress boot step (blue tag on the screen).
pub fn status_progress(label: &str) {
    write_status("..", CONSOLE_STATUS_INFO, label);
}

fn write_status(tag: &str, kind: u32, label: &str) {
    // Hold OUT across every serial fragment + the screen line so one
    // `[ TAG ] label\n` is atomic w.r.t. other console writers.
    let _guard = OUT.lock();
    write_status_held(tag, kind, label);
}

/// [`write_status`] with `OUT` held.
fn write_status_held(tag: &str, kind: u32, label: &str) {
    serial_status(tag, label);
    match OPS.get() {
        Some(ops) => unsafe {
            (ops.status_line)(tag.as_ptr(), tag.len(), kind, label.as_ptr(), label.len())
        },
        None => {
            // Replayed as text: the module colours `[ TAG ] ` prefixes itself.
            early_record(CONSOLE_TEXT, b"[ ");
            early_record(CONSOLE_TEXT, tag.as_bytes());
            early_record(CONSOLE_TEXT, b" ]");
            if !label.is_empty() {
                early_record(CONSOLE_TEXT, b" ");
                early_record(CONSOLE_TEXT, label.as_bytes());
            }
            early_record(CONSOLE_TEXT, b"\n");
        }
    }
}

/// The status line's serial form, `[ TAG ] label`, with `OUT` held.
fn serial_status(tag: &str, label: &str) {
    let mut serial = SerialPort::new();
    let _ = serial.write_str("[ ");
    let _ = serial.write_str(tag);
    let _ = serial.write_str(" ]");
    if !label.is_empty() {
        let _ = serial.write_str(" ");
        let _ = serial.write_str(label);
    }
    let _ = serial.write_str("\n");
}

/// Formatted text without the heap: [`write_str`] for what is written
/// before the heap exists (`heap`'s line for an allocation that early).
pub fn write_fmt(args: fmt::Arguments) {
    let _ = Console.write_fmt(args);
}

pub fn flush() {
    let _guard = OUT.lock();
    SerialPort::new().flush();
}

struct Console;

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let _guard = OUT.lock();
        SerialPort::new().write_str(s)?;
        screen_write(CONSOLE_TEXT, s.as_bytes());
        Ok(())
    }
}
