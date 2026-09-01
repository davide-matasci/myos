use crate::ffi::{OsStr, OsString};
use crate::fs::TryLockError;
use crate::io::{self, BorrowedCursor, ErrorKind, IoSlice, IoSliceMut, SeekFrom};
use crate::os::myos::ffi::OsStrExt;
use crate::os::myos::io::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use crate::path::{Path, PathBuf};
use crate::sys::fd::FileDesc;
use crate::sys::myos::abi;
use crate::sys::time::SystemTime;
use crate::sys::{cvt, unsupported, unsupported_err, AsInner, FromInner, IntoInner};
use crate::fmt;

#[path = "unsupported.rs"]
mod stub;
pub use stub::{
    Dir, DirBuilder, canonicalize, copy, exists, link, readlink, remove_dir_all, rename, rmdir,
    symlink, unlink,
};

#[derive(Debug)]
pub struct File(FileDesc);

#[derive(Clone)]
pub struct FileAttr {
    size: u64,
    is_dir: bool,
    is_file: bool,
    is_symlink: bool,
}

#[derive(Clone, Debug)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
}

#[derive(Copy, Clone, Debug, Default)]
pub struct FileTimes {}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FilePermissions {}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct FileType {
    is_file: bool,
}

pub struct ReadDir(!);
pub struct DirEntry(!);

pub fn readdir(_path: &Path) -> io::Result<ReadDir> {
    unsupported()
}

const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;

fn stat_path(path: &Path) -> io::Result<FileAttr> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() {
        return Err(io::const_error!(ErrorKind::InvalidInput, "empty path"));
    }
    let mut buf = abi::StatBuf { st_mode: 0, st_size: 0, st_ino: 0, st_nlink: 0 };
    cvt(abi::stat(bytes, &mut buf))?;
    let fmt = buf.st_mode & S_IFMT;
    Ok(FileAttr {
        size: buf.st_size as u64,
        is_dir: fmt == S_IFDIR,
        is_file: fmt == S_IFREG,
        is_symlink: fmt == S_IFLNK,
    })
}

pub fn stat(path: &Path) -> io::Result<FileAttr> {
    stat_path(path)
}

pub fn lstat(path: &Path) -> io::Result<FileAttr> {
    // myos has no distinct lstat yet; same as stat.
    stat_path(path)
}

pub fn set_perm(_path: &Path, _perm: FilePermissions) -> io::Result<()> {
    unsupported()
}

pub fn set_times(_path: &Path, _times: FileTimes) -> io::Result<()> {
    unsupported()
}

pub fn set_times_nofollow(_path: &Path, _times: FileTimes) -> io::Result<()> {
    unsupported()
}

impl FileAttr {
    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn perm(&self) -> FilePermissions {
        FilePermissions {}
    }

    pub fn file_type(&self) -> FileType {
        FileType { is_file: self.is_file && !self.is_dir && !self.is_symlink }
    }

    pub fn modified(&self) -> io::Result<SystemTime> {
        unsupported()
    }

    pub fn accessed(&self) -> io::Result<SystemTime> {
        unsupported()
    }

    pub fn created(&self) -> io::Result<SystemTime> {
        unsupported()
    }
}

impl FilePermissions {
    pub fn readonly(&self) -> bool {
        true
    }

    pub fn set_readonly(&mut self, _readonly: bool) {}
}

impl FileTimes {
    pub fn set_accessed(&mut self, _t: SystemTime) {}
    pub fn set_modified(&mut self, _t: SystemTime) {}
}

impl FileType {
    pub fn is_dir(&self) -> bool {
        !self.is_file
    }

    pub fn is_file(&self) -> bool {
        self.is_file
    }

    pub fn is_symlink(&self) -> bool {
        false
    }
}

impl OpenOptions {
    pub fn new() -> OpenOptions {
        OpenOptions {
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
        }
    }

    pub fn read(&mut self, read: bool) {
        self.read = read;
    }

    pub fn write(&mut self, write: bool) {
        self.write = write;
    }

    pub fn append(&mut self, append: bool) {
        self.append = append;
    }

    pub fn truncate(&mut self, truncate: bool) {
        self.truncate = truncate;
    }

    pub fn create(&mut self, create: bool) {
        self.create = create;
    }

    pub fn create_new(&mut self, create_new: bool) {
        self.create_new = create_new;
    }

    #[inline(never)]
    pub fn open(&mut self, path: &Path) -> io::Result<File> {
        open_path(path, self)
    }
}

