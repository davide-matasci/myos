//! `/dev/console/kbd`: every key press and release, for programs that read
//! the keyboard themselves (a game, a graphical session on `/dev/fb`).
//!
//! One event per line: `d <code>[ <char>]` for a press, `u <code>` for a
//! release. `code` is the Linux `KEY_*` number; `char` is what the loaded
//! keymap gives the key with the current shift/AltGr, as UTF-8, left out for
//! space, control characters and keys it does not map. No autorepeat.
//!
//! Opening the file takes the keyboard: the tty gets no keys until the fd
//! closes (or its program exits); a second open fails. Serial input is
//! unaffected.

use myos_abi::{MYOS_POLL_RECHECK, MYOS_POLLIN, MYOS_READ_WAIT, ModuleVfsOps, VfsStatInfo};

use crate::{api, kbd, keyboard};

const S_IFREG: u32 = 0o100000;

fn root(path: *const u8, len: usize) -> bool {
    path.is_null() || len == 0 || unsafe { core::slice::from_raw_parts(path, len) } == b"."
}

unsafe extern "C" fn kbd_lookup(_: *const u8, _: usize, _: *mut *const u8, _: *mut usize) -> i32 {
    -1
}

unsafe extern "C" fn kbd_stat(path: *const u8, len: usize, out: *mut VfsStatInfo) -> i32 {
    if !root(path, len) {
        return -1;
    }
    unsafe {
        *out = VfsStatInfo { mode: S_IFREG | 0o444, size: 0, ino: 1, nlink: 1, mtime: 0, atime: 0 };
    }
    0
}

unsafe extern "C" fn kbd_listdir(_: *const u8, _: usize, _: *mut u8, _: usize, _: *mut usize) -> i32 {
    -1
}

/// Whole event lines, or [`MYOS_READ_WAIT`] while none is queued.
unsafe extern "C" fn kbd_read(path: *const u8, len: usize, _pos: usize, buf: *mut u8, cap: usize) -> i32 {
    if !root(path, len) || buf.is_null() {
        return -1;
    }
    keyboard::pump();
    if !kbd::raw_pending() {
        return MYOS_READ_WAIT;
    }
    let out = unsafe { core::slice::from_raw_parts_mut(buf, cap) };
    kbd::raw_read(out) as i32
}

unsafe extern "C" fn kbd_open(path: *const u8, len: usize) -> i32 {
    if root(path, len) && kbd::grab() { 0 } else { -1 }
}

unsafe extern "C" fn kbd_release(path: *const u8, len: usize) -> i32 {
    if root(path, len) {
        kbd::ungrab();
    }
    0
}

/// Readable while events are queued. A keyboard without an interrupt is
/// polled: pollers re-check it.
unsafe extern "C" fn kbd_poll(_: *const u8, _: usize) -> u32 {
    keyboard::pump();
    let recheck = if keyboard::irq() { 0 } else { MYOS_POLL_RECHECK };
    recheck | if kbd::raw_pending() { MYOS_POLLIN } else { 0 }
}

/// Mount `/dev/console/kbd`. 0 ok, negative on error.
pub fn mount() -> i32 {
    // Built here, not in a static: aarch64 ET_EXEC modules do not relocate
    // fn pointers in .rodata (see netfs).
    let ops = ModuleVfsOps {
        lookup: kbd_lookup,
        stat: kbd_stat,
        listdir: kbd_listdir,
        register: None,
        read: Some(kbd_read),
        write: None,
        create: None,
        truncate: None,
        mkdir: None,
        rmdir: None,
        unlink: None,
        rename: None,
        symlink: None,
        readlink: None,
        release: Some(kbd_release),
        mmap: None,
        poll: Some(kbd_poll),
        open: Some(kbd_open),
        set_times: None,
        unmount: None,
        unlink_keep: None,
        read_ino: None,
        write_ino: None,
        stat_ino: None,
        forget_ino: None,
        set_size: None,
        set_size_ino: None,
        file_id: None,
        set_times_ino: None,
    };
    api().vfs_mount("kbd", "dev/console/kbd", &ops)
}
