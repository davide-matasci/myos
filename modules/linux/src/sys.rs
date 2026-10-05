//! Linux syscall handlers, independent of the arch's syscall numbering.
//!
//! Each takes Linux arguments, calls the native implementation and returns
//! a Linux result (`-errno` on failure).

use alloc::string::String;
use alloc::vec::Vec;

use super::abi::*;
use super::files;
use super::net;
use crate::k::fs;
use crate::k::task;
use crate::k::user::{self, SyscallRegs};

const AT_FDCWD: usize = -100isize as usize;
const AT_EMPTY_PATH: usize = 0x1000;
const AT_REMOVEDIR: usize = 0x200;
pub const AT_SYMLINK_NOFOLLOW: usize = 0x100;
const MAX_PATH: usize = 256;
const ENAMETOOLONG: usize = 36;
const EISDIR: usize = 21;

pub(super) type R = Result<usize, usize>;
type R2<T> = Result<T, usize>;

/// Fold a handler's `Result` into the result register.
pub fn ret(r: R) -> usize {
    match r {
        Ok(v) => v,
        Err(e) => err(e),
    }
}

/// A native result that is one of its failure sentinels.
fn native_failed(r: usize) -> bool {
    r >= crate::k::signal::SYSERR_EINTR
}

pub(super) fn native(r: usize, generic: usize) -> R {
    let v = result(r, generic);
    if native_failed(r) { Err(v.wrapping_neg()) } else { Ok(v) }
}

pub(super) fn put(ptr: usize, bytes: &[u8]) -> Result<(), usize> {
    if ptr == 0 || !user::buffer_ok(ptr, bytes.len()) || !user::copy_to_user(ptr, bytes) {
        return Err(EFAULT);
    }
    Ok(())
}

pub(super) fn get_bytes(ptr: usize, out: &mut [u8]) -> Result<(), usize> {
    get(ptr, out)
}

fn get(ptr: usize, out: &mut [u8]) -> Result<(), usize> {
    if ptr == 0 || !user::copy_from_user(ptr, out) {
        return Err(EFAULT);
    }
    Ok(())
}

pub(super) fn get_u64(ptr: usize) -> Result<u64, usize> {
    let mut b = [0u8; 8];
    get(ptr, &mut b)?;
    Ok(u64::from_le_bytes(b))
}

/// A NUL-terminated string from user memory (at most `max` bytes).
fn user_cstr(ptr: usize, max: usize) -> Result<Vec<u8>, usize> {
    if ptr == 0 {
        return Err(EFAULT);
    }
    let mut out = Vec::new();
    let mut p = ptr;
    loop {
        let chunk = (user::PAGE - p % user::PAGE).min(64);
        let mut buf = [0u8; 64];
        get(p, &mut buf[..chunk])?;
        if let Some(i) = buf[..chunk].iter().position(|&b| b == 0) {
            out.extend_from_slice(&buf[..i]);
            return Ok(out);
        }
        out.extend_from_slice(&buf[..chunk]);
        if out.len() > max {
            return Err(ENAMETOOLONG);
        }
        p += chunk;
    }
}

/// A NULL-terminated array of strings (`argv` / `envp`), at most `max_n`,
/// each within the native exec limit on all strings together.
fn user_str_array(ptr: usize, max_n: usize) -> Result<Vec<Vec<u8>>, usize> {
    let mut out = Vec::new();
    if ptr == 0 {
        return Ok(out);
    }
    loop {
        let p = get_u64(ptr + out.len() * 8)? as usize;
        if p == 0 {
            return Ok(out);
        }
        if out.len() == max_n {
            return Err(E2BIG);
        }
        let s = user_cstr(p, user::MAX_EXEC_STRINGS).map_err(|e| if e == ENAMETOOLONG { E2BIG } else { e })?;
        out.push(s);
    }
}

fn join(dir: &str, name: &str) -> String {
    let mut s = String::from(dir);
    if !s.ends_with('/') {
        s.push('/');
    }
    s.push_str(name);
    s
}

/// The path a `*at` call names: absolute, cwd-relative, or relative to the
/// directory fd `dirfd`.
fn path_at(dirfd: usize, ptr: usize) -> R2<String> {
    let raw = user_cstr(ptr, MAX_PATH)?;
    let s = String::from_utf8(raw).map_err(|_| EINVAL)?;
    if s.is_empty() {
        return Err(ENOENT);
    }
    if s.starts_with('/') || dirfd == AT_FDCWD {
        return Ok(s);
    }
    match files::get(dirfd) {
        Some(d) if d.dir => Ok(join(&d.path, &s)),
        Some(_) => Err(ENOTDIR),
        None => Err(EBADF),
    }
}

/// The task's absolute view of `path` (what the fd table records).
fn view_path(path: &str) -> String {
    let mut b = [0u8; MAX_PATH];
    match fs::resolve_user_path_virtual(path, &mut b) {
        Some(n) => String::from(core::str::from_utf8(&b[..n]).unwrap_or(path)),
        None => String::from(path),
    }
}

/// The VFS path behind `path` (cwd, chroot and symlinks applied).
fn real_path(path: &str) -> R2<String> {
    user::resolve_copied_path(path).ok_or(ENOENT)
}

/// Like [`real_path`], but a symlink in the last component is not followed.
fn real_path_nofollow(path: &str) -> R2<String> {
    user::resolve_copied_path_nofollow(path).ok_or(ENOENT)
}

