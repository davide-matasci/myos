//! myos C ABI shims for rustix / uutils (minimal; grow as port proceeds).

use crate::prelude::*;

pub type intmax_t = i64;
pub type uintmax_t = u64;
pub type size_t = usize;
pub type ptrdiff_t = isize;
pub type intptr_t = isize;
pub type uintptr_t = usize;
pub type ssize_t = isize;
pub type pid_t = i32;
pub type uid_t = u32;
pub type gid_t = u32;
pub type mode_t = u32;
pub type dev_t = u64;
pub type ino_t = u64;
pub type nlink_t = u64;
pub type off_t = i64;
pub type blksize_t = i64;
pub type blkcnt_t = i64;
pub type time_t = i64;
pub type suseconds_t = i64;
pub type clock_t = i64;
pub type clockid_t = i32;
pub type sigset_t = u64;
pub type socklen_t = u32;
pub type sa_family_t = u16;

extern_ty! {
    pub type DIR;
}

s! {
    pub struct stat {
        pub st_dev: dev_t,
        pub st_ino: ino_t,
        pub st_mode: mode_t,
        pub st_nlink: nlink_t,
        pub st_uid: uid_t,
        pub st_gid: gid_t,
        pub st_rdev: dev_t,
        pub __pad1: c_long,
        pub st_size: off_t,
        pub st_blksize: blksize_t,
        pub st_blocks: blkcnt_t,
        pub st_atime: time_t,
        pub st_atime_nsec: c_long,
        pub st_mtime: time_t,
        pub st_mtime_nsec: c_long,
        pub st_ctime: time_t,
        pub st_ctime_nsec: c_long,
        pub __unused: [c_long; 3],
    }

    pub struct timespec {
        pub tv_sec: time_t,
        pub tv_nsec: c_long,
    }

    pub struct timeval {
        pub tv_sec: time_t,
        pub tv_usec: suseconds_t,
    }

    pub struct iovec {
        pub iov_base: *mut c_void,
        pub iov_len: size_t,
    }

    pub struct dirent {
        pub d_ino: ino_t,
        pub d_off: off_t,
        pub d_reclen: c_ushort,
        pub d_type: c_uchar,
        pub d_name: [c_char; 256],
    }
}

// Linux x86_64 errno values (subset).
pub const EPERM: c_int = 1;
pub const ENOENT: c_int = 2;
pub const EINTR: c_int = 4;
pub const EIO: c_int = 5;
pub const EBADF: c_int = 9;
pub const ENOMEM: c_int = 12;
pub const EACCES: c_int = 13;
pub const EFAULT: c_int = 14;
pub const EEXIST: c_int = 17;
pub const ENOTDIR: c_int = 20;
pub const EINVAL: c_int = 22;
pub const ENOSYS: c_int = 38;
pub const ENOTEMPTY: c_int = 39;
pub const ELOOP: c_int = 40;
pub const ENAMETOOLONG: c_int = 36;
pub const EOVERFLOW: c_int = 75;
pub const TIMER_ABSTIME: c_int = 1;
pub const LC_CTYPE: c_int = 0;

pub const O_RDONLY: c_int = 0;
pub const O_WRONLY: c_int = 1;
pub const O_RDWR: c_int = 2;
pub const O_CREAT: c_int = 0o100;
pub const O_EXCL: c_int = 0o200;
pub const O_TRUNC: c_int = 0o1000;
pub const O_APPEND: c_int = 0o2000;
pub const O_CLOEXEC: c_int = 0o2000000;
pub const O_DIRECTORY: c_int = 0o200000;
pub const O_NOFOLLOW: c_int = 0o400000;
pub const O_NONBLOCK: c_int = 0o4000;

pub const S_IFMT: mode_t = 0o170000;
pub const S_IFREG: mode_t = 0o100000;
pub const S_IFDIR: mode_t = 0o040000;
pub const S_IFLNK: mode_t = 0o120000;

pub const STDIN_FILENO: c_int = 0;
pub const STDOUT_FILENO: c_int = 1;
pub const STDERR_FILENO: c_int = 2;

pub const AT_FDCWD: c_int = -100;

pub const F_DUPFD: c_int = 0;
pub const F_GETFD: c_int = 1;
pub const F_SETFD: c_int = 2;
pub const F_GETFL: c_int = 3;
pub const F_SETFL: c_int = 4;
pub const FD_CLOEXEC: c_int = 1;

pub const DT_UNKNOWN: c_uchar = 0;
pub const DT_REG: c_uchar = 8;
pub const DT_DIR: c_uchar = 4;
pub const DT_LNK: c_uchar = 10;

pub const CLOCK_REALTIME: clockid_t = 0;
pub const CLOCK_MONOTONIC: clockid_t = 1;

mod syscalls {
    use super::*;

