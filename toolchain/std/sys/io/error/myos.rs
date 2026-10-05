//! myos I/O error strings.
//!
//! The kernel returns one `SYSERR` (`usize::MAX`) for most failed syscalls;
//! std maps that to `-1` → raw os error `1`. Reads and writes tell EIO, ENXIO
//! and EINTR apart (`abi::io_result`), and the file calls work out an errno
//! of their own (`sys/fs/myos.rs`). Do **not** use the
//! upstream `generic` backend: its `error_string` always returns
//! `"operation successful"`, which hid real open/read failures (e.g. riscv64
//! `uutils cat` after findnest).

use crate::io;

pub fn errno() -> i32 {
    0
}

pub fn is_interrupted(code: i32) -> bool {
    code == 4 // EINTR
}

pub fn decode_error_kind(code: i32) -> io::ErrorKind {
    use io::ErrorKind::*;
    match code {
        0 => Uncategorized,
        // std `cvt(-1)` → raw os error 1: the kernel's one failure value,
        // where a call cannot tell more (`sys/fs/myos.rs` does for its calls)
        1 => Other,
        2 => NotFound,
        4 => Interrupted,
        5 => Other, // EIO
        6 => NotFound, // ENXIO: no such device (a FIFO without a reader)
        9 => InvalidInput, // EBADF
        11 => WouldBlock,
        12 => OutOfMemory,
        13 => PermissionDenied,
        17 => AlreadyExists,
        18 => CrossesDevices,
        20 => NotADirectory,
        21 => IsADirectory,
        22 => InvalidInput,
        28 => StorageFull,
        29 => NotSeekable,
        30 => ReadOnlyFilesystem,
        32 => BrokenPipe,
        36 => InvalidFilename,
        38 => Unsupported,
        39 => DirectoryNotEmpty,
        _ => Uncategorized,
    }
}

pub fn error_string(errno: i32) -> String {
    match errno {
        0 => "success",
        1 => "syscall failed",
        2 => "no such file or directory",
        4 => "interrupted system call",
        5 => "input/output error",
        6 => "no such device or address",
        9 => "bad file descriptor",
        11 => "resource temporarily unavailable",
        12 => "out of memory",
        13 => "permission denied",
        17 => "file exists",
        18 => "invalid cross-device link",
        20 => "not a directory",
        21 => "is a directory",
        22 => "invalid argument",
        28 => "no space left on device",
        29 => "illegal seek",
        30 => "read-only file system",
        32 => "broken pipe",
        36 => "file name too long",
        38 => "function not implemented",
        39 => "directory not empty",
        n => return format!("os error {n}"),
    }
    .to_string()
}