// ---- files ----------------------------------------------------------------

/// A non-blocking pipe end that would block (bits: readable 1, writable 2).
fn would_block(fd: usize, ready: u32) -> bool {
    files::nonblock(fd) && task::fd_poll_bits(fd).is_some_and(|b| b & ready == 0)
}

pub fn read(fd: usize, buf: usize, len: usize) -> R {
    let io = || native(user::sys_read(fd, buf, len), EBADF);
    if would_block(fd, 1) {
        return Err(EAGAIN);
    }
    if files::writer(fd).is_some() {
        return eventfd_read(fd, buf, len);
    }
    match files::get(fd) {
        Some(e) if e.dir => Err(EISDIR),
        Some(e) if e.sock.is_some() => net::recv(fd, false, io),
        _ => io(),
    }
}

pub fn write(fd: usize, buf: usize, len: usize) -> R {
    let io = || native(task::fd_write(fd, buf, len), EBADF);
    if let Some(w) = files::writer(fd) {
        return eventfd_write(w, buf, len);
    }
    if would_block(fd, 2) {
        return Err(EAGAIN);
    }
    if is_socket(fd) { net::send(fd, io) } else { io() }
}

fn is_socket(fd: usize) -> bool {
    files::get(fd).is_some_and(|e| e.sock.is_some())
}

/// readv / writev: one native call per `iovec`, stopping at a short one.
pub fn rw_vec(fd: usize, iov: usize, cnt: usize, write: bool) -> R {
    if cnt > 1024 {
        return Err(EINVAL);
    }
    let io = || {
        let mut total = 0usize;
        for i in 0..cnt {
            let base = get_u64(iov + i * 16)? as usize;
            let len = get_u64(iov + i * 16 + 8)? as usize;
            if len == 0 {
                continue;
            }
            let r = if write { task::fd_write(fd, base, len) } else { user::sys_read(fd, base, len) };
            if native_failed(r) {
                return if total > 0 { Ok(total) } else { native(r, EBADF) };
            }
            total += r;
            if r < len {
                break;
            }
        }
        Ok(total)
    };
    match (is_socket(fd), write) {
        (true, true) => net::send(fd, io),
        (true, false) => net::recv(fd, false, io),
        (false, _) => io(),
    }
}

