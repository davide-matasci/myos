use crate::ffi::{OsStr, OsString};
use crate::fs::TryLockError;
use crate::io::{self, BorrowedCursor, ErrorKind, IoSlice, IoSliceMut, SeekFrom};
use crate::os::myos::ffi::OsStrExt;
use crate::os::myos::io::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use crate::path::{Path, PathBuf};
use crate::sys::fd::FileDesc;
use crate::sys::myos::abi;
use crate::sys::time::{SystemTime, UNIX_EPOCH};
use crate::time::Duration;
use crate::sys::{cvt, unsupported, AsInner, FromInner, IntoInner};
use crate::fmt;

#[path = "unsupported.rs"]
mod stub;
pub use stub::{canonicalize, link};
pub use crate::sys::fs::common::{Dir, copy, exists, remove_dir_all};

/// The errno values the calls below report. The kernel has one failure
/// value, so each call works out its cause the way libgloss does.
const ENOENT: i32 = 2;
const EEXIST: i32 = 17;
const ENOTDIR: i32 = 20;
const EISDIR: i32 = 21;
const EINVAL: i32 = 22;
const ESPIPE: i32 = 29;
const EROFS: i32 = 30;
const ENAMETOOLONG: i32 = 36;
const ENOTEMPTY: i32 = 39;

/// The longest path the kernel takes.
const PATH_MAX: usize = 256;

fn os_err<T>(code: i32) -> io::Result<T> {
    Err(io::Error::from_raw_os_error(code))
}

/// The kernel's stat of `path` (the last component not followed), if any.
fn kstat(bytes: &[u8]) -> Option<abi::StatBuf> {
    let mut buf = abi::StatBuf::default();
    (abi::statat(abi::AT_FDCWD, bytes, abi::AT_SYMLINK_NOFOLLOW, &mut buf) == 0).then_some(buf)
}

/// Why a call that needed `path` to exist failed: it does not, or it does
/// and the filesystem refused (read-only, or no such operation there).
fn missing_or(bytes: &[u8], code: i32) -> i32 {
    if kstat(bytes).is_some() { code } else { ENOENT }
}

fn path_bytes(path: &Path) -> io::Result<&[u8]> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() {
        return os_err(ENOENT);
    }
    if bytes.len() > PATH_MAX {
        return os_err(ENAMETOOLONG);
    }
    Ok(bytes)
}

#[derive(Debug)]
pub struct DirBuilder {}

impl DirBuilder {
    pub fn new() -> DirBuilder {
        DirBuilder {}
    }

    pub fn mkdir(&self, p: &Path) -> io::Result<()> {
        let bytes = path_bytes(p)?;
        if abi::mknodat(bytes, abi::MKNOD_DIR) == 0 {
            return Ok(());
        }
        // `create_dir_all` takes EEXIST for an existing directory.
        os_err(missing_or(bytes, EEXIST))
    }
}

pub fn unlink(p: &Path) -> io::Result<()> {
    let bytes = path_bytes(p)?;
    // A directory is rmdir's (Linux says EISDIR), whatever a filesystem's
    // unlink would do with it.
    match kstat(bytes) {
        None => return os_err(ENOENT),
        Some(st) if st.st_mode & S_IFMT == S_IFDIR => return os_err(EISDIR),
        Some(_) => {}
    }
    if abi::unlinkat(bytes, 0) == 0 { Ok(()) } else { os_err(EROFS) }
}

pub fn rmdir(p: &Path) -> io::Result<()> {
    let bytes = path_bytes(p)?;
    if abi::unlinkat(bytes, abi::AT_REMOVEDIR) == 0 {
        return Ok(());
    }
    match kstat(bytes) {
        None => os_err(ENOENT),
        Some(st) if st.st_mode & S_IFMT != S_IFDIR => os_err(ENOTDIR),
        Some(_) => os_err(ENOTEMPTY),
    }
}

pub fn rename(old: &Path, new: &Path) -> io::Result<()> {
    let (a, b) = (path_bytes(old)?, path_bytes(new)?);
    if abi::renameat(a, b) == 0 {
        return Ok(());
    }
    os_err(missing_or(a, EROFS))
}

