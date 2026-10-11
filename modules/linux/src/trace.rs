//! The layer's trace (`docs/linux-compat.md`, Debugging): with `1` written
//! to `/proc/linux_trace`, every syscall the layer does not implement is
//! reported on the console as it answers `ENOSYS`, with the calling process,
//! the number (the arch's own numbering) and the first three arguments. A
//! program that gives up quietly then says what it asked for. `0` turns it
//! off again; it starts off.

use core::sync::atomic::{AtomicBool, Ordering};

use super::abi::{err, ENOSYS};
use crate::k::{api, task};

static ON: AtomicBool = AtomicBool::new(false);

const NAME: &str = "linux_trace";

/// Publish `/proc/linux_trace` (reads `0` or `1`), writable by whoever the
/// policy lets write `/proc` (`docs/security.md`: root).
pub fn init() {
    publish();
    let _ = api().proc_set_writer(NAME, Some(write));
}

fn publish() {
    let _ = api().proc_register(NAME, if ON.load(Ordering::Relaxed) { b"1\n" } else { b"0\n" });
}

/// A write of `1` or `0` (surrounding white space ignored) turns the trace
/// on or off; anything else is refused.
unsafe extern "C" fn write(data: *const u8, len: usize) -> i32 {
    // Copied before it is looked at: the buffer is the writer's.
    let mut buf = [0u8; 8];
    if data.is_null() || len == 0 || len > buf.len() {
        return -1;
    }
    unsafe { core::ptr::copy_nonoverlapping(data, buf.as_mut_ptr(), len) };
    let on = match buf[..len].trim_ascii() {
        b"1" => true,
        b"0" => false,
        _ => return -1,
    };
    ON.store(on, Ordering::Relaxed);
    publish();
    len as i32
}

/// The answer to a syscall the layer does not implement (`ENOSYS`),
/// reported first when the trace is on.
pub fn unimplemented(nr: usize, a: &[usize; 6]) -> usize {
    if ON.load(Ordering::Relaxed) {
        api().write_str(&alloc::format!(
            "linux: pid {} syscall {nr} ({:#x}, {:#x}, {:#x}): ENOSYS\n",
            task::current_pid(),
            a[0],
            a[1],
            a[2]
        ));
    }
    err(ENOSYS)
}