pub fn openat(dirfd: usize, path: usize, flags: usize) -> R {
    const O_ACCMODE: usize = 3;
    const O_CREAT: usize = 0o100;
    const O_EXCL: usize = 0o200;
    const O_LARGEFILE: usize = 0o100000;
    const O_DIRECTORY: usize = 0o200000;
    const O_NOFOLLOW: usize = 0o400000;
    const O_CLOEXEC: usize = 0o2000000;
    const O_NONBLOCK: usize = 0o4000;
    let p = pty_alias(path_at(dirfd, path)?);
    let real = real_path(&p)?;
    let st = fs::stat(&real);
    if st.as_ref().is_some_and(|s| s.mode & fs::S_IFMT == 0o040000) {
        if flags & O_ACCMODE != 0 {
            return Err(EISDIR);
        }
        // The native fd is the directory itself (opened read-only);
        // getdents64 is served from the path table.
        let fd = native(user::open_path(&p, 0), EMFILE)?;
        files::set(fd, view_path(&p), true);
        files::set_cloexec(fd, flags & O_CLOEXEC != 0);
        return Ok(fd);
    }
    if flags & O_DIRECTORY != 0 {
        return Err(if st.is_some() { ENOTDIR } else { ENOENT });
    }
    if flags & O_CREAT != 0 && flags & O_EXCL != 0 && st.is_some() {
        return Err(EEXIST);
    }
    if flags & O_CREAT == 0 && st.is_none() {
        return Err(ENOENT);
    }
    let native_flags = flags & !(O_EXCL | O_LARGEFILE | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    let fd = native(user::open_path(&p, native_flags), ENOENT)?;
    files::set(fd, view_path(&p), false);
    files::set_cloexec(fd, flags & O_CLOEXEC != 0);
    files::set_nonblock(fd, flags & O_NONBLOCK != 0);
    Ok(fd)
}

/// The Linux names of the ptys, for musl's `openpty` and `ptsname`:
/// `/dev/ptmx` is `/dev/pts/clone`, `/dev/pts/N` the pair's `data`
/// (docs/tty.md).
fn pty_alias(path: String) -> String {
    if path == "/dev/ptmx" {
        return String::from("/dev/pts/clone");
    }
    match path.strip_prefix("/dev/pts/") {
        Some(n) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => {
            alloc::format!("{path}/data")
        }
        _ => path,
    }
}

pub fn close(fd: usize) -> R {
    if let Some(w) = files::writer(fd) {
        files::set_writer(fd, None);
        close(w).ok();
    }
    files::remove(fd);
    if task::fd_close(fd) { Ok(0) } else { Err(EBADF) }
}

/// `ftruncate(fd, len)`. The VFS cuts a file only to nothing (an `O_TRUNC`
/// open of it): a longer length is a zero written at its end, a shorter
/// non-zero one is refused.
pub fn ftruncate(fd: usize, len: usize) -> R {
    const O_WRONLY: usize = 1;
    const O_TRUNC: usize = 0o1000;
    let path = files::get(fd).filter(|e| !e.dir && e.sock.is_none()).ok_or(EINVAL)?.path;
    let real = real_path(&path)?;
    let size = fs::stat(&real).ok_or(EBADF)?.size as usize;
    if len == 0 && size != 0 {
        let t = native(user::open_path(&path, O_WRONLY | O_TRUNC), EIO)?;
        task::fd_close(t);
    } else if len > size {
        write_at(&real, len - 1, &[0])?;
    } else if len != 0 && len < size {
        return Err(EINVAL);
    }
    Ok(0)
}

pub fn truncate(path: usize, len: usize) -> R {
    const O_WRONLY: usize = 1;
    let fd = openat(AT_FDCWD, path, O_WRONLY)?;
    let r = ftruncate(fd, len);
    close(fd).ok();
    r
}

/// `madvise`: `MADV_DONTNEED` drops the pages (they read as zero, or as
/// the file, next time: allocators count on it); other advice is ignored.
pub fn madvise(addr: usize, len: usize, advice: usize) -> R {
    const MADV_DONTNEED: usize = 4;
    if advice == MADV_DONTNEED && !task::mmap_discard(addr, len) {
        return Err(EINVAL);
    }
    Ok(0)
}

/// `fsync`, `fdatasync`, `fchmod`, `fchown`: done once `fd` is valid.
/// myos keeps no owners or permission bits, and ext2 writes a file's
/// cached blocks back when its last fd closes.
pub fn fd_noop(fd: usize) -> R {
    task::fd_kind(fd).map(|_| 0).ok_or(EBADF)
}

/// `chmod`, `chown` and their `at` forms: done once the file exists.
pub fn path_noop(dirfd: usize, path: usize) -> R {
    let real = real_path(&path_at(dirfd, path)?)?;
    fs::stat(&real).map(|_| 0).ok_or(ENOENT)
}

fn put_stat(buf: usize, st: &fs::StatInfo) -> R {
    let b = super::arch::stat_bytes(st.mode, st.size as u64, st.ino as u64, st.nlink as u64, st.dev as u64, st.atime, st.mtime);
    put(buf, &b)?;
    Ok(0)
}

pub fn fstatat(dirfd: usize, path: usize, buf: usize, flags: usize) -> R {
    if flags & AT_EMPTY_PATH != 0 && user_cstr(path, MAX_PATH).is_ok_and(|p| p.is_empty()) {
        return fstat(dirfd, buf);
    }
    let p = path_at(dirfd, path)?;
    let real = if flags & AT_SYMLINK_NOFOLLOW != 0 { real_path_nofollow(&p)? } else { real_path(&p)? };
    let st = fs::stat(&real).ok_or(ENOENT)?;
    put_stat(buf, &st)
}

pub fn fstat(fd: usize, buf: usize) -> R {
    if is_socket(fd) {
        put(buf, &super::arch::stat_bytes(0o140777, 0, fd as u64 + 1, 1, 0, 0, 0))?;
        return Ok(0);
    }
    if let Some(e) = files::get(fd) {
        if let Some(st) = fs::stat(&real_path(&e.path)?) {
            return put_stat(buf, &st);
        }
    }
    let (mode, size) = match task::fd_kind(fd).ok_or(EBADF)? {
        task::FdKind::Tty => (0o020620, 0),
        task::FdKind::Pipe => (0o010600, 0),
        task::FdKind::File { size } => (0o100644, size as u64),
    };
    put(buf, &super::arch::stat_bytes(mode, size, fd as u64 + 1, 1, 0, 0, 0))?;
    Ok(0)
}

/// `utimensat`'s `tv_nsec` values: now, or leave the time as it is.
const UTIME_NOW: u64 = (1 << 30) - 1;
const UTIME_OMIT: u64 = (1 << 30) - 2;

/// The two times at user `ptr` (two `{seconds, sub}` pairs, the second word
/// nanoseconds or microseconds; `nsec` says which encodes
/// `UTIME_NOW`/`UTIME_OMIT`), as seconds or `MYOS_TIME_OMIT`; null: now.
fn user_times(ptr: usize, nsec: bool) -> R2<(u64, u64)> {
    let now = now_us() / 1_000_000;
    if ptr == 0 {
        return Ok((now, now));
    }
    let mut b = [0u8; 32];
    get(ptr, &mut b)?;
    let word = |i: usize| u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap());
    let one = |sec: u64, sub: u64| match sub {
        UTIME_NOW if nsec => Ok(now),
        UTIME_OMIT if nsec => Ok(myos_abi::MYOS_TIME_OMIT),
        _ if (sec as i64) < 0 => Err(EINVAL),
        _ => Ok(sec),
    };
    Ok((one(word(0), word(1))?, one(word(2), word(3))?))
}

/// `utimensat`; a null `path` is `futimens(dirfd)`.
pub fn utimensat(dirfd: usize, path: usize, times: usize, flags: usize) -> R {
    let real = if path == 0 {
        real_path(&files::get(dirfd).ok_or(EBADF)?.path)?
    } else {
        let p = path_at(dirfd, path)?;
        if flags & AT_SYMLINK_NOFOLLOW != 0 { real_path_nofollow(&p)? } else { real_path(&p)? }
    };
    let (atime, mtime) = user_times(times, true)?;
    fs::stat(&real).ok_or(ENOENT)?;
    if fs::set_times(&real, atime, mtime) { Ok(0) } else { Err(EROFS) }
}