/// Bootfs is read-only. Avoid reading all `OpenOptions` flags in one LLVM
/// frame (patched std + `File::open` blew the user stack and double-faulted).
#[inline(never)]
fn open_path(path: &Path, opts: &OpenOptions) -> io::Result<File> {
    if opts.write {
        return Err(io::const_error!(
            ErrorKind::Unsupported,
            "myos bootfs is read-only"
        ));
    }
    if opts.append {
        return Err(io::const_error!(
            ErrorKind::Unsupported,
            "myos bootfs is read-only"
        ));
    }
    if opts.truncate {
        return Err(io::const_error!(
            ErrorKind::Unsupported,
            "myos bootfs is read-only"
        ));
    }
    if opts.create {
        return Err(io::const_error!(
            ErrorKind::Unsupported,
            "myos bootfs is read-only"
        ));
    }
    if opts.create_new {
        return Err(io::const_error!(
            ErrorKind::Unsupported,
            "myos bootfs is read-only"
        ));
    }
    if !opts.read {
        return Err(io::const_error!(ErrorKind::InvalidInput, "read access required"));
    }
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() {
        return Err(io::const_error!(ErrorKind::InvalidInput, "empty path"));
    }
    let fd = cvt(abi::open(bytes))?;
    Ok(File(unsafe { FileDesc::from_raw_fd(fd as RawFd) }))
}

impl File {
    #[inline(never)]
    pub fn open(path: &Path, opts: &OpenOptions) -> io::Result<File> {
        open_path(path, opts)
    }

    pub fn file_attr(&self) -> io::Result<FileAttr> {
        // No fstat yet; unsupported keeps callers on the open/read path.
        unsupported()
    }

    pub fn fsync(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn datasync(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn lock(&self) -> io::Result<()> {
        unsupported()
    }

    pub fn lock_shared(&self) -> io::Result<()> {
        unsupported()
    }

    pub fn try_lock(&self) -> Result<(), TryLockError> {
        Err(TryLockError::Error(unsupported_err()))
    }

    pub fn try_lock_shared(&self) -> Result<(), TryLockError> {
        Err(TryLockError::Error(unsupported_err()))
    }

    pub fn unlock(&self) -> io::Result<()> {
        unsupported()
    }

    pub fn truncate(&self, _size: u64) -> io::Result<()> {
        unsupported()
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        self.0.read_vectored(bufs)
    }

    #[inline]
    pub fn is_read_vectored(&self) -> bool {
        self.0.is_read_vectored()
    }

    pub fn read_buf(&self, cursor: BorrowedCursor<'_, u8>) -> io::Result<()> {
        self.0.read_buf(cursor)
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.0.write_vectored(bufs)
    }

    #[inline]
    pub fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }

    #[inline]
    pub fn flush(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn seek(&self, _pos: SeekFrom) -> io::Result<u64> {
        unsupported()
    }

    pub fn size(&self) -> Option<io::Result<u64>> {
        None
    }

    pub fn tell(&self) -> io::Result<u64> {
        unsupported()
    }

    pub fn duplicate(&self) -> io::Result<File> {
        unsupported()
    }

    pub fn set_permissions(&self, _perm: FilePermissions) -> io::Result<()> {
        unsupported()
    }

    pub fn set_times(&self, _times: FileTimes) -> io::Result<()> {
        unsupported()
    }
}

impl AsInner<FileDesc> for File {
    #[inline]
    fn as_inner(&self) -> &FileDesc {
        &self.0
    }
}

impl IntoInner<FileDesc> for File {
    fn into_inner(self) -> FileDesc {
        self.0
    }
}

impl FromInner<FileDesc> for File {
    fn from_inner(file_desc: FileDesc) -> Self {
        Self(file_desc)
    }
}

impl AsFd for File {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl AsRawFd for File {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl FromRawFd for File {
    unsafe fn from_raw_fd(raw_fd: RawFd) -> Self {
        File(unsafe { FileDesc::from_raw_fd(raw_fd) })
    }
}

impl IntoRawFd for File {
    fn into_raw_fd(self) -> RawFd {
        self.0.into_raw_fd()
    }
}

impl fmt::Debug for ReadDir {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
    }
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<io::Result<DirEntry>> {
        self.0
    }
}

impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.0
    }

    pub fn file_name(&self) -> OsString {
        self.0
    }

    pub fn metadata(&self) -> io::Result<FileAttr> {
        self.0
    }

    pub fn file_type(&self) -> io::Result<FileType> {
        self.0
    }
}