pub fn symlink(original: &Path, link: &Path) -> io::Result<()> {
    let (a, b) = (path_bytes(original)?, path_bytes(link)?);
    if abi::symlinkat(a, b) == 0 {
        return Ok(());
    }
    os_err(if kstat(b).is_some() { EEXIST } else { EROFS })
}

pub fn readlink(p: &Path) -> io::Result<PathBuf> {
    let bytes = path_bytes(p)?;
    let mut buf = [0u8; 1024];
    let n = abi::readlinkat(bytes, &mut buf);
    if n < 0 {
        return os_err(missing_or(bytes, EINVAL));
    }
    Ok(PathBuf::from(OsStr::from_bytes(&buf[..n as usize])))
}

#[derive(Debug)]
pub struct File(FileDesc);

#[derive(Clone)]
pub struct FileAttr {
    size: u64,
    is_dir: bool,
    is_file: bool,
    is_symlink: bool,
    mode: u32,
    nlink: u32,
    ino: u32,
    dev: u32,
    /// Seconds since the epoch, 0 where the filesystem keeps none.
    atime: i64,
    mtime: i64,
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
pub struct FileTimes {
    accessed: Option<SystemTime>,
    modified: Option<SystemTime>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FilePermissions {}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct FileType {
    is_dir: bool,
    is_file: bool,
    is_symlink: bool,
}

pub struct ReadDir {
    root: PathBuf,
    buf: crate::vec::Vec<u8>,
    pos: usize,
}

pub struct DirEntry {
    root: PathBuf,
    file_name: OsString,
}

pub fn readdir(path: &Path) -> io::Result<ReadDir> {
    let bytes = path.as_os_str().as_bytes();
    // Empty path is cwd; kernel/libgloss treat "" like "/".
    let path_bytes = if bytes.is_empty() { b".".as_slice() } else { bytes };
    // The kernel fills the buffer when there may be more: grow it until the
    // names fit (up to its limit).
    let mut buf = crate::vec![0u8; 4096];
    loop {
        let n = cvt(abi::listdirat(path_bytes, &mut buf))? as usize;
        if n < buf.len() || buf.len() >= 256 * 1024 {
            buf.truncate(n);
            break;
        }
        buf = crate::vec![0u8; buf.len() * 2];
    }
    Ok(ReadDir { root: path.to_path_buf(), buf, pos: 0 })
}

const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;

/// `stat` of `path` relative to `dirfd`, with `statat`'s flags.
fn stat_at(dirfd: usize, bytes: &[u8], flags: usize) -> io::Result<FileAttr> {
    let mut buf = abi::StatBuf::default();
    if abi::statat(dirfd, bytes, flags, &mut buf) < 0 {
        // The kernel's one stat error: the path does not resolve (libgloss
        // says ENOENT too). uutils touch creates a file only on NotFound.
        return Err(io::Error::from_raw_os_error(2));
    }
    let fmt = buf.st_mode & S_IFMT;
    Ok(FileAttr {
        size: buf.st_size,
        is_dir: fmt == S_IFDIR,
        is_file: fmt == S_IFREG,
        is_symlink: fmt == S_IFLNK,
        mode: buf.st_mode,
        nlink: buf.st_nlink,
        ino: buf.st_ino,
        dev: buf.st_dev,
        atime: buf.st_atime,
        mtime: buf.st_mtime,
    })
}

fn stat_path(path: &Path, flags: usize) -> io::Result<FileAttr> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() {
        return Err(io::const_error!(ErrorKind::InvalidInput, "empty path"));
    }
    stat_at(abi::AT_FDCWD, bytes, flags)
}

pub fn stat(path: &Path) -> io::Result<FileAttr> {
    stat_path(path, 0)
}

pub fn lstat(path: &Path) -> io::Result<FileAttr> {
    stat_path(path, abi::AT_SYMLINK_NOFOLLOW)
}

pub fn set_perm(_path: &Path, _perm: FilePermissions) -> io::Result<()> {
    unsupported()
}

/// One `FileTimes` entry as the kernel's seconds; unset leaves it as it is.
fn kernel_time(t: Option<SystemTime>) -> io::Result<i64> {
    match t {
        None => Ok(abi::UTIME_OMIT),
        Some(t) => t
            .sub_time(&UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_secs()).ok())
            .ok_or(io::const_error!(ErrorKind::InvalidInput, "file time before the epoch")),
    }
}