/// `utimes` (x86_64): `struct timeval` times.
pub fn utimes(path: usize, times: usize) -> R {
    let real = real_path(&path_at(AT_FDCWD, path)?)?;
    let (atime, mtime) = user_times(times, false)?;
    fs::stat(&real).ok_or(ENOENT)?;
    if fs::set_times(&real, atime, mtime) { Ok(0) } else { Err(EROFS) }
}

pub fn lseek(fd: usize, off: usize, whence: usize) -> R {
    if files::get(fd).is_some_and(|e| e.dir) {
        if off == 0 && whence == 0 {
            files::set_pos(fd, 0);
            return Ok(0);
        }
        return Err(EINVAL);
    }
    native(user::sys_lseek(fd, off, whence), ESPIPE)
}

pub fn getdents64(fd: usize, buf: usize, count: usize) -> R {
    let e = match files::get(fd) {
        Some(e) if e.dir => e,
        Some(_) => return Err(ENOTDIR),
        None => return Err(if task::fd_kind(fd).is_some() { ENOTDIR } else { EBADF }),
    };
    let real = real_path(&e.path)?;
    let mut list = alloc::vec![0u8; 4096];
    let n = fs::listdir(&real, &mut list);
    let names = [&b"."[..], &b".."[..]]
        .into_iter()
        .chain(list[..n].split(|&b| b == b'\n').filter(|s| !s.is_empty()));
    let mut out = Vec::new();
    let mut pos = e.pos;
    for (i, name) in names.enumerate().skip(e.pos) {
        let ty = if i < 2 {
            DT_DIR
        } else {
            let child = join(&real, core::str::from_utf8(name).unwrap_or(""));
            fs::stat(&child).map_or(DT_UNKNOWN, |s| dtype(s.mode))
        };
        if !push_dirent(&mut out, count, i as u64 + 1, i as u64 + 1, ty, name) {
            if out.is_empty() {
                return Err(EINVAL);
            }
            break;
        }
        pos = i + 1;
    }
    files::set_pos(fd, pos);
    if !out.is_empty() {
        put(buf, &out)?;
    }
    Ok(out.len())
}

/// `FIONBIO`, `FIOCLEX` / `FIONCLEX`; the terminal requests from the
/// terminal's ctl text (`tty`, docs/tty.md). myos has no ioctl of its own:
/// anything else is `ENOTTY`.
pub fn ioctl(fd: usize, req: usize, arg: usize) -> R {
    use super::tty;
    const FIONBIO: usize = 0x5421;
    const FIONCLEX: usize = 0x5450;
    const FIOCLEX: usize = 0x5451;
    match req {
        FIONBIO => {
            let mut on = [0u8; 4];
            get(arg, &mut on)?;
            set_nonblock(fd, on != [0; 4]);
            Ok(0)
        }
        FIOCLEX | FIONCLEX => {
            files::set_cloexec(fd, req == FIOCLEX);
            Ok(0)
        }
        tty::TCGETS => put(arg, &tty::termios(fd)?).map(|_| 0),
        tty::TCSETS | tty::TCSETSW | tty::TCSETSF => {
            let mut t = [0u8; tty::TERMIOS_LEN];
            get(arg, &mut t)?;
            tty::set_termios(fd, &t)
        }
        tty::TCFLSH => tty::flush(fd, arg),
        tty::TIOCSCTTY => tty::set_ctty(fd),
        tty::TIOCGWINSZ => put(arg, &tty::winsize(fd)?).map(|_| 0),
        tty::TIOCSWINSZ => {
            let mut w = [0u8; 8];
            get(arg, &mut w)?;
            tty::set_winsize(fd, &w)
        }
        tty::TIOCGPTN => put(arg, &tty::pty_index(fd)?.to_ne_bytes()).map(|_| 0),
        tty::TIOCSPTLCK => tty::pty_index(fd).map(|_| 0),
        _ => Err(ENOTTY),
    }
}

pub fn faccessat(dirfd: usize, path: usize) -> R {
    let real = real_path(&path_at(dirfd, path)?)?;
    fs::stat(&real).map(|_| 0).ok_or(ENOENT)
}

pub fn pipe2(fds: usize, flags: usize) -> R {
    let (r, w) = task::pipe_open().ok_or(EMFILE)?;
    for fd in [r, w] {
        files::remove(fd);
        files::set_cloexec(fd, flags & O_CLOEXEC != 0);
        files::set_nonblock(fd, flags & O_NONBLOCK != 0);
    }
    let mut b = [0u8; 8];
    b[..4].copy_from_slice(&(r as i32).to_le_bytes());
    b[4..].copy_from_slice(&(w as i32).to_le_bytes());
    put(fds, &b)?;
    Ok(0)
}

/// `eventfd2(initval, flags)` as a native pipe: the eventfd is the read
/// end, the write end is kept aside (`files::writer`). A write of a
/// non-zero count puts a byte in the pipe; a read takes what is there and
/// gives the number of bytes as the count. Enough for a wakeup (libcurl's),
/// not for exact counts: no semaphore mode, no initial value.
pub fn eventfd2(init: usize, flags: usize) -> R {
    const EFD_SEMAPHORE: usize = 1;
    if init != 0 || flags & EFD_SEMAPHORE != 0 {
        return Err(EINVAL);
    }
    let (r, w) = task::pipe_open().ok_or(EMFILE)?;
    for fd in [r, w] {
        files::remove(fd);
        files::set_cloexec(fd, flags & O_CLOEXEC != 0);
    }
    files::set_nonblock(r, flags & O_NONBLOCK != 0);
    files::set_writer(r, Some(w));
    Ok(r)
}

