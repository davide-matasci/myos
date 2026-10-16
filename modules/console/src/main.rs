//! Console module: the framebuffer text screen (ANSI-capable, status-line
//! colouring, block cursor), `/dev/fb` for programs that draw on the screen
//! themselves ([`fbdev`]), the local keyboards (PS/2 on x86_64,
//! virtio-input on the `virt` boards; raw presses and releases at
//! `/dev/console/kbd`, [`kbdev`]), the loadable keymap, and the pointer
//! other modules' drivers feed (`/dev/console/mouse`, [`mouse`]).
//!
//! The screen is one of [`SCREENS`] text screens at a time (`docs/tty.md`):
//! 0 the console's, 1–9 the ttys'. Each keeps its cells, so the kernel
//! writes to any of them and the one shown is painted whole on a switch
//! (Alt+0…9 on the keyboard, [`kbd::screen_key`]). Serial is the kernel's.
//! Speaks only through [`myos_abi::KernelApi`].

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

mod fb;
mod fbdev;
mod font;
mod kbd;
mod kbdev;
mod keymap;
mod mouse;
#[cfg(target_arch = "x86_64")]
mod ps2;
#[cfg(not(target_arch = "x86_64"))]
mod virtio_input;

#[cfg(target_arch = "x86_64")]
use ps2 as keyboard;
#[cfg(not(target_arch = "x86_64"))]
use virtio_input as keyboard;

use myos_abi::{
    ABI_VERSION, ApiCell, CONSOLE_BANNER, CONSOLE_INFO, CONSOLE_KEY_SCREEN, CONSOLE_SCREENS, CONSOLE_STATUS_FAIL,
    CONSOLE_STATUS_INFO, CONSOLE_STATUS_OK, CONSOLE_STATUS_WARN, FramebufferInfo, KernelApi, Lock,
    ModuleConsoleOps,
};

use core::sync::atomic::{AtomicBool, Ordering};

use fb::FrameBufferWriter;

static API: ApiCell = ApiCell::new();

/// The text screens: made when first written to or shown, `active` the one
/// on the framebuffer (unless a program took it, [`fbdev`]).
pub(crate) struct Screens {
    info: Option<FramebufferInfo>,
    screens: [Option<FrameBufferWriter>; CONSOLE_SCREENS],
    pub(crate) active: usize,
}

impl Screens {
    /// Screen `n`, made blank when it is new.
    fn get(&mut self, n: usize) -> Option<&mut FrameBufferWriter> {
        let info = self.info.as_ref()?;
        let slot = self.screens.get_mut(n)?;
        if slot.is_none() {
            *slot = FrameBufferWriter::from_info(info);
        }
        slot.as_mut()
    }

    /// The screen on the framebuffer.
    pub(crate) fn active(&mut self) -> Option<&mut FrameBufferWriter> {
        self.get(self.active)
    }
}

pub(crate) static FB: Lock<Screens> = Lock::new(Screens {
    info: None,
    screens: [const { None }; CONSOLE_SCREENS],
    active: 0,
});
/// There is a framebuffer.
static SCREEN: AtomicBool = AtomicBool::new(false);

pub(crate) fn api() -> &'static KernelApi {
    API.get()
}

/// `[ OK ] label` on the kernel console (serial + this screen).
pub(crate) fn status_ok(label: &str) {
    myos_abi::status_ok(api(), label);
}

pub(crate) fn status_fail(label: &str) {
    myos_abi::status_fail(api(), label);
}

fn text<'a>(ptr: *const u8, len: usize) -> &'a str {
    if ptr.is_null() || len == 0 {
        return "";
    }
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    core::str::from_utf8(bytes).unwrap_or("")
}

unsafe extern "C" fn write_kind(screen: u32, buf: *const u8, len: usize, kind: u32) {
    if buf.is_null() || len == 0 {
        return;
    }
    let bytes = unsafe { core::slice::from_raw_parts(buf, len) };
    let mut guard = FB.lock();
    let Some(w) = guard.get(screen as usize) else {
        return;
    };
    match kind {
        CONSOLE_BANNER => w.put_str_colored(text(buf, len), fb::ACCENT),
        CONSOLE_INFO => w.put_str_colored(text(buf, len), fb::DIM),
        _ => {
            for &b in bytes {
                w.put_byte(b);
            }
        }
    }
}

unsafe extern "C" fn status_line(
    tag: *const u8,
    tag_len: usize,
    kind: u32,
    label: *const u8,
    label_len: usize,
) {
    let color = match kind {
        CONSOLE_STATUS_OK => fb::OK,
        CONSOLE_STATUS_FAIL => fb::FAIL,
        CONSOLE_STATUS_WARN => fb::WARN,
        _ => fb::INFO,
    };
    let _ = CONSOLE_STATUS_INFO;
    let mut guard = FB.lock();
    if let Some(w) = guard.get(0) {
        w.put_status_line(text(tag, tag_len), color, text(label, label_len));
    }
}

