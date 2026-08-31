//! Environment for myos: empty for now (no env block from the loader).

pub use super::common::Env;

use crate::ffi::{OsStr, OsString};
use crate::io;
use crate::vec::Vec;

pub fn env() -> Env {
    Env::new(Vec::new())
}

pub fn getenv(_: &OsStr) -> Option<OsString> {
    None
}

pub unsafe fn setenv(_: &OsStr, _: &OsStr) -> io::Result<()> {
    Err(io::const_error!(
        io::ErrorKind::Unsupported,
        "cannot set env vars on this platform"
    ))
}

pub unsafe fn unsetenv(_: &OsStr) -> io::Result<()> {
    Err(io::const_error!(
        io::ErrorKind::Unsupported,
        "cannot unset env vars on this platform"
    ))
}