fn eventfd_read(fd: usize, buf: usize, len: usize) -> R {
    if len < 8 {
        return Err(EINVAL);
    }
    // The bytes land in the caller's buffer, then their number replaces them.
    let n = native(user::sys_read(fd, buf, 8), EBADF)?;
    put(buf, &(n as u64).to_le_bytes())?;
    Ok(8)
}

fn eventfd_write(writer: usize, buf: usize, len: usize) -> R {
    if len < 8 {
        return Err(EINVAL);
    }
    let mut v = [0u8; 8];
    get(buf, &mut v)?;
    if u64::from_le_bytes(v) != 0 {
        // Any one byte of the caller's buffer will do.
        native(task::fd_write(writer, buf, 1), EBADF)?;
    }
    Ok(8)
}

pub fn dup(fd: usize, min: usize) -> R {
    let new = task::fd_dup_min(fd, min).ok_or(EBADF)?;
    files::dup(fd, new);
    Ok(new)
}

pub fn dup3(old: usize, new: usize, same_ok: bool, flags: usize) -> R {
    if old == new {
        return if !same_ok {
            Err(EINVAL)
        } else if task::fd_kind(old).is_some() {
            Ok(new)
        } else {
            Err(EBADF)
        };
    }
    if !task::fd_dup2(old, new) {
        return Err(EBADF);
    }
    files::dup(old, new);
    files::set_cloexec(new, flags & O_CLOEXEC != 0);
    Ok(new)
}

pub fn fcntl(fd: usize, cmd: usize, arg: usize) -> R {
    const F_DUPFD: usize = 0;
    const F_GETFD: usize = 1;
    const F_SETFD: usize = 2;
    const F_GETFL: usize = 3;
    const F_SETFL: usize = 4;
    const F_GETLK: usize = 5;
    const F_SETLK: usize = 6;
    const F_SETLKW: usize = 7;
    const F_OFD_GETLK: usize = 36;
    const F_OFD_SETLK: usize = 37;
    const F_OFD_SETLKW: usize = 38;
    const F_DUPFD_CLOEXEC: usize = 1030;
    if task::fd_kind(fd).is_none() {
        return Err(EBADF);
    }
    const FD_CLOEXEC: usize = 1;
    match cmd {
        F_DUPFD => dup(fd, arg),
        F_DUPFD_CLOEXEC => {
            let new = dup(fd, arg)?;
            files::set_cloexec(new, true);
            Ok(new)
        }
        F_GETFD => Ok(if files::cloexec(fd) { FD_CLOEXEC } else { 0 }),
        F_SETFD => {
            files::set_cloexec(fd, arg & FD_CLOEXEC != 0);
            Ok(0)
        }
        F_SETFL => {
            set_nonblock(fd, arg & O_NONBLOCK != 0);
            Ok(0)
        }
        F_GETFL => {
            let sock = files::get(fd).and_then(|e| e.sock).is_some_and(|s| s.nonblock);
            Ok(2 | if sock || files::nonblock(fd) { O_NONBLOCK } else { 0 }) // O_RDWR
        }
        // Record locks are granted and not kept (see `flock`): F_GETLK
        // always finds the range free (`l_type` = F_UNLCK).
        F_GETLK | F_OFD_GETLK => {
            put(arg, &2i16.to_le_bytes())?;
            Ok(0)
        }
        F_SETLK | F_SETLKW | F_OFD_SETLK | F_OFD_SETLKW => Ok(0),
        _ => Err(EINVAL),
    }
}

/// `O_NONBLOCK` on `fd`: a socket's state, or the flag `read` / `write`
/// honour on a pipe.
fn set_nonblock(fd: usize, on: bool) {
    if !net::set_nonblock(fd, on) {
        files::set_nonblock(fd, on);
    }
}

pub fn getcwd(buf: usize, size: usize) -> R {
    let r = user::sys_getcwd(buf, size);
    if native_failed(r) { Err(ERANGE) } else { Ok(r + 1) }
}

pub fn chdir(path: usize) -> R {
    native(user::chdir_path(&path_at(AT_FDCWD, path)?), ENOENT)
}

pub fn fchdir(fd: usize) -> R {
    match files::get(fd) {
        Some(e) if e.dir => native(user::chdir_path(&e.path), ENOENT),
        Some(_) => Err(ENOTDIR),
        None => Err(EBADF),
    }
}

pub fn mkdirat(dirfd: usize, path: usize) -> R {
    let real = real_path_nofollow(&path_at(dirfd, path)?)?;
    if fs::stat(&real).is_some() {
        return Err(EEXIST);
    }
    if fs::mkdir(&real) { Ok(0) } else { Err(ENOENT) }
}

pub fn unlinkat(dirfd: usize, path: usize, flags: usize) -> R {
    let real = real_path_nofollow(&path_at(dirfd, path)?)?;
    let st = fs::stat(&real).ok_or(ENOENT)?;
    let is_dir = st.mode & fs::S_IFMT == 0o040000;
    if flags & AT_REMOVEDIR != 0 {
        if !is_dir {
            return Err(ENOTDIR);
        }
        return if fs::rmdir(&real) { Ok(0) } else { Err(EINVAL) };
    }
    if is_dir {
        return Err(EISDIR);
    }
    if fs::unlink(&real) { Ok(0) } else { Err(EPERM) }
}

