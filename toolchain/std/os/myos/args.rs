//! Zero-allocation argv access for `#![no_main]` binaries on myos.

use crate::ffi::OsStr;
use crate::sys::args::{self, StaticArgs};

/// Borrow command-line arguments captured during PAL `init`.
#[unstable(feature = "myos_ext", issue = "none")]
#[inline]
pub fn args_os() -> StaticArgs {
    args::static_args()
}

/// Argument count captured during PAL `init`.
#[unstable(feature = "myos_ext", issue = "none")]
#[inline]
pub fn argc() -> usize {
    args::count()
}

/// Borrow `argv[i]` without allocating.
#[unstable(feature = "myos_ext", issue = "none")]
#[inline]
pub fn arg(i: usize) -> Option<&'static OsStr> {
    args::static_args().nth(i)
}

/// Basename from the last successful `exec` (`SYS_EXECNAME`), when argv is empty.
#[unstable(feature = "myos_ext", issue = "none")]
#[inline]
pub fn exec_basename() -> Option<&'static OsStr> {
    args::exec_basename()
}