fn kernel_times(times: FileTimes) -> io::Result<[i64; 2]> {
    Ok([kernel_time(times.accessed)?, kernel_time(times.modified)?])
}

pub fn set_times(path: &Path, times: FileTimes) -> io::Result<()> {
    cvt(abi::utimensat(abi::AT_FDCWD, path_bytes(path)?, &kernel_times(times)?, 0))?;
    Ok(())
}

pub fn set_times_nofollow(path: &Path, times: FileTimes) -> io::Result<()> {
    cvt(abi::utimensat(abi::AT_FDCWD, path_bytes(path)?, &kernel_times(times)?, abi::AT_SYMLINK_NOFOLLOW))?;
    Ok(())
}

fn system_time(secs: i64) -> SystemTime {
    UNIX_EPOCH.checked_add_duration(&Duration::from_secs(secs.max(0) as u64)).unwrap_or(UNIX_EPOCH)
}

impl FileAttr {
    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn perm(&self) -> FilePermissions {
        FilePermissions {}
    }

    pub fn file_type(&self) -> FileType {
        FileType {
            is_dir: self.is_dir,
            is_file: self.is_file && !self.is_dir && !self.is_symlink,
            is_symlink: self.is_symlink,
        }
    }

    pub fn modified(&self) -> io::Result<SystemTime> {
        Ok(system_time(self.mtime))
    }

    pub fn accessed(&self) -> io::Result<SystemTime> {
        Ok(system_time(self.atime))
    }

    pub fn mode(&self) -> u32 {
        self.mode
    }

    pub fn nlink(&self) -> u64 {
        u64::from(self.nlink.max(1))
    }

    pub fn ino(&self) -> u64 {
        u64::from(self.ino)
    }

    pub fn dev(&self) -> u64 {
        u64::from(self.dev)
    }

    pub fn atime(&self) -> i64 {
        self.atime
    }

    pub fn mtime(&self) -> i64 {
        self.mtime
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
    pub fn set_accessed(&mut self, t: SystemTime) {
        self.accessed = Some(t);
    }
    pub fn set_modified(&mut self, t: SystemTime) {
        self.modified = Some(t);
    }
}

impl FileType {
    pub fn is_dir(&self) -> bool {
        self.is_dir
    }

    pub fn is_file(&self) -> bool {
        self.is_file
    }

