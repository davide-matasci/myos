//! `errno` for myos (single-threaded userspace; no TLS yet).

use crate::Errno;

static mut MYOS_ERRNO: i32 = 0;

pub fn with_description<F, T>(err: Errno, callback: F) -> T
where
    F: FnOnce(Result<&str, Errno>) -> T,
{
    callback(Ok(match err.0 {
        0 => "Success",
        1 => "Operation not permitted",
        2 => "No such file or directory",
        9 => "Bad file descriptor",
        12 => "Cannot allocate memory",
        22 => "Invalid argument",
        _ => "Unknown error",
    }))
}

pub const STRERROR_NAME: &str = "myos";

pub fn errno() -> Errno {
    unsafe { Errno(MYOS_ERRNO) }
}

pub fn set_errno(Errno(code): Errno) {
    unsafe {
        MYOS_ERRNO = code;
    }
}
