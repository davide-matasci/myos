//! Paths for myos.

use crate::ffi::{OsStr, OsString};
use crate::io;
use crate::marker::PhantomData;
use crate::path::{self, PathBuf};
use crate::os::myos::ffi::{OsStrExt, OsStringExt};
use crate::sys::args;
use crate::sys::myos::abi;
use crate::fmt;

pub fn getcwd() -> io::Result<PathBuf> {
    let mut buf = crate::vec![0u8; 257];
    let n = abi::getcwd(&mut buf);
    if n < 0 {
        // A cwd that has been removed.
        return Err(io::Error::from_raw_os_error(2));
    }
    buf.truncate(n as usize);
    Ok(PathBuf::from(OsString::from_vec(buf)))
}

pub fn chdir(p: &path::Path) -> io::Result<()> {
    if abi::chdirat(p.as_os_str().as_bytes()) < 0 {
        return Err(io::Error::from_raw_os_error(2));
    }
    Ok(())
}

pub struct SplitPaths<'a>(!, PhantomData<&'a ()>);

pub fn split_paths(_unparsed: &OsStr) -> SplitPaths<'_> {
    SplitPaths(unreachable!(), PhantomData)
}

impl<'a> Iterator for SplitPaths<'a> {
    type Item = PathBuf;
    fn next(&mut self) -> Option<PathBuf> {
        self.0
    }
}

#[derive(Debug)]
pub struct JoinPathsError;

pub fn join_paths<I, T>(_paths: I) -> Result<OsString, JoinPathsError>
where
    I: Iterator<Item = T>,
    T: AsRef<OsStr>,
{
    Err(JoinPathsError)
}

impl fmt::Display for JoinPathsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        "not supported on this platform yet".fmt(f)
    }
}

impl crate::error::Error for JoinPathsError {}

pub fn current_exe() -> io::Result<PathBuf> {
    let mut iter = args::static_args();
    let Some(arg0) = iter.next() else {
        return Err(io::const_error!(
            io::ErrorKind::Unsupported,
            "no argv[0] for current_exe"
        ));
    };
    Ok(PathBuf::from(arg0))
}

pub fn temp_dir() -> PathBuf {
    PathBuf::from("/tmp")
}

pub fn home_dir() -> Option<PathBuf> {
    None
}
