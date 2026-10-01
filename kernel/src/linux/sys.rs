//! Linux syscall handlers, independent of the arch's syscall numbering.
//!
//! Each takes Linux arguments, calls the native implementation and returns
//! a Linux result (`-errno` on failure).

use alloc::string::String;
use alloc::vec::Vec;

use super::abi::*;
use super::files;
use crate::fs;
use crate::task;
use crate::user;

const AT_FDCWD: usize = -100isize as usize;
const AT_EMPTY_PATH: usize = 0x1000;
const AT_REMOVEDIR: usize = 0x200;
const MAX_PATH: usize = 256;
const ENAMETOOLONG: usize = 36;
const EISDIR: usize = 21;
const EAGAIN: usize = 11;

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
    r >= crate::signal::SYSERR_EINTR
}

fn native(r: usize, generic: usize) -> R {
    let v = result(r, generic);
    if native_failed(r) { Err(v.wrapping_neg()) } else { Ok(v) }
}

fn aspace() -> u64 {
    task::current_aspace()
}

pub(super) fn put(ptr: usize, bytes: &[u8]) -> Result<(), usize> {
    if ptr == 0 || !user::buffer_ok(ptr, bytes.len()) || !user::copy_to_user(aspace(), ptr, bytes) {
        return Err(EFAULT);
    }
    Ok(())
}

pub(super) fn get_bytes(ptr: usize, out: &mut [u8]) -> Result<(), usize> {
    get(ptr, out)
}

