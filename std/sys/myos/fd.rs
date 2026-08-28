#![unstable(reason = "not public", issue = "none", feature = "fd")]

use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut, Read, SeekFrom};
use crate::os::myos::io::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use crate::sys::{cvt, unsupported};
use crate::sys::myos::abi;

const IOV_MAX: usize = 1024;

#[derive(Debug)]
pub struct FileDesc {
    fd: OwnedFd,
}

impl FileDesc {
    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let result = cvt(abi::read(self.fd.as_raw_fd(), buf))?;
        Ok(result as usize)
    }

    pub fn read_buf(&self, mut buf: BorrowedCursor<'_, u8>) -> io::Result<()> {
        let slice = unsafe {
            core::slice::from_raw_parts_mut(buf.as_mut().as_mut_ptr() as *mut u8, buf.capacity())
        };
        let result = cvt(abi::read(self.fd.as_raw_fd(), slice))?;
        unsafe { buf.advance(result as usize) };
        Ok(())
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        let mut total = 0;
        for b in bufs.iter_mut().take(IOV_MAX) {
            total += self.read(b)?;
        }
        Ok(total)
    }

    #[inline]
    pub fn is_read_vectored(&self) -> bool {
        true
    }

    pub fn read_to_end(&self, buf: &mut Vec<u8>) -> io::Result<usize> {
        let mut me = self;
        (&mut me).read_to_end(buf)
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        let result = cvt(abi::write(self.fd.as_raw_fd(), buf))?;
        Ok(result as usize)
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let mut total = 0;
        for b in bufs.iter().take(IOV_MAX) {
            total += self.write(b)?;
        }
        Ok(total)
    }

    #[inline]
    pub fn is_write_vectored(&self) -> bool {
        true
    }

    pub fn seek(&self, _pos: SeekFrom) -> io::Result<u64> {
        unsupported()
    }

    pub fn tell(&self) -> io::Result<u64> {
        unsupported()
    }

    pub fn duplicate(&self) -> io::Result<FileDesc> {
        unsupported()
    }

    pub fn duplicate_path(&self, _path: &[u8]) -> io::Result<FileDesc> {
        unsupported()
    }

    pub fn nonblocking(&self) -> io::Result<bool> {
        Ok(false)
    }

    pub fn set_cloexec(&self) -> io::Result<()> {
        unsupported()
    }

    pub fn set_nonblocking(&self, _nonblocking: bool) -> io::Result<()> {
        unsupported()
    }
}

impl<'a> Read for &'a FileDesc {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (**self).read(buf)
    }
}

impl IntoInner<OwnedFd> for FileDesc {
    fn into_inner(self) -> OwnedFd {
        self.fd
    }
}

impl FromInner<OwnedFd> for FileDesc {
    fn from_inner(owned_fd: OwnedFd) -> Self {
        Self { fd: owned_fd }
    }
}

impl FromRawFd for FileDesc {
    unsafe fn from_raw_fd(raw_fd: RawFd) -> Self {
        let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        Self { fd }
    }
}

impl AsInner<OwnedFd> for FileDesc {
    #[inline]
    fn as_inner(&self) -> &OwnedFd {
        &self.fd
    }
}

impl AsFd for FileDesc {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl AsRawFd for FileDesc {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl IntoRawFd for FileDesc {
    fn into_raw_fd(self) -> RawFd {
        self.fd.into_raw_fd()
    }
}

use crate::sys::{AsInner, FromInner, IntoInner};