pub fn renameat(olddir: usize, old: usize, newdir: usize, new: usize) -> R {
    let old = real_path_nofollow(&path_at(olddir, old)?)?;
    let new = real_path_nofollow(&path_at(newdir, new)?)?;
    if fs::rename(&old, &new) { Ok(0) } else { Err(ENOENT) }
}

pub fn symlinkat(target: usize, newdir: usize, link: usize) -> R {
    let target = String::from_utf8(user_cstr(target, MAX_PATH)?).map_err(|_| EINVAL)?;
    let link = real_path_nofollow(&path_at(newdir, link)?)?;
    if fs::symlink(&target, &link) { Ok(0) } else { Err(EEXIST) }
}

pub fn readlinkat(dirfd: usize, path: usize, buf: usize, size: usize) -> R {
    let real = real_path_nofollow(&path_at(dirfd, path)?)?;
    let mut tmp = [0u8; MAX_PATH];
    let cap = size.min(tmp.len());
    let n = fs::readlink(&real, &mut tmp[..cap]).ok_or(EINVAL)?;
    put(buf, &tmp[..n])?;
    Ok(n)
}

// ---- memory ---------------------------------------------------------------

/// Anonymous or file-backed (a private copy of the file: `MAP_SHARED` file
/// mappings are refused, as the native layer cannot write them back).
pub fn mmap(addr: usize, len: usize, prot: usize, flags: usize, fd: usize, off: usize) -> R {
    const MAP_SHARED: usize = 0x01;
    const MAP_ANONYMOUS: usize = 0x20;
    if flags & MAP_SHARED != 0 && flags & MAP_ANONYMOUS == 0 {
        return Err(ENODEV);
    }
    if flags & MAP_ANONYMOUS == 0 && task::fd_kind(fd).is_none() {
        return Err(EBADF);
    }
    native(user::do_mmap(addr, len, prot, flags, fd as isize, off), ENOMEM)
}

/// `pread64(fd, buf, count, offset)`: a read at `offset` that leaves the
/// file position alone.
/// `flock`: granted and not kept. myos has no file locks; a lock taken by
/// cargo, SQLite or git against another copy of itself is all this skips.
pub fn flock(fd: usize) -> R {
    if task::fd_kind(fd).is_none() {
        Err(EBADF)
    } else {
        Ok(0)
    }
}

pub fn pread(fd: usize, buf: usize, count: usize, off: usize) -> R {
    let mut tmp = alloc::vec![0u8; count.min(1 << 20)];
    let n = task::fd_pread(fd, off, &mut tmp).ok_or(ESPIPE)?;
    put(buf, &tmp[..n])?;
    Ok(n)
}

/// Write `data` at `off` of the file at VFS path `real`. The filesystems
/// write no further than the end of a file: a gap before `off` is filled
/// with zeros first, which is what reading the hole would give.
fn write_at(real: &str, off: usize, data: &[u8]) -> R {
    let zeros = [0u8; 4096];
    let mut end = fs::stat(real).ok_or(EBADF)?.size as usize;
    while end < off {
        let n = (off - end).min(zeros.len());
        end += fs::write(real, end, &zeros[..n]).filter(|&w| w > 0).ok_or(EIO)?;
    }
    fs::write(real, off, data).ok_or(EIO)
}

/// `pwrite64`: the VFS writes at a position by path (the fd's offset stays).
pub fn pwrite(fd: usize, buf: usize, count: usize, off: usize) -> R {
    let path = files::get(fd).filter(|e| !e.dir && e.sock.is_none()).ok_or(ESPIPE)?.path;
    let real = real_path(&path)?;
    let mut tmp = alloc::vec![0u8; count.min(1 << 20)];
    get(buf, &mut tmp)?;
    write_at(&real, off, &tmp)
}

/// `pwritev` / `pwritev2` (flags ignored): one `pwrite` per buffer.
pub fn pwritev(fd: usize, iov: usize, cnt: usize, off: usize) -> R {
    if cnt > 1024 {
        return Err(EINVAL);
    }
    let mut total = 0;
    for i in 0..cnt {
        let base = get_u64(iov + i * 16)? as usize;
        let len = get_u64(iov + i * 16 + 8)? as usize;
        let n = pwrite(fd, base, len, off + total)?;
        total += n;
        if n < len {
            break;
        }
    }
    Ok(total)
}

// ---- processes ------------------------------------------------------------

pub fn fork(regs: &SyscallRegs) -> R {
    native(user::sys_fork(regs), EAGAIN)
}

pub fn execve(path: usize, argv: usize, envp: usize) -> R {
    const ENOEXEC: usize = 8;
    let mut p = path_at(AT_FDCWD, path)?;
    let mut args = user_str_array(argv, user::MAX_ARGC)?;
    let env = user_str_array(envp, user::MAX_ENVC)?;
    let real = real_path(&p)?;
    if let Some((interp, arg)) = script_interpreter(&real)? {
        // `#!interp [arg]`: run the interpreter on the script, as Linux does.
        let mut v = Vec::from([interp.clone().into_bytes()]);
        v.extend(arg.map(String::into_bytes));
        v.push(p.into_bytes());
        v.extend(args.into_iter().skip(1));
        (p, args) = (interp, v);
    }
    // The native exec limit on the strings' total size (NULs included).
    if args.iter().chain(&env).map(|s| s.len() + 1).sum::<usize>() > user::MAX_EXEC_STRINGS {
        return Err(E2BIG);
    }
    let arg_refs: Vec<&[u8]> = args.iter().map(|s| s.as_slice()).collect();
    let env_refs: Vec<&[u8]> = env.iter().map(|s| s.as_slice()).collect();
    // A file that is there but did not run is not an executable.
    let missing = if fs::stat(&real).is_some() { ENOEXEC } else { ENOENT };
    native(user::exec_linux(&p, &arg_refs, &env_refs), missing)
}