    const SYS_WRITE: usize = 0;
    const SYS_EXIT: usize = 1;
    const SYS_OPEN: usize = 2;
    const SYS_READ: usize = 3;
    const SYS_CLOSE: usize = 4;
    const SYS_FORK: usize = 6;
    const SYS_WAIT: usize = 7;
    const SYS_BRK: usize = 9;
    const SYS_PIPE: usize = 10;
    const SYS_DUP2: usize = 11;
    const SYS_STAT: usize = 12;
    const SYS_READLINK: usize = 22;
    const SYS_GETTIMEOFDAY: usize = 33;
    const SYS_KILL: usize = 34;
    const SYS_SIGACTION: usize = 35;
    const SYS_GETPID: usize = 36;
    const SYS_STAT2: usize = 61;
    const SYS_UTIMENS: usize = 62;
    const SYS_FUTIMENS: usize = 63;
    /// `utimens` / `futimens` time values: now, or leave the time as it is.
    const KERNEL_UTIME_NOW: i64 = -1;
    const KERNEL_UTIME_OMIT: i64 = -2;

    static mut MYOS_ERRNO: c_int = 0;

    pub fn set_errno(e: c_int) {
        unsafe {
            MYOS_ERRNO = e;
        }
    }

    pub fn get_errno() -> c_int {
        unsafe { MYOS_ERRNO }
    }

