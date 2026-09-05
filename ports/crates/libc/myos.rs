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
    const SYS_GETTIMEOFDAY: usize = 28;

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

    pub unsafe fn sys_gettimeofday(tv: *mut timeval) -> c_int {
        let ret = raw_syscall(SYS_GETTIMEOFDAY, tv as usize, 0, 0);
        if ret == usize::MAX {
            set_errno(EIO);
            -1
        } else {
            0
        }
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
pub unsafe extern "C" fn isatty(_fd: c_int) -> c_int {
    0
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
    let fd = open(path, O_RDONLY, 0);
    if fd < 0 {
        return -1;
    }
    let r = fstat(fd, buf);
    let _ = close(fd);
    r
}

#[no_mangle]
pub unsafe extern "C" fn lstat(path: *const c_char, buf: *mut stat) -> c_int {
    stat(path, buf)
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

#[no_mangle]
pub unsafe extern "C" fn ioctl(_fd: c_int, _request: c_ulong, _arg: *mut c_void) -> c_int {
    syscalls::set_errno(ENOSYS);
    -1
}

#[no_mangle]
pub unsafe extern "C" fn lseek(_fd: c_int, _offset: off_t, _whence: c_int) -> off_t {
    syscalls::set_errno(ENOSYS);
    -1
}

#[no_mangle]
pub unsafe extern "C" fn getpid() -> pid_t {
    1
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
    let mut tv = timeval { tv_sec: 0, tv_usec: 0 };
    if unsafe { gettimeofday(&mut tv, core::ptr::null_mut()) } != 0 {
        return -1;
    }
    unsafe {
        (*tp).tv_sec = tv.tv_sec;
        (*tp).tv_nsec = tv.tv_usec.saturating_mul(1000);
    }
    0
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
    pub unsafe fn readlink(path: *const c_char, buf: *mut c_char, bufsiz: size_t) -> ssize_t;
    pub unsafe fn fchmod(fd: c_int, mode: mode_t) -> c_int;
    pub unsafe fn chmod(path: *const c_char, mode: mode_t) -> c_int;
    pub unsafe fn fchown(fd: c_int, owner: uid_t, group: gid_t) -> c_int;
    pub unsafe fn chown(path: *const c_char, owner: uid_t, group: gid_t) -> c_int;
    pub unsafe fn utimensat(dirfd: c_int, path: *const c_char, times: *const timespec, flags: c_int) -> c_int;
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
    pub unsafe fn kill(pid: pid_t, sig: c_int) -> c_int;
    pub unsafe fn raise(sig: c_int) -> c_int;
    pub unsafe fn sigaction(sig: c_int, act: *const c_void, oldact: *mut c_void) -> c_int;
    pub unsafe fn sigprocmask(how: c_int, set: *const sigset_t, oldset: *mut sigset_t) -> c_int;
    pub unsafe fn execve(path: *const c_char, argv: *const *const c_char, envp: *const *const c_char) -> c_int;
    pub unsafe fn execvp(file: *const c_char, argv: *const *const c_char) -> c_int;
    pub unsafe fn _exit(status: c_int) -> c_int;
    pub unsafe fn nanosleep(req: *const timespec, rem: *mut timespec) -> c_int;
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

include!("rustix_compat.rs");
