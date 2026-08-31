//! myos hostname stub (no gethostname syscall yet).
use std::ffi::OsString;
use std::io;

pub fn get() -> io::Result<OsString> {
    Ok(OsString::from("myos"))
}

#[cfg(feature = "set")]
pub fn set(_hostname: &std::ffi::OsStr) -> io::Result<()> {
    Err(io::Error::from_raw_os_error(38)) // ENOSYS
}