/// The interpreter of a `#!` script at VFS path `real`, and its optional
/// argument (the rest of the line), if the file is one.
fn script_interpreter(real: &str) -> R2<Option<(String, Option<String>)>> {
    const ENOEXEC: usize = 8;
    let mut head = [0u8; 256];
    let n = fs::read(real, 0, &mut head).unwrap_or(0);
    let Some(line) = head[..n].strip_prefix(b"#!") else {
        return Ok(None);
    };
    let end = line.iter().position(|&b| b == b'\n').ok_or(ENOEXEC)?;
    let line = core::str::from_utf8(&line[..end]).map_err(|_| ENOEXEC)?.trim();
    let (interp, arg) = match line.split_once([' ', '\t']) {
        Some((i, a)) => (i, Some(a.trim()).filter(|a| !a.is_empty())),
        None => (line, None),
    };
    if interp.is_empty() {
        return Err(ENOEXEC);
    }
    Ok(Some((String::from(interp), arg.map(String::from))))
}

pub fn wait4(pid: usize, status: usize, options: usize, rusage: usize) -> R {
    const WNOHANG: usize = 1;
    let r = user::sys_waitpid(status, options & WNOHANG, pid);
    let child = native(r, ECHILD)?;
    if child != 0 && status != 0 {
        // A signal death reports the native signal number; translate it.
        let mut b = [0u8; 4];
        get(status, &mut b)?;
        let mut st = u32::from_le_bytes(b);
        if st & 0x7f != 0 {
            st = (st & !0x7f) | sig_to_linux(st & 0x7f) as u32;
            put(status, &st.to_le_bytes())?;
        }
    }
    if rusage != 0 {
        put(rusage, &[0u8; 144])?;
    }
    Ok(child)
}

pub fn kill(pid: usize, sig: usize) -> R {
    if sig == 0 {
        let pid = pid as isize;
        return if pid <= 0 || task::is_live_user(pid as usize) { Ok(0) } else { Err(ESRCH) };
    }
    let n = sig_from_linux(sig);
    if n == 0 {
        return Err(EINVAL);
    }
    if crate::k::signal::kill(pid as isize, n) { Ok(0) } else { Err(ESRCH) }
}

/// `sched_getaffinity`: every online CPU (`processor_count` in
/// `/proc/cpuinfo`); a task runs where the core puts it. Rust's
/// `available_parallelism` and musl's `sysconf(_SC_NPROCESSORS_ONLN)` count
/// these bits: without them cargo builds one job at a time. The size of
/// the mask written is the result, as on Linux.
pub fn sched_getaffinity(len: usize, mask: usize) -> R {
    let mut b = [0u8; 256];
    let n = fs::read("/proc/cpuinfo", 0, &mut b).unwrap_or(0);
    let cpus = core::str::from_utf8(&b[..n])
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("processor_count: ")))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 64);
    let size = cpus.div_ceil(64) * 8;
    if len < size {
        return Err(EINVAL);
    }
    let bits: u64 = if cpus == 64 { u64::MAX } else { (1 << cpus) - 1 };
    put(mask, &bits.to_le_bytes())?;
    Ok(size)
}

/// The kernel's host name file (`docs/linux-compat.md`).
const HOSTNAME: &str = "/proc/sys/kernel/hostname";

pub fn uname(buf: usize) -> R {
    let mut b = [0u8; 6 * 65];
    let mut host = [0u8; 65];
    let mut n = fs::read(HOSTNAME, 0, &mut host).unwrap_or(0);
    while n > 0 && host[n - 1] == b'\n' {
        n -= 1;
    }
    let fields: [&[u8]; 6] =
        [b"Linux", &host[..n.min(64)], b"6.1.0-myos-compat", b"#1 myos", super::arch::MACHINE, b"(none)"];
    for (i, f) in fields.iter().enumerate() {
        b[i * 65..i * 65 + f.len()].copy_from_slice(f);
    }
    put(buf, &b)?;
    Ok(0)
}

pub fn sethostname(name: usize, len: usize) -> R {
    if len > 64 {
        return Err(EINVAL);
    }
    let mut b = [0u8; 65];
    if len > 0 {
        get(name, &mut b[..len])?;
    }
    if b[..len].contains(&b'\n') {
        return Err(EINVAL);
    }
    // The newline ends the name, and makes an empty one a write of 1 byte.
    b[len] = b'\n';
    fs::write(HOSTNAME, 0, &b[..=len]).ok_or(EPERM)?;
    Ok(0)
}

// ---- time -----------------------------------------------------------------

/// The `struct timespec` at `ts` as a deadline in [`now_us`] time: as is
/// when `absolute`, else from now. Every Linux clock reads that one clock.
pub(super) fn timespec_deadline(ts: usize, absolute: bool) -> R2<u64> {
    let sec = get_u64(ts)?;
    let nsec = get_u64(ts + 8)?;
    let t = sec.saturating_mul(1_000_000).saturating_add(nsec / 1000);
    Ok(if absolute { t } else { now_us().saturating_add(t) })
}

