//! Console module: the framebuffer text screen (ANSI-capable, status-line
//! colouring, block cursor), `/dev/fb` for programs that draw on the screen
//! themselves ([`fbdev`]), the local keyboards (PS/2 on x86_64,
//! virtio-input on the `virt` boards; raw presses and releases at
//! `/dev/console/kbd`, [`kbdev`]) and the loadable keymap.
//!
//! Serial is the kernel's; this module paints what the kernel also sends to
//! serial, and feeds keyboard bytes into the kernel's console line
//! discipline. Speaks only through [`myos_abi::KernelApi`].

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

mod fb;
mod fbdev;
mod font;
mod kbd;
mod kbdev;
mod keymap;
#[cfg(target_arch = "x86_64")]
mod ps2;
#[cfg(not(target_arch = "x86_64"))]
mod virtio_input;

#[cfg(target_arch = "x86_64")]
use ps2 as keyboard;
#[cfg(not(target_arch = "x86_64"))]
use virtio_input as keyboard;

use myos_abi::{
    ABI_VERSION, ApiCell, CONSOLE_BANNER, CONSOLE_INFO, CONSOLE_STATUS_FAIL, CONSOLE_STATUS_INFO,
    CONSOLE_STATUS_OK, CONSOLE_STATUS_WARN, FramebufferInfo, KernelApi, Lock, ModuleConsoleOps,
};

use fb::FrameBufferWriter;

static API: ApiCell = ApiCell::new();
static FB: Lock<Option<FrameBufferWriter<'static>>> = Lock::new(None);

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

unsafe extern "C" fn write_kind(buf: *const u8, len: usize, kind: u32) {
    if buf.is_null() || len == 0 {
        return;
    }
    if fbdev::graphics() {
        return;
    }
    let bytes = unsafe { core::slice::from_raw_parts(buf, len) };
    let mut guard = FB.lock();
    let Some(w) = guard.as_mut() else {
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
    if fbdev::graphics() {
        return;
    }
    let mut guard = FB.lock();
    if let Some(w) = guard.as_mut() {
        w.put_status_line(text(tag, tag_len), color, text(label, label_len));
    }
}

unsafe extern "C" fn winsize(rows: *mut u16, cols: *mut u16) -> i32 {
    if rows.is_null() || cols.is_null() {
        return -1;
    }
    let guard = FB.lock();
    let Some(w) = guard.as_ref() else {
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
        if let Some(w) = guard.as_mut() {
            w.blink_toggle();
        }
    }
}

unsafe extern "C" fn keyboard_present() -> i32 {
    i32::from(keyboard::present())
}

unsafe extern "C" fn keyboard_poll() -> i32 {
    keyboard::poll_byte().map_or(-1, i32::from)
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
    winsize,
    blink,
    keyboard_present,
    keyboard_poll,
    keymap_load,
    keymap_loaded,
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
        let mut w = FrameBufferWriter::from_info(&info);
        w.clear();
        *FB.lock() = Some(w);
    }
    keyboard::init();
    if keyboard::present() && kbdev::mount() != 0 {
        status_fail("console: /dev/console/kbd");
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
