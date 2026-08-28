use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut};
use crate::mem::ManuallyDrop;
use crate::os::fd::FromRawFd;
use crate::sys::fd::FileDesc;
use crate::sys::myos::abi::{EBADF, STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};

pub struct Stdin;
pub struct Stdout;
pub struct Stderr;

impl Stdin {
    pub const fn new() -> Stdin {
        Stdin
    }
}

impl io::Read for Stdin {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        unsafe { ManuallyDrop::new(FileDesc::from_raw_fd(STDIN_FILENO)).read(buf) }
    }

    fn read_buf(&mut self, buf: BorrowedCursor<'_, u8>) -> io::Result<()> {
        unsafe { ManuallyDrop::new(FileDesc::from_raw_fd(STDIN_FILENO)).read_buf(buf) }
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        unsafe { ManuallyDrop::new(FileDesc::from_raw_fd(STDIN_FILENO)).read_vectored(bufs) }
    }

    #[inline]
    fn is_read_vectored(&self) -> bool {
        true
    }
}

impl Stdout {
    pub const fn new() -> Stdout {
        Stdout
    }
}

impl io::Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        unsafe { ManuallyDrop::new(FileDesc::from_raw_fd(STDOUT_FILENO)).write(buf) }
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        unsafe { ManuallyDrop::new(FileDesc::from_raw_fd(STDOUT_FILENO)).write_vectored(bufs) }
    }

    #[inline]
    fn is_write_vectored(&self) -> bool {
        true
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Stderr {
    pub const fn new() -> Stderr {
        Stderr
    }
}

impl io::Write for Stderr {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        unsafe { ManuallyDrop::new(FileDesc::from_raw_fd(STDERR_FILENO)).write(buf) }
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        unsafe { ManuallyDrop::new(FileDesc::from_raw_fd(STDERR_FILENO)).write_vectored(bufs) }
    }

    #[inline]
    fn is_write_vectored(&self) -> bool {
        true
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub const STDIN_BUF_SIZE: usize = 0;

struct StackBuf {
    data: [u8; 256],
    len: usize,
}

impl core::fmt::Write for StackBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let take = bytes.len().min(self.data.len().saturating_sub(self.len));
        self.data[self.len..self.len + take].copy_from_slice(&bytes[..take]);
        self.len += take;
        Ok(())
    }
}

/// Format straight to the serial write syscall (no `stdout()` OnceLock).
pub fn print_args(args: core::fmt::Arguments<'_>) {
    let mut buf = StackBuf { data: [0; 256], len: 0 };
    let _ = core::fmt::write(&mut buf, args);
    if buf.len != 0 {
        let _ = crate::sys::myos::abi::write(STDOUT_FILENO, &buf.data[..buf.len]);
    }
}

pub fn is_ebadf(err: &io::Error) -> bool {
    err.raw_os_error() == Some(EBADF)
}

pub fn panic_output() -> Option<Vec<u8>> {
    None
}