/// The monotonic-ns deadline (for `block_until`) of `deadline_us`.
pub(super) fn monotonic_deadline(deadline_us: u64) -> u64 {
    crate::k::time::monotonic_ns()
        .saturating_add(deadline_us.saturating_sub(now_us()).saturating_mul(1000))
        .max(1)
}

pub(super) fn now_us() -> u64 {
    match crate::k::time::timeval() {
        Some((s, us)) => s as u64 * 1_000_000 + us as u64,
        None => 0,
    }
}

pub fn clock_gettime(ts: usize) -> R {
    let us = now_us();
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&(us / 1_000_000).to_le_bytes());
    b[8..].copy_from_slice(&((us % 1_000_000) * 1000).to_le_bytes());
    put(ts, &b)?;
    Ok(0)
}

#[allow(dead_code)]
pub fn time(t: usize) -> R {
    let s = (now_us() / 1_000_000) as usize;
    if t != 0 {
        put(t, &s.to_le_bytes())?;
    }
    Ok(s)
}

/// Sleep for the timespec at `req` (or until that absolute time).
pub fn nanosleep(req: usize, absolute: bool) -> R {
    let deadline = timespec_deadline(req, absolute)?;
    // Really sleep (Blocked, CPU halts) until the monotonic equivalent.
    let mono = monotonic_deadline(deadline);
    loop {
        if now_us() >= deadline {
            return Ok(0);
        }
        if crate::k::signal::interrupt_wait() {
            return Err(EINTR);
        }
        task::sleep_until(mono, false);
    }
}

pub fn getrandom(buf: usize, len: usize) -> R {
    let mut done = 0;
    let mut tmp = [0u8; 256];
    while done < len {
        let n = (len - done).min(tmp.len());
        crate::k::rng::fill(&mut tmp[..n]);
        put(buf + done, &tmp[..n])?;
        done += n;
    }
    Ok(len)
}

pub fn prlimit(resource: usize, old: usize) -> R {
    const RLIMIT_NOFILE: usize = 7;
    if old != 0 {
        let v = if resource == RLIMIT_NOFILE { 32 } else { u64::MAX };
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&v.to_le_bytes());
        b[8..].copy_from_slice(&v.to_le_bytes());
        put(old, &b)?;
    }
    Ok(0)
}

/// poll(2) on pipes and sockets; other fds always report ready.
pub fn poll(fds: usize, nfds: usize, timeout_ms: isize) -> R {
    const POLLIN: u16 = 1;
    const POLLOUT: u16 = 4;
    const POLLERR: u16 = 8;
    const POLLHUP: u16 = 0x10;
    const POLLNVAL: u16 = 0x20;
    if nfds > 64 {
        return Err(EINVAL);
    }
    let deadline = (timeout_ms >= 0).then(|| now_us().saturating_add(timeout_ms as u64 * 1000));
    loop {
        let seq = task::wait_seq();
        let mut ready = 0;
        for i in 0..nfds {
            let mut b = [0u8; 8];
            get(fds + i * 8, &mut b)?;
            let fd = i32::from_le_bytes(b[..4].try_into().unwrap());
            let events = u16::from_le_bytes([b[4], b[5]]);
            let rev = if fd < 0 {
                0
            } else if task::fd_kind(fd as usize).is_none() {
                POLLNVAL
            } else if let Some(r) = net::poll_events(fd as usize) {
                r & (events | POLLERR | POLLHUP)
            } else {
                match task::fd_poll_bits(fd as usize) {
                    Some(bits) => {
                        let mut r = 0;
                        if bits & 1 != 0 {
                            r |= POLLIN;
                        }
                        if bits & 2 != 0 {
                            r |= POLLOUT;
                        }
                        if bits & 4 != 0 {
                            r |= POLLHUP;
                        }
                        r & (events | POLLHUP)
                    }
                    None => events & (POLLIN | POLLOUT),
                }
            };
            b[6..8].copy_from_slice(&rev.to_le_bytes());
            put(fds + i * 8, &b)?;
            if rev != 0 {
                ready += 1;
            }
        }
        if ready > 0 || deadline.is_some_and(|d| now_us() >= d) {
            return Ok(ready);
        }
        if crate::k::signal::interrupt_wait() {
            return Err(EINTR);
        }
        // Sleep until something happens (pipe/tty/device traffic, an exit)
        // or the timeout; readiness is re-scanned above either way.
        let mono = match deadline {
            Some(d) => crate::k::time::monotonic_ns()
                .saturating_add(d.saturating_sub(now_us()).saturating_mul(1000))
                .max(1),
            None => 0,
        };
        task::block_until(task::WAIT_ANY, seq, mono);
    }
}

/// `ppoll(fds, nfds, timeout, sigmask)` (the only poll on aarch64/riscv64;
/// the signal mask argument is not applied).
pub fn ppoll(fds: usize, nfds: usize, ts: usize) -> R {
    let ms = if ts == 0 {
        -1
    } else {
        let sec = get_u64(ts)?;
        let nsec = get_u64(ts + 8)?;
        (sec.saturating_mul(1000) + nsec.div_ceil(1_000_000)).min(isize::MAX as u64) as isize
    };
    poll(fds, nfds, ms)
}