    pub fn is_symlink(&self) -> bool {
        self.is_symlink
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

/// The kernel's open flags for `opts` (the same checks as the unix std).
/// Kept out of [`open_path`]'s frame: reading every `OpenOptions` flag in one
/// LLVM frame once blew the user stack (patched std + `File::open`).
#[inline(never)]
fn open_flags(opts: &OpenOptions) -> io::Result<usize> {
    let writes = opts.write || opts.append;
    let access = match (opts.read, writes) {
        (true, false) => 0,
        (false, true) => abi::O_WRONLY,
        (true, true) => abi::O_RDWR,
        (false, false) => return os_err(EINVAL),
    };
    if !opts.write && (opts.truncate || opts.create || opts.create_new) && !opts.append {
        return os_err(EINVAL);
    }
    if opts.append && opts.truncate && !opts.create_new {
        return os_err(EINVAL);
    }
    // As on Unix, no fd std opens is inherited by a program it execs.
    let mut flags = access | abi::O_CLOEXEC;
    if opts.create || opts.create_new {
        flags |= abi::O_CREAT;
    }
    if opts.create_new {
        flags |= abi::O_EXCL;
    }
    if opts.truncate {
        flags |= abi::O_TRUNC;
    }
    if opts.append {
        flags |= abi::O_APPEND;
    }
    Ok(flags)
}

#[inline(never)]
fn open_path(path: &Path, opts: &OpenOptions) -> io::Result<File> {
    let flags = open_flags(opts)?;
    let bytes = path_bytes(path)?;
    let fd = abi::openat(abi::AT_FDCWD, bytes, flags);
    if fd == abi::SYSERR_EEXIST {
        return os_err(EEXIST);
    }
    if fd < 0 {
        return os_err(match kstat(bytes) {
            None => ENOENT,
            Some(st) if st.st_mode & S_IFMT == S_IFDIR => EISDIR,
            Some(_) => EROFS,
        });
    }
    Ok(File(unsafe { FileDesc::from_raw_fd(fd as RawFd) }))
}

impl File {
    #[inline(never)]
    pub fn open(path: &Path, opts: &OpenOptions) -> io::Result<File> {
        open_path(path, opts)
    }

    pub fn file_attr(&self) -> io::Result<FileAttr> {
        stat_at(self.0.as_raw_fd() as usize, b"", abi::AT_EMPTY_PATH)
    }

    pub fn fsync(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn datasync(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn lock(&self) -> io::Result<()> {
        self.flock(abi::LOCK_EX)
    }

    pub fn lock_shared(&self) -> io::Result<()> {
        self.flock(abi::LOCK_SH)
    }

    pub fn try_lock(&self) -> Result<(), TryLockError> {
        self.try_flock(abi::LOCK_EX)
    }

    pub fn try_lock_shared(&self) -> Result<(), TryLockError> {
        self.try_flock(abi::LOCK_SH)
    }

    pub fn unlock(&self) -> io::Result<()> {
        self.flock(abi::LOCK_UN)
    }

    fn flock(&self, op: usize) -> io::Result<()> {
        cvt(abi::flock(self.0.as_raw_fd(), op)).map(drop)
    }

    fn try_flock(&self, op: usize) -> Result<(), TryLockError> {
        match self.flock(op | abi::LOCK_NB) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Err(TryLockError::WouldBlock),
            Err(e) => Err(TryLockError::Error(e)),
        }
    }

    pub fn truncate(&self, size: u64) -> io::Result<()> {
        cvt(abi::ftruncate(self.0.as_raw_fd(), size)).map(drop)
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

    pub fn seek(&self, pos: SeekFrom) -> io::Result<u64> {
        let (offset, whence) = match pos {
            SeekFrom::Start(off) => (off as i64, 0),
            SeekFrom::Current(off) => (off, 1),
            SeekFrom::End(off) => (off, 2),
        };
        let ret = abi::lseek(self.0.as_raw_fd(), offset, whence);
        if ret < 0 { os_err(ESPIPE) } else { Ok(ret as u64) }
    }

    pub fn size(&self) -> Option<io::Result<u64>> {
        None
    }

    pub fn tell(&self) -> io::Result<u64> {
        self.seek(SeekFrom::Current(0))
    }

    pub fn duplicate(&self) -> io::Result<File> {
        unsupported()
    }

    pub fn set_permissions(&self, _perm: FilePermissions) -> io::Result<()> {
        unsupported()
    }

    pub fn set_times(&self, times: FileTimes) -> io::Result<()> {
        cvt(abi::utimensat(self.0.as_raw_fd() as usize, b"", &kernel_times(times)?, abi::AT_EMPTY_PATH))?;
        Ok(())
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
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.root, f)
    }
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<io::Result<DirEntry>> {
        while self.pos < self.buf.len() {
            let start = self.pos;
            while self.pos < self.buf.len() && self.buf[self.pos] != b'\n' {
                self.pos += 1;
            }
            let end = self.pos;
            if self.pos < self.buf.len() {
                self.pos += 1; // skip newline
            }
            if end == start {
                continue;
            }
            let name = &self.buf[start..end];
            // Skip only "." and ".."
            if name == b"." || name == b".." {
                continue;
            }
            return Some(Ok(DirEntry {
                root: self.root.clone(),
                file_name: OsStr::from_bytes(name).to_os_string(),
            }));
        }
        None
    }
}

impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.root.join(self.file_name.as_os_str())
    }

    pub fn file_name(&self) -> OsString {
        self.file_name.clone()
    }

    pub fn metadata(&self) -> io::Result<FileAttr> {
        stat_path(&self.path(), abi::AT_SYMLINK_NOFOLLOW)
    }

    pub fn file_type(&self) -> io::Result<FileType> {
        self.metadata().map(|m| m.file_type())
    }
}