fn get(ptr: usize, out: &mut [u8]) -> Result<(), usize> {
    if ptr == 0 || !user::copy_from_user(aspace(), ptr, out) {
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

/// A NULL-terminated array of strings (`argv` / `envp`).
fn user_str_array(ptr: usize, max_n: usize, max_len: usize) -> Result<Vec<Vec<u8>>, usize> {
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
        let s = user_cstr(p, max_len).map_err(|e| if e == ENAMETOOLONG { E2BIG } else { e })?;
        if s.len() > max_len {
            return Err(E2BIG);
        }
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

/// The VFS path behind `path` (cwd and chroot applied).
fn real_path(path: &str) -> R2<String> {
    user::resolve_copied_path(path).ok_or(ENOENT)
}

// ---- files ----------------------------------------------------------------

pub fn read(fd: usize, buf: usize, len: usize) -> R {
    if files::get(fd).is_some_and(|e| e.dir) {
        return Err(EISDIR);
    }
    native(user::sys_read(fd, buf, len), EBADF)
}

pub fn write(fd: usize, buf: usize, len: usize) -> R {
    native(task::fd_write(fd, buf, len), EBADF)
}

/// readv / writev: one native call per `iovec`, stopping at a short one.
pub fn rw_vec(fd: usize, iov: usize, cnt: usize, write: bool) -> R {
    if cnt > 1024 {
        return Err(EINVAL);
    }
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
}

pub fn openat(dirfd: usize, path: usize, flags: usize) -> R {
    const O_ACCMODE: usize = 3;
    const O_CREAT: usize = 0o100;
    const O_EXCL: usize = 0o200;
    const O_LARGEFILE: usize = 0o100000;
    const O_DIRECTORY: usize = 0o200000;
    const O_NOFOLLOW: usize = 0o400000;
    const O_CLOEXEC: usize = 0o2000000;
    let p = path_at(dirfd, path)?;
    let real = real_path(&p)?;
    let st = fs::stat(&real);
    if st.as_ref().is_some_and(|s| s.mode & fs::S_IFMT == 0o040000) {
        if flags & O_ACCMODE != 0 {
            return Err(EISDIR);
        }
        // Directories have no native fd; hold the slot with /dev/null and
        // serve getdents64 from the path table.
        let fd = native(user::open_path("/dev/null", 0), ENOENT)?;
        files::set(fd, view_path(&p), true);
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
    Ok(fd)
}

pub fn close(fd: usize) -> R {
    files::remove(fd);
    if task::fd_close(fd) { Ok(0) } else { Err(EBADF) }
}

fn put_stat(buf: usize, st: &fs::StatInfo) -> R {
    put(buf, &super::arch::stat_bytes(st.mode, st.size as u64, st.ino as u64, st.nlink as u64, st.dev as u64))?;
    Ok(0)
}

pub fn fstatat(dirfd: usize, path: usize, buf: usize, flags: usize) -> R {
    if flags & AT_EMPTY_PATH != 0 && user_cstr(path, MAX_PATH).is_ok_and(|p| p.is_empty()) {
        return fstat(dirfd, buf);
    }
    let real = real_path(&path_at(dirfd, path)?)?;
    let st = fs::stat(&real).ok_or(ENOENT)?;
    put_stat(buf, &st)
}

pub fn fstat(fd: usize, buf: usize) -> R {
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
    put(buf, &super::arch::stat_bytes(mode, size, fd as u64 + 1, 1, 0))?;
    Ok(0)
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

pub fn ioctl(fd: usize, req: usize, arg: usize) -> R {
    native(task::fd_ioctl(fd, req, arg), ENOTTY)
}

pub fn faccessat(dirfd: usize, path: usize) -> R {
    let real = real_path(&path_at(dirfd, path)?)?;
    fs::stat(&real).map(|_| 0).ok_or(ENOENT)
}

pub fn pipe2(fds: usize) -> R {
    let (r, w) = task::pipe_open().ok_or(EMFILE)?;
    files::remove(r);
    files::remove(w);
    let mut b = [0u8; 8];
    b[..4].copy_from_slice(&(r as i32).to_le_bytes());
    b[4..].copy_from_slice(&(w as i32).to_le_bytes());
    put(fds, &b)?;
    Ok(0)
}

pub fn dup(fd: usize, min: usize) -> R {
    let new = task::fd_dup_min(fd, min).ok_or(EBADF)?;
    files::dup(fd, new);
    Ok(new)
}

pub fn dup3(old: usize, new: usize, same_ok: bool) -> R {
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
    Ok(new)
}

pub fn fcntl(fd: usize, cmd: usize, arg: usize) -> R {
    const F_DUPFD: usize = 0;
    const F_GETFD: usize = 1;
    const F_SETFD: usize = 2;
    const F_GETFL: usize = 3;
    const F_SETFL: usize = 4;
    const F_DUPFD_CLOEXEC: usize = 1030;
    if task::fd_kind(fd).is_none() {
        return Err(EBADF);
    }
    match cmd {
        F_DUPFD | F_DUPFD_CLOEXEC => dup(fd, arg),
        // No close-on-exec or non-blocking flags to report or change yet.
        F_GETFD | F_SETFD | F_SETFL => Ok(0),
        F_GETFL => Ok(2), // O_RDWR
        _ => Err(EINVAL),
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
    let real = real_path(&path_at(dirfd, path)?)?;
    if fs::stat(&real).is_some() {
        return Err(EEXIST);
    }
    if fs::mkdir(&real) { Ok(0) } else { Err(ENOENT) }
}

pub fn unlinkat(dirfd: usize, path: usize, flags: usize) -> R {
    let real = real_path(&path_at(dirfd, path)?)?;
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
    let old = real_path(&path_at(olddir, old)?)?;
    let new = real_path(&path_at(newdir, new)?)?;
    if fs::rename(&old, &new) { Ok(0) } else { Err(ENOENT) }
}

pub fn symlinkat(target: usize, newdir: usize, link: usize) -> R {
    let target = String::from_utf8(user_cstr(target, MAX_PATH)?).map_err(|_| EINVAL)?;
    let link = real_path(&path_at(newdir, link)?)?;
    if fs::symlink(&target, &link) { Ok(0) } else { Err(EEXIST) }
}

pub fn readlinkat(dirfd: usize, path: usize, buf: usize, size: usize) -> R {
    let real = real_path(&path_at(dirfd, path)?)?;
    let mut tmp = [0u8; MAX_PATH];
    let cap = size.min(tmp.len());
    let n = fs::readlink(&real, &mut tmp[..cap]).ok_or(EINVAL)?;
    put(buf, &tmp[..n])?;
    Ok(n)
}

// ---- memory ---------------------------------------------------------------

pub fn mmap(addr: usize, len: usize, prot: usize, flags: usize, fd: usize, off: usize) -> R {
    const MAP_ANONYMOUS: usize = 0x20;
    if flags & MAP_ANONYMOUS == 0 {
        // File mappings are not supported yet.
        return Err(ENODEV);
    }
    native(user::do_mmap(addr, len, prot, flags, fd as isize, off), ENOMEM)
}

// ---- processes ------------------------------------------------------------

pub fn fork(user_rip: usize, user_rsp: usize) -> R {
    native(user::sys_fork(user_rip, user_rsp), EAGAIN)
}

pub fn clone(flags: usize, stack: usize, user_rip: usize, user_rsp: usize) -> R {
    // Only the fork-equivalent form (exit signal in the low byte, no
    // sharing flags, no new stack). Threads and CLONE_VM are not supported.
    if flags & !0xff != 0 || stack != 0 {
        return Err(ENOSYS);
    }
    fork(user_rip, user_rsp)
}

pub fn execve(path: usize, argv: usize, envp: usize) -> R {
    let p = path_at(AT_FDCWD, path)?;
    // The native exec limits (kernel/src/user/mod.rs).
    let args = user_str_array(argv, 16, 128)?;
    let env = user_str_array(envp, 32, 128)?;
    let arg_refs: Vec<&[u8]> = args.iter().map(|s| s.as_slice()).collect();
    let env_refs: Vec<&[u8]> = env.iter().map(|s| s.as_slice()).collect();
    native(super::exec_linux(&p, &arg_refs, &env_refs), ENOENT)
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
    if crate::signal::kill(pid as isize, n) { Ok(0) } else { Err(ESRCH) }
}

pub fn uname(buf: usize) -> R {
    let mut b = [0u8; 6 * 65];
    let fields: [&[u8]; 6] =
        [b"Linux", b"myos", b"6.1.0-myos-compat", b"#1 myos", super::arch::MACHINE, b"(none)"];
    for (i, f) in fields.iter().enumerate() {
        b[i * 65..i * 65 + f.len()].copy_from_slice(f);
    }
    put(buf, &b)?;
    Ok(0)
}

// ---- time -----------------------------------------------------------------

pub(super) fn now_us() -> u64 {
    match crate::time::timeval() {
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

#[cfg(target_arch = "x86_64")]
pub fn time(t: usize) -> R {
    let s = (now_us() / 1_000_000) as usize;
    if t != 0 {
        put(t, &s.to_le_bytes())?;
    }
    Ok(s)
}

/// Sleep for the timespec at `req` (or until that absolute time).
pub fn nanosleep(req: usize, absolute: bool) -> R {
    let sec = get_u64(req)?;
    let nsec = get_u64(req + 8)?;
    let t = sec.saturating_mul(1_000_000).saturating_add(nsec / 1000);
    let deadline = if absolute { t } else { now_us().saturating_add(t) };
    while now_us() < deadline {
        if crate::signal::interrupt_wait() {
            return Err(EINTR);
        }
        task::yield_now();
    }
    Ok(0)
}

pub fn getrandom(buf: usize, len: usize) -> R {
    let mut done = 0;
    let mut tmp = [0u8; 256];
    while done < len {
        let n = (len - done).min(tmp.len());
        crate::rng::fill(&mut tmp[..n]);
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

/// poll(2) on pipes; other fds always report ready.
pub fn poll(fds: usize, nfds: usize, timeout_ms: isize) -> R {
    const POLLIN: u16 = 1;
    const POLLOUT: u16 = 4;
    const POLLHUP: u16 = 0x10;
    const POLLNVAL: u16 = 0x20;
    if nfds > 64 {
        return Err(EINVAL);
    }
    let deadline = (timeout_ms >= 0).then(|| now_us().saturating_add(timeout_ms as u64 * 1000));
    loop {
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
        if crate::signal::interrupt_wait() {
            return Err(EINTR);
        }
        task::yield_now();
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