/// Put screen `n` on the framebuffer, the one shown before kept in its
/// cells. While a program has the screen ([`fbdev`]) it only becomes the
/// one shown when the program gives the screen back.
unsafe extern "C" fn show(n: u32) -> i32 {
    let n = n as usize;
    let mut guard = FB.lock();
    if n >= CONSOLE_SCREENS || guard.get(n).is_none() {
        return -1;
    }
    if n == guard.active {
        return 0;
    }
    if !fbdev::graphics() {
        if let Some(w) = guard.active() {
            w.hide();
        }
    }
    guard.active = n;
    if !fbdev::graphics() {
        if let Some(w) = guard.active() {
            w.show();
        }
    }
    0
}

unsafe extern "C" fn winsize(rows: *mut u16, cols: *mut u16) -> i32 {
    if rows.is_null() || cols.is_null() {
        return -1;
    }
    let mut guard = FB.lock();
    let Some(w) = guard.active() else {
        return -1;
    };
    let (r, c) = w.winsize();
    unsafe {
        *rows = r;
        *cols = c;
    }
    0
}

/// Timer tick: toggle the block cursor. Non-blocking: if a paint holds the
/// lock this phase is skipped and the next tick re-syncs.
unsafe extern "C" fn blink() {
    if fbdev::graphics() {
        return;
    }
    if let Some(mut guard) = FB.try_lock() {
        if let Some(w) = guard.active() {
            w.blink_toggle();
        }
    }
}

unsafe extern "C" fn keyboard_present() -> i32 {
    i32::from(keyboard::present())
}

unsafe extern "C" fn keyboard_irq() -> i32 {
    i32::from(keyboard::irq())
}

/// The text screen is there and not taken over by `/dev/fb`. No lock: the
/// kernel asks with interrupts off, from its idle loop.
unsafe extern "C" fn cursor_blinks() -> i32 {
    i32::from(SCREEN.load(Ordering::Relaxed) && !fbdev::graphics())
}

/// The next key byte, or [`CONSOLE_KEY_SCREEN`] plus a screen Alt+digit
/// asked for, or -1.
unsafe extern "C" fn keyboard_poll() -> i32 {
    if let Some(n) = kbd::take_screen() {
        return CONSOLE_KEY_SCREEN + n as i32;
    }
    let b = keyboard::poll_byte();
    if let Some(n) = kbd::take_screen() {
        // The byte before it is not lost: it comes first.
        return match b {
            Some(b) => {
                kbd::put_back_screen(n);
                i32::from(b)
            }
            None => CONSOLE_KEY_SCREEN + n as i32,
        };
    }
    b.map_or(-1, i32::from)
}

unsafe extern "C" fn keymap_load(ptr: *const u8, len: usize) -> i32 {
    if ptr.is_null() || len == 0 {
        return -1;
    }
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    match keymap::load_from_text(bytes) {
        Ok(()) => 0,
        Err(e) => {
            status_fail(e);
            -1
        }
    }
}

unsafe extern "C" fn keymap_loaded() -> i32 {
    i32::from(keymap::is_loaded())
}

static OPS: ModuleConsoleOps = ModuleConsoleOps {
    write_kind,
    status_line,
    show,
    winsize,
    blink,
    keyboard_present,
    keyboard_poll,
    keymap_load,
    keymap_loaded,
    keyboard_irq,
    cursor_blinks,
};

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api_ptr: *const KernelApi) -> i32 {
    if api_ptr.is_null() {
        return -1;
    }
    let api: &'static KernelApi = unsafe { &*api_ptr };
    if api.abi_version != ABI_VERSION {
        return -2;
    }
    unsafe { API.set(api) };
    let mut info = FramebufferInfo::default();
    let screen = api.framebuffer_info(&mut info) == 0 && info.addr != 0;
    if screen {
        let mut screens = FB.lock();
        screens.info = Some(info);
        // The console's screen, shown from the start.
        if let Some(w) = screens.get(0) {
            w.show();
            SCREEN.store(true, Ordering::Relaxed);
        }
    }
    keyboard::init();
    if keyboard::present() && kbdev::mount() != 0 {
        status_fail("console: /dev/console/kbd");
    }
    if mouse::init() != 0 {
        status_fail("console: /dev/console/mouse");
    }
    let rc = api.console_register(&OPS);
    if screen && fbdev::mount(info) != 0 {
        status_fail("console: /dev/fb");
    }
    rc
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

// So `cargo build --bin console` links. The kernel never jumps here.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
