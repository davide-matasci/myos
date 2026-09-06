//! myos I/O error strings.
//!
//! The kernel currently returns a single `SYSERR` (`usize::MAX`) for failed
//! syscalls; std maps that to `-1` → raw os error `1`. Do **not** use the
//! upstream `generic` backend: its `error_string` always returns
//! `"operation successful"`, which hid real open/read failures (e.g. riscv64
//! `uutils cat` after findnest).

use crate::io;

pub fn errno() -> i32 {
    0
}

pub fn is_interrupted(_code: i32) -> bool {
    false
}

pub fn decode_error_kind(code: i32) -> io::ErrorKind {
    match code {
        0 => io::ErrorKind::Uncategorized,
        // std `cvt(-1)` → raw os error 1 (kernel SYSERR has no distinct errno yet)
        1 => io::ErrorKind::Other,
        2 => io::ErrorKind::NotFound,
        9 => io::ErrorKind::InvalidInput, // EBADF
        12 => io::ErrorKind::OutOfMemory,
        13 => io::ErrorKind::PermissionDenied,
        17 => io::ErrorKind::AlreadyExists,
        20 => io::ErrorKind::NotADirectory,
        22 => io::ErrorKind::InvalidInput,
        38 => io::ErrorKind::Unsupported,
        _ => io::ErrorKind::Uncategorized,
    }
}

pub fn error_string(errno: i32) -> String {
    match errno {
        0 => "success".to_string(),
        1 => "syscall failed".to_string(),
        2 => "no such file or directory".to_string(),
        9 => "bad file descriptor".to_string(),
        12 => "out of memory".to_string(),
        13 => "permission denied".to_string(),
        17 => "file exists".to_string(),
        20 => "not a directory".to_string(),
        22 => "invalid argument".to_string(),
        38 => "function not implemented".to_string(),
        n => format!("os error {n}"),
    }
}
