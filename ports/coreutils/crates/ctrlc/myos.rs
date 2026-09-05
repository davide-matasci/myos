//! myos has no signals; compile-time stub for ctrlc (used by uu_sort).
use crate::error::Error as CtrlcError;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    EEXIST,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::EEXIST => write!(f, "EEXIST"),
        }
    }
}

impl std::error::Error for Error {}

pub type Signal = i32;

pub unsafe fn init_os_handler(_overwrite: bool) -> Result<(), Error> {
    Ok(())
}

pub unsafe fn block_ctrl_c() -> Result<(), CtrlcError> {
    // No SIGINT delivery on myos; park so the ctrlc helper thread stays quiet.
    loop {
        std::thread::park();
    }
}