    fn cstr_len(ptr: *const c_char) -> usize {
        if ptr.is_null() {
            return 0;
        }
        unsafe {
            let mut n = 0usize;
            while *ptr.add(n) != 0 {
                n += 1;
            }
            n
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe fn raw_syscall(nr: usize, a0: usize, a1: usize, a2: usize) -> usize {
        let ret: usize;
        core::arch::asm!(
            "syscall",
            in("rax") nr,
            in("rdi") a0,
            in("rsi") a1,
            in("rdx") a2,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
        ret
    }

    #[cfg(target_arch = "aarch64")]
    unsafe fn raw_syscall(nr: usize, a0: usize, a1: usize, a2: usize) -> usize {
        let ret: usize;
        core::arch::asm!(
            "svc #0",
            in("x8") nr,
            in("x0") a0,
            in("x1") a1,
            in("x2") a2,
            lateout("x0") ret,
            options(nostack),
        );
        ret
    }

    #[cfg(target_arch = "riscv64")]
    unsafe fn raw_syscall(nr: usize, a0: usize, a1: usize, a2: usize) -> usize {
        let ret: usize;
        core::arch::asm!(
            "ecall",
            in("a7") nr,
            in("a0") a0,
            in("a1") a1,
            in("a2") a2,
            lateout("a0") ret,
            options(nostack),
        );
        ret
    }

    pub unsafe fn sys_write(fd: c_int, buf: *const c_void, len: size_t) -> ssize_t {
        let ret = raw_syscall(SYS_WRITE, fd as usize, buf as usize, len);
        if ret == usize::MAX {
            set_errno(EIO);
            -1
        } else {
            ret as ssize_t
        }
    }

    pub unsafe fn sys_read(fd: c_int, buf: *mut c_void, len: size_t) -> ssize_t {
        let ret = raw_syscall(SYS_READ, fd as usize, buf as usize, len);
        if ret == usize::MAX {
            set_errno(EIO);
            -1
        } else {
            ret as ssize_t
        }
    }

    pub unsafe fn sys_open(path: *const c_char, _flags: c_int, _mode: mode_t) -> c_int {
        let len = cstr_len(path);
        let ret = raw_syscall(SYS_OPEN, path as usize, len, 0);
        if ret == usize::MAX {
            set_errno(ENOENT);
            -1
        } else {
            ret as c_int
        }
    }

    pub unsafe fn sys_close(fd: c_int) -> c_int {
        let ret = raw_syscall(SYS_CLOSE, fd as usize, 0, 0);
        if ret == usize::MAX {
            set_errno(EBADF);
            -1
        } else {
            0
        }
    }

    pub unsafe fn sys_pipe(fds: *mut c_int) -> c_int {
        let ret = raw_syscall(SYS_PIPE, fds as usize, 0, 0);
        if ret == usize::MAX {
            set_errno(ENOSYS);
            -1
        } else {
            0
        }
    }

    pub unsafe fn sys_dup2(oldfd: c_int, newfd: c_int) -> c_int {
        let ret = raw_syscall(SYS_DUP2, oldfd as usize, newfd as usize, 0);
        if ret == usize::MAX {
            set_errno(EBADF);
            -1
        } else {
            newfd
        }
    }

    pub unsafe fn sys_fork() -> pid_t {
        let ret = raw_syscall(SYS_FORK, 0, 0, 0);
        if ret == usize::MAX {
            set_errno(ENOSYS);
            -1
        } else {
            ret as pid_t
        }
    }

    pub unsafe fn sys_wait(status: *mut c_int) -> pid_t {
        let ret = raw_syscall(SYS_WAIT, status as usize, 0, 0);
        if ret == usize::MAX {
            set_errno(ECHILD);
            -1
        } else {
            ret as pid_t
        }
    }

    pub unsafe fn sys_brk(addr: *mut c_void) -> *mut c_void {
        let ret = raw_syscall(SYS_BRK, addr as usize, 0, 0);
        ret as *mut c_void
    }

    /// Kernel open flags (`open_path`): write access for a control file.
    pub const K_O_WRONLY: usize = 1;

    pub unsafe fn sys_open_flags(path: *const c_char, kflags: usize) -> c_int {
        let len = cstr_len(path);
        let ret = raw_syscall(SYS_OPEN, path as usize, len, kflags);
        if ret == usize::MAX {
            set_errno(ENOENT);
            -1
        } else {
            ret as c_int
        }
    }

    pub unsafe fn sys_readlink(path: *const c_char, buf: *mut c_char, bufsiz: size_t) -> ssize_t {
        let len = cstr_len(path);
        if len == 0 || len > 0xffff || bufsiz == 0 || bufsiz > 0xffff {
            set_errno(ENAMETOOLONG);
            return -1;
        }
        let ret = raw_syscall(SYS_READLINK, path as usize, buf as usize, (len << 16) | bufsiz);
        if ret == usize::MAX {
            set_errno(ENOENT);
            -1
        } else {
            ret as ssize_t
        }
    }

    pub unsafe fn sys_gettimeofday(tv: *mut timeval) -> c_int {
        let ret = raw_syscall(SYS_GETTIMEOFDAY, tv as usize, 0, 0);
        if ret == usize::MAX {
            set_errno(EIO);
            -1
        } else {
            0
        }
    }

    pub unsafe fn sys_kill(pid: pid_t, sig: c_int) -> c_int {
        if sig <= 0 || sig > 31 {
            set_errno(EINVAL);
            return -1;
        }
        let ret = raw_syscall(SYS_KILL, pid as usize, sig as usize, 0);
        if ret == usize::MAX {
            set_errno(ESRCH);
            -1
        } else {
            0
        }
    }

    pub unsafe fn sys_sigaction(sig: c_int, act: *const c_void, oact: *mut c_void) -> c_int {
        if sig <= 0 || sig > 31 {
            set_errno(EINVAL);
            return -1;
        }
        let ret = raw_syscall(SYS_SIGACTION, sig as usize, act as usize, oact as usize);
        if ret == usize::MAX {
            set_errno(ENOSYS);
            -1
        } else {
            0
        }
    }

    pub unsafe fn sys_getpid() -> pid_t {
        let ret = raw_syscall(SYS_GETPID, 0, 0, 0);
        ret as pid_t
    }

    /// Kernel `MyosStat2Buf` layout (must match `kernel/src/user/syscall.rs`).
    #[repr(C)]
    #[derive(Default)]
    struct KernelStat {
        st_mode: u32,
        st_nlink: u32,
        st_ino: u32,
        st_dev: u32,
        st_size: u64,
        st_atime: i64,
        st_mtime: i64,
    }

    /// Path-based stat via SYS_STAT2. Unlike open+fstat, this works for directories
    /// (VFS refuses to open dirs, which broke rustix/uutils `ls`).
    pub unsafe fn sys_stat(path: *const c_char, buf: *mut super::stat) -> c_int {
        if path.is_null() || buf.is_null() {
            set_errno(EINVAL);
            return -1;
        }
        let len = cstr_len(path);
        let mut kstat = KernelStat::default();
        let ret = raw_syscall(SYS_STAT2, path as usize, len, &mut kstat as *mut _ as usize);
        if ret == usize::MAX {
            set_errno(ENOENT);
            return -1;
        }
        core::ptr::write_bytes(buf as *mut u8, 0, core::mem::size_of::<super::stat>());
        (*buf).st_dev = kstat.st_dev as dev_t;
        (*buf).st_ino = kstat.st_ino as ino_t;
        (*buf).st_mode = kstat.st_mode;
        (*buf).st_nlink = if kstat.st_nlink == 0 {
            1
        } else {
            kstat.st_nlink as nlink_t
        };
        (*buf).st_size = kstat.st_size as off_t;
        (*buf).st_blksize = 4096;
        (*buf).st_blocks = kstat.st_size.div_ceil(512) as blkcnt_t;
        (*buf).st_atime = kstat.st_atime as time_t;
        (*buf).st_mtime = kstat.st_mtime as time_t;
        (*buf).st_ctime = kstat.st_mtime as time_t;
        0
    }

    /// `timespec[2]` (null = both now) -> the kernel's two `i64` seconds.
    unsafe fn kernel_times(times: *const super::timespec, out: &mut [i64; 2]) -> Option<usize> {
        if times.is_null() {
            return Some(0);
        }
        for (i, t) in out.iter_mut().enumerate() {
            let ts = &*times.add(i);
            *t = match ts.tv_nsec {
                super::UTIME_NOW => KERNEL_UTIME_NOW,
                super::UTIME_OMIT => KERNEL_UTIME_OMIT,
                0..=999_999_999 if ts.tv_sec >= 0 => ts.tv_sec as i64,
                _ => {
                    set_errno(EINVAL);
                    return None;
                }
            };
        }
        Some(out.as_ptr() as usize)
    }

    /// utimensat on a path (relative to the cwd), following symlinks.
    pub unsafe fn sys_utimens(path: *const c_char, times: *const super::timespec) -> c_int {
        let mut raw = [0i64; 2];
        let Some(times) = kernel_times(times, &mut raw) else {
            return -1;
        };
        if raw_syscall(SYS_UTIMENS, path as usize, cstr_len(path), times) == usize::MAX {
            set_errno(ENOENT);
            return -1;
        }
        0
    }

    pub unsafe fn sys_futimens(fd: c_int, times: *const super::timespec) -> c_int {
        let mut raw = [0i64; 2];
        let Some(times) = kernel_times(times, &mut raw) else {
            return -1;
        };
        if raw_syscall(SYS_FUTIMENS, fd as usize, times, 0) == usize::MAX {
            set_errno(EBADF);
            return -1;
        }
        0
    }
}

pub const ECHILD: c_int = 10;

macro_rules! enosys {
    ($($(#[$attr:meta])* $vis:vis unsafe fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)*) => {$(
        $(#[$attr])*
        $vis unsafe fn $name($($arg: $ty),*) -> $ret {
            let _ = ($($arg,)*);
            syscalls::set_errno(ENOSYS);
            (-1isize) as $ret
        }
    )*};
}

// Implemented syscalls.
#[no_mangle]
pub unsafe extern "C" fn write(fd: c_int, buf: *const c_void, count: size_t) -> ssize_t {
    syscalls::sys_write(fd, buf, count)
}

#[no_mangle]
pub unsafe extern "C" fn read(fd: c_int, buf: *mut c_void, count: size_t) -> ssize_t {
    syscalls::sys_read(fd, buf, count)
}

#[no_mangle]
pub unsafe extern "C" fn open(path: *const c_char, flags: c_int, mode: mode_t) -> c_int {
    let _ = (flags, mode);
    syscalls::sys_open(path, flags, mode)
}

#[no_mangle]
pub unsafe extern "C" fn close(fd: c_int) -> c_int {
    syscalls::sys_close(fd)
}

#[no_mangle]
pub unsafe extern "C" fn pipe(fds: *mut c_int) -> c_int {
    syscalls::sys_pipe(fds)
}

#[no_mangle]
pub unsafe extern "C" fn dup2(oldfd: c_int, newfd: c_int) -> c_int {
    syscalls::sys_dup2(oldfd, newfd)
}

#[no_mangle]
pub unsafe extern "C" fn fork() -> pid_t {
    syscalls::sys_fork()
}

#[no_mangle]
pub unsafe extern "C" fn wait(status: *mut c_int) -> pid_t {
    syscalls::sys_wait(status)
}

#[no_mangle]
pub unsafe extern "C" fn waitpid(pid: pid_t, status: *mut c_int, _options: c_int) -> pid_t {
    let _ = pid;
    syscalls::sys_wait(status)
}

#[no_mangle]
pub unsafe extern "C" fn isatty(fd: c_int) -> c_int {
    let mut dir = [0u8; tty::PATH];
    if tty::dir(fd, &mut dir).is_some() {
        1
    } else {
        syscalls::set_errno(ENOTTY);
        0
    }
}

#[no_mangle]
pub unsafe extern "C" fn fstat(fd: c_int, buf: *mut stat) -> c_int {
    if buf.is_null() {
        syscalls::set_errno(EINVAL);
        return -1;
    }
    if fd < 0 {
        syscalls::set_errno(EBADF);
        return -1;
    }
    unsafe {
        (*buf).st_mode = S_IFREG | 0o644;
        (*buf).st_size = 0;
        (*buf).st_blksize = 4096;
        (*buf).st_nlink = 1;
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn stat(path: *const c_char, buf: *mut stat) -> c_int {
    syscalls::sys_stat(path, buf)
}

#[no_mangle]
pub unsafe extern "C" fn utimensat(dirfd: c_int, path: *const c_char, times: *const timespec, flags: c_int) -> c_int {
    // Only cwd-relative or absolute paths, and never the symlink itself.
    if dirfd != AT_FDCWD || flags != 0 {
        syscalls::set_errno(ENOSYS);
        return -1;
    }
    syscalls::sys_utimens(path, times)
}

#[no_mangle]
pub unsafe extern "C" fn futimens(fd: c_int, times: *const timespec) -> c_int {
    syscalls::sys_futimens(fd, times)
}

#[no_mangle]
pub unsafe extern "C" fn lstat(path: *const c_char, buf: *mut stat) -> c_int {
    // No distinct lstat yet; same as stat (tmpfs symlinks still report as links).
    syscalls::sys_stat(path, buf)
}

#[no_mangle]
pub unsafe extern "C" fn fcntl(fd: c_int, cmd: c_int, arg: c_ulong) -> c_int {
    if fd < 0 {
        syscalls::set_errno(EBADF);
        return -1;
    }
    match cmd {
        F_GETFL => O_RDONLY,
        F_GETFD => 0,
        F_SETFD => 0,
        F_DUPFD => arg as c_int,
        _ => {
            let _ = arg;
            syscalls::set_errno(EINVAL);
            -1
        }
    }
}

/// The tty ioctls, served from the terminal's files (`tty`); there is no
/// ioctl syscall behind this, anything else is ENOTTY.
#[no_mangle]
pub unsafe extern "C" fn ioctl(fd: c_int, request: c_ulong, arg: *mut c_void) -> c_int {
    const TCGETS: c_ulong = 0x5401;
    const TCSETS: c_ulong = 0x5402;
    const TCFLSH: c_ulong = 0x540B;
    const TIOCSCTTY: c_ulong = 0x540E;
    const TIOCGWINSZ: c_ulong = 0x5413;
    const TIOCSWINSZ: c_ulong = 0x5414;
    if fd < 0 {
        syscalls::set_errno(EBADF);
        return -1;
    }
    let ok = match request {
        TCGETS => tty::get(fd, arg as *mut tty::termios, core::ptr::null_mut()),
        TCSETS => tty::set(fd, arg as *const tty::termios),
        TIOCGWINSZ => tty::get(fd, core::ptr::null_mut(), arg as *mut tty::winsize),
        TIOCSWINSZ => {
            let w = &*(arg as *const tty::winsize);
            let mut line = tty::Text::new();
            line.push(b"winsize ");
            line.push_dec(w.ws_row as usize);
            line.push(b" ");
            line.push_dec(w.ws_col as usize);
            line.push(b"\n");
            tty::write(fd, line.bytes())
        }
        TCFLSH => {
            let line: &[u8] = match arg as usize {
                0 => b"flush in\n",
                1 => b"flush out\n",
                2 => b"flush both\n",
                _ => {
                    syscalls::set_errno(EINVAL);
                    return -1;
                }
            };
            tty::write(fd, line)
        }
        TIOCSCTTY => tty::write(fd, b"ctty\n"),
        _ => {
            syscalls::set_errno(ENOTTY);
            return -1;
        }
    };
    if ok { 0 } else { -1 }
}

/// The terminal behind an fd, through its files (docs/tty.md): a directory
/// with `data` (the terminal) and `ctl` (its state as text), found from the
/// fd's `/proc/self/fd` link.
mod tty {
    use super::*;

    /// `struct termios` as libgloss lays it out (56 bytes).
    #[repr(C)]
    pub struct termios {
        pub c_iflag: u32,
        pub c_oflag: u32,
        pub c_cflag: u32,
        pub c_lflag: u32,
        pub c_cc: [u8; 32],
        pub c_ispeed: u32,
        pub c_ospeed: u32,
    }

    #[repr(C)]
    pub struct winsize {
        pub ws_row: u16,
        pub ws_col: u16,
        pub ws_xpixel: u16,
        pub ws_ypixel: u16,
    }

    pub const PATH: usize = 64;
    const CTL: usize = 512;

    /// A small NUL-terminated text buffer (paths, ctl lines).
    pub struct Text {
        buf: [u8; CTL],
        len: usize,
    }

    impl Text {
        pub fn new() -> Self {
            Text { buf: [0; CTL], len: 0 }
        }
        pub fn push(&mut self, s: &[u8]) {
            let n = s.len().min(CTL - 1 - self.len);
            self.buf[self.len..self.len + n].copy_from_slice(&s[..n]);
            self.len += n;
        }
        pub fn push_dec(&mut self, mut v: usize) {
            let mut digits = [0u8; 20];
            let mut i = digits.len();
            loop {
                i -= 1;
                digits[i] = b'0' + (v % 10) as u8;
                v /= 10;
                if v == 0 {
                    break;
                }
            }
            self.push(&digits[i..]);
        }
        pub fn push_hex(&mut self, v: u32, min_digits: usize) {
            let mut digits = [0u8; 8];
            let mut i = digits.len();
            let mut v = v;
            while i > digits.len() - min_digits || v != 0 {
                i -= 1;
                digits[i] = b"0123456789abcdef"[(v & 15) as usize];
                v >>= 4;
            }
            self.push(&digits[i..]);
        }
        pub fn bytes(&self) -> &[u8] {
            &self.buf[..self.len]
        }
        pub fn cstr(&self) -> *const c_char {
            self.buf.as_ptr() as *const c_char
        }
    }

    /// The directory of the terminal `fd` is open on (`/dev/pts/3`), from
    /// its `/proc/self/fd` link, whose target ends in `/data` (or `/master`
    /// for a pty's master end). `None` when the fd is not a terminal.
    pub unsafe fn dir(fd: c_int, out: &mut [u8; PATH]) -> Option<usize> {
        if fd < 0 {
            return None;
        }
        let mut link = Text::new();
        link.push(b"/proc/self/fd/");
        link.push_dec(fd as usize);
        let mut target = [0u8; 128];
        let n = syscalls::sys_readlink(link.cstr(), target.as_mut_ptr() as *mut c_char, target.len());
        if n <= 0 {
            return None;
        }
        let target = &target[..n as usize];
        let slash = target.iter().rposition(|&b| b == b'/')?;
        let tail = &target[slash..];
        if !target.starts_with(b"/dev/") || (tail != b"/data" && tail != b"/master") || slash >= PATH {
            return None;
        }
        out[..slash].copy_from_slice(&target[..slash]);
        Some(slash)
    }

    unsafe fn ctl_open(fd: c_int, kflags: usize) -> Option<c_int> {
        let mut d = [0u8; PATH];
        let n = dir(fd, &mut d)?;
        let mut path = Text::new();
        path.push(&d[..n]);
        path.push(b"/ctl");
        let cfd = syscalls::sys_open_flags(path.cstr(), kflags);
        if cfd < 0 {
            syscalls::set_errno(ENOTTY);
            return None;
        }
        Some(cfd)
    }

    /// Write `text` to the terminal's ctl; false (EINVAL) when the kernel
    /// refused it, ENOTTY when `fd` is not a terminal.
    pub unsafe fn write(fd: c_int, text: &[u8]) -> bool {
        let Some(cfd) = ctl_open(fd, syscalls::K_O_WRONLY) else {
            syscalls::set_errno(ENOTTY);
            return false;
        };
        let n = syscalls::sys_write(cfd, text.as_ptr() as *const c_void, text.len());
        syscalls::sys_close(cfd);
        if n != text.len() as ssize_t {
            syscalls::set_errno(EINVAL);
            return false;
        }
        true
    }

    fn number(word: &[u8]) -> u32 {
        match word.strip_prefix(b"0x") {
            Some(hex) => hex.iter().fold(0u32, |v, &b| (v << 4) | hex_digit(b)),
            None => word.iter().fold(0u32, |v, &b| v * 10 + (b.wrapping_sub(b'0') as u32 % 10)),
        }
    }

    fn hex_digit(b: u8) -> u32 {
        match b {
            b'0'..=b'9' => (b - b'0') as u32,
            b'a'..=b'f' => (b - b'a' + 10) as u32,
            b'A'..=b'F' => (b - b'A' + 10) as u32,
            _ => 0,
        }
    }

    /// Read the terminal's termios and/or window size (null to skip either).
    pub unsafe fn get(fd: c_int, t: *mut termios, w: *mut winsize) -> bool {
        let Some(cfd) = ctl_open(fd, 0) else {
            syscalls::set_errno(ENOTTY);
            return false;
        };
        let mut text = [0u8; CTL];
        let mut len = 0usize;
        while len < text.len() {
            let n = syscalls::sys_read(cfd, text[len..].as_mut_ptr() as *mut c_void, text.len() - len);
            if n <= 0 {
                break;
            }
            len += n as usize;
        }
        syscalls::sys_close(cfd);
        if !t.is_null() {
            core::ptr::write_bytes(t, 0, 1);
        }
        if !w.is_null() {
            core::ptr::write_bytes(w, 0, 1);
        }
        for line in text[..len].split(|&b| b == b'\n') {
            let mut words = line.split(|&b| b == b' ').filter(|w| !w.is_empty());
            let Some(key) = words.next() else { continue };
            if !t.is_null() {
                let t = &mut *t;
                match key {
                    b"iflag" => t.c_iflag = number(words.next().unwrap_or(b"")),
                    b"oflag" => t.c_oflag = number(words.next().unwrap_or(b"")),
                    b"cflag" => t.c_cflag = number(words.next().unwrap_or(b"")),
                    b"lflag" => t.c_lflag = number(words.next().unwrap_or(b"")),
                    b"cc" => {
                        for (i, w) in words.by_ref().take(32).enumerate() {
                            t.c_cc[i] = w.iter().fold(0u32, |v, &b| (v << 4) | hex_digit(b)) as u8;
                        }
                    }
                    b"speed" => {
                        t.c_ispeed = number(words.next().unwrap_or(b""));
                        t.c_ospeed = number(words.next().unwrap_or(b""));
                    }
                    _ => {}
                }
            }
            if !w.is_null() && key == b"winsize" {
                let w = &mut *w;
                w.ws_row = number(words.next().unwrap_or(b"")) as u16;
                w.ws_col = number(words.next().unwrap_or(b"")) as u16;
            }
        }
        true
    }

    /// Write the termios to the terminal.
    pub unsafe fn set(fd: c_int, t: *const termios) -> bool {
        if t.is_null() {
            syscalls::set_errno(EFAULT);
            return false;
        }
        let t = &*t;
        let mut text = Text::new();
        for (name, v) in [
            (&b"iflag 0x"[..], t.c_iflag),
            (b"oflag 0x", t.c_oflag),
            (b"cflag 0x", t.c_cflag),
            (b"lflag 0x", t.c_lflag),
        ] {
            text.push(name);
            text.push_hex(v, 1);
            text.push(b"\n");
        }
        text.push(b"cc");
        for &c in &t.c_cc {
            text.push(b" ");
            text.push_hex(c as u32, 2);
        }
        text.push(b"\nspeed ");
        text.push_dec(t.c_ispeed as usize);
        text.push(b" ");
        text.push_dec(t.c_ospeed as usize);
        text.push(b"\n");
        write(fd, text.bytes())
    }
}

#[no_mangle]
pub unsafe extern "C" fn lseek(_fd: c_int, _offset: off_t, _whence: c_int) -> off_t {
    syscalls::set_errno(ENOSYS);
    -1
}

#[no_mangle]
pub unsafe extern "C" fn getpid() -> pid_t {
    syscalls::sys_getpid()
}

#[no_mangle]
pub unsafe extern "C" fn kill(pid: pid_t, sig: c_int) -> c_int {
    syscalls::sys_kill(pid, sig)
}

#[no_mangle]
pub unsafe extern "C" fn raise(sig: c_int) -> c_int {
    let pid = syscalls::sys_getpid();
    syscalls::sys_kill(pid, sig)
}

#[no_mangle]
pub unsafe extern "C" fn sigaction(sig: c_int, act: *const c_void, oldact: *mut c_void) -> c_int {
    // Kernel expects {handler, flags, mask} usizes. Callers using newlib layout
    // should go through libgloss; rustix/ctrlc typically pass opaque pointers —
    // we forward as-is when non-null (first word = handler).
    syscalls::sys_sigaction(sig, act, oldact)
}

#[no_mangle]
pub unsafe extern "C" fn getuid() -> uid_t {
    0
}

#[no_mangle]
pub unsafe extern "C" fn getgid() -> gid_t {
    0
}

#[no_mangle]
pub unsafe extern "C" fn geteuid() -> uid_t {
    0
}

#[no_mangle]
pub unsafe extern "C" fn getegid() -> gid_t {
    0
}

#[no_mangle]
pub unsafe extern "C" fn getcwd(buf: *mut c_char, size: size_t) -> *mut c_char {
    if buf.is_null() || size < 2 {
        syscalls::set_errno(EINVAL);
        return core::ptr::null_mut();
    }
    unsafe {
        *buf = b'/' as c_char;
        *buf.add(1) = 0;
    }
    buf
}

#[no_mangle]
pub unsafe extern "C" fn chdir(_path: *const c_char) -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn _myos_errno_location() -> *mut c_int {
    static mut ERR: c_int = 0;
    unsafe {
        ERR = syscalls::get_errno();
        &mut ERR as *mut c_int
    }
}

#[no_mangle]
pub unsafe extern "C" fn __errno_location() -> *mut c_int {
    _myos_errno_location()
}


#[no_mangle]
pub unsafe extern "C" fn gettimeofday(tv: *mut timeval, _tz: *mut c_void) -> c_int {
    if tv.is_null() {
        syscalls::set_errno(EINVAL);
        return -1;
    }
    unsafe { syscalls::sys_gettimeofday(tv) }
}

#[no_mangle]
pub unsafe extern "C" fn clock_gettime(_clk: clockid_t, tp: *mut timespec) -> c_int {
    if tp.is_null() {
        syscalls::set_errno(EINVAL);
        return -1;
    }
    unsafe {
        (*tp).tv_sec = 0;
        (*tp).tv_nsec = 0;
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn readlink(path: *const c_char, buf: *mut c_char, bufsiz: size_t) -> ssize_t {
    syscalls::sys_readlink(path, buf, bufsiz)
}

// Stubs for symbols rustix may reference on first compile pass.
enosys! {
    pub unsafe fn openat(dirfd: c_int, path: *const c_char, flags: c_int, mode: mode_t) -> c_int;
    pub unsafe fn unlink(path: *const c_char) -> c_int;
    pub unsafe fn rename(old: *const c_char, new: *const c_char) -> c_int;
    pub unsafe fn mkdir(path: *const c_char, mode: mode_t) -> c_int;
    pub unsafe fn rmdir(path: *const c_char) -> c_int;
    pub unsafe fn link(old: *const c_char, new: *const c_char) -> c_int;
    pub unsafe fn symlink(target: *const c_char, linkpath: *const c_char) -> c_int;
    pub unsafe fn fchmod(fd: c_int, mode: mode_t) -> c_int;
    pub unsafe fn chmod(path: *const c_char, mode: mode_t) -> c_int;
    pub unsafe fn fchown(fd: c_int, owner: uid_t, group: gid_t) -> c_int;
    pub unsafe fn chown(path: *const c_char, owner: uid_t, group: gid_t) -> c_int;
    pub unsafe fn ftruncate(fd: c_int, length: off_t) -> c_int;
    pub unsafe fn truncate(path: *const c_char, length: off_t) -> c_int;
    pub unsafe fn fsync(fd: c_int) -> c_int;
    pub unsafe fn fdatasync(fd: c_int) -> c_int;
    pub unsafe fn faccessat(dirfd: c_int, path: *const c_char, mode: c_int, flags: c_int) -> c_int;
    pub unsafe fn access(path: *const c_char, mode: c_int) -> c_int;
    pub unsafe fn dup(fd: c_int) -> c_int;
    pub unsafe fn getdents64(fd: c_int, dirp: *mut c_void, count: size_t) -> ssize_t;
    pub unsafe fn pread(fd: c_int, buf: *mut c_void, count: size_t, offset: off_t) -> ssize_t;
    pub unsafe fn pwrite(fd: c_int, buf: *const c_void, count: size_t, offset: off_t) -> ssize_t;
    pub unsafe fn readv(fd: c_int, iov: *const iovec, iovcnt: c_int) -> ssize_t;
    pub unsafe fn writev(fd: c_int, iov: *const iovec, iovcnt: c_int) -> ssize_t;
    pub unsafe fn pipe2(fds: *mut c_int, flags: c_int) -> c_int;
    pub unsafe fn sigprocmask(how: c_int, set: *const sigset_t, oldset: *mut sigset_t) -> c_int;
    pub unsafe fn execve(path: *const c_char, argv: *const *const c_char, envp: *const *const c_char) -> c_int;
    pub unsafe fn execvp(file: *const c_char, argv: *const *const c_char) -> c_int;
    pub unsafe fn _exit(status: c_int) -> c_int;
    pub unsafe fn nanosleep(req: *const timespec, rem: *mut timespec) -> c_int;
    pub unsafe fn clock_nanosleep(clock_id: clockid_t, flags: c_int, req: *const timespec, rem: *mut timespec) -> c_int;
    pub unsafe fn sched_yield() -> c_int;
    pub unsafe fn usleep(usec: c_uint) -> c_int;
    pub unsafe fn sleep(secs: c_uint) -> c_uint;
    pub unsafe fn symlinkat(target: *const c_char, newdirfd: c_int, linkpath: *const c_char) -> c_int;
    pub unsafe fn readlinkat(dirfd: c_int, path: *const c_char, buf: *mut c_char, bufsiz: size_t) -> ssize_t;
    pub unsafe fn fstatat(dirfd: c_int, path: *const c_char, buf: *mut stat, flags: c_int) -> c_int;
    pub unsafe fn unlinkat(dirfd: c_int, path: *const c_char, flags: c_int) -> c_int;
    pub unsafe fn mkdirat(dirfd: c_int, path: *const c_char, mode: mode_t) -> c_int;
    pub unsafe fn linkat(olddirfd: c_int, oldpath: *const c_char, newdirfd: c_int, newpath: *const c_char, flags: c_int) -> c_int;
    pub unsafe fn renameat(olddirfd: c_int, oldpath: *const c_char, newdirfd: c_int, newpath: *const c_char) -> c_int;
}


#[no_mangle]
pub unsafe extern "C" fn setlocale(_category: c_int, _locale: *const c_char) -> *mut c_char {
    core::ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn tolower(c: c_int) -> c_int {
    if (b'A' as c_int) <= c && c <= (b'Z' as c_int) {
        c + 32
    } else {
        c
    }
}

#[no_mangle]
pub unsafe extern "C" fn toupper(c: c_int) -> c_int {
    if (b'a' as c_int) <= c && c <= (b'z' as c_int) {
        c - 32
    } else {
        c
    }
}

include!("rustix_compat.rs");
