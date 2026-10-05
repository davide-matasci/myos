//! Raw myos syscalls (matches `user/lib` numbering).

pub const SYS_WRITE: usize = 0;
pub const SYS_EXIT: usize = 1;
pub const SYS_READ: usize = 3;
pub const SYS_CLOSE: usize = 4;
pub const SYS_FORK: usize = 6;
pub const SYS_WAIT: usize = 7;
pub const SYS_BRK: usize = 9;
pub const SYS_PIPE: usize = 10;
pub const SYS_DUP2: usize = 11;
pub const SYS_EXECNAME: usize = 13;
pub const SYS_DUPFD: usize = 14;
pub const SYS_GETCWD: usize = 16;
pub const SYS_LSEEK: usize = 26;
pub const SYS_GETTIMEOFDAY: usize = 33;
pub const SYS_NANOSLEEP: usize = 52;
/// The path calls (`kernel/src/user/at.rs`): a directory fd ([`AT_FDCWD`]:
/// the cwd) and a path `(ptr, len)` relative to it; [`AT_EMPTY_PATH`] with
/// an empty path is the fd's own file.
pub const SYS_OPENAT: usize = 70;
pub const SYS_STATAT: usize = 71;
pub const SYS_MKNODAT: usize = 72;
pub const SYS_SYMLINKAT: usize = 73;
pub const SYS_UNLINKAT: usize = 74;
pub const SYS_RENAMEAT: usize = 75;
pub const SYS_READLINKAT: usize = 76;
pub const SYS_UTIMENSAT: usize = 77;
pub const SYS_CHDIRAT: usize = 78;
pub const SYS_LISTDIRAT: usize = 79;
pub const SYS_EXECAT: usize = 80;
pub const AT_FDCWD: usize = -100isize as usize;
pub const AT_SYMLINK_NOFOLLOW: usize = 0x100;
pub const AT_REMOVEDIR: usize = 0x200;
pub const AT_EMPTY_PATH: usize = 0x1000;
pub const MKNOD_DIR: usize = 0;
/// `utimens` / `futimens` time values: now, or leave the time as it is.
pub const UTIME_NOW: i64 = -1;
pub const UTIME_OMIT: i64 = -2;

pub const STDIN_FILENO: i32 = 0;
pub const STDOUT_FILENO: i32 = 1;
pub const STDERR_FILENO: i32 = 2;

/// `open` flags on top of the access mode (0 read, 1 write, 2 both), as
/// libgloss passes them (`syscalls.c`).
pub const O_WRONLY: usize = 1;
pub const O_RDWR: usize = 2;
pub const O_CREAT: usize = 0x40;
pub const O_TRUNC: usize = 0x200;
pub const O_APPEND: usize = 0x400;

pub const EBADF: i32 = 9;
pub const F_DUPFD_CLOEXEC: i32 = 1030;

#[inline]
pub fn close(fd: i32) -> isize {
    let ret = raw_close(fd as usize);
    if ret == usize::MAX {
        -1
    } else {
        0
    }
}

/// A read's or write's result: the count, or minus an errno for the
/// kernel's failure values (`SYSERR` and the distinct EIO, ENXIO and EINTR,
/// `kernel/src/task/fd.rs`), so `cvt` reports what happened.
#[inline]
fn io_result(ret: usize) -> isize {
    match usize::MAX - ret {
        0 => -1,
        1 => -5, // EIO: the pty's other end is gone
        2 => -6, // ENXIO
        3 => -4, // EINTR: a caught signal
        _ => ret as isize,
    }
}

#[inline]
pub fn write(fd: i32, buf: &[u8]) -> isize {
    io_result(raw_write(fd as usize, buf.as_ptr() as usize, buf.len()))
}

#[inline]
pub fn read(fd: i32, buf: &mut [u8]) -> isize {
    io_result(raw_read(fd as usize, buf.as_mut_ptr() as usize, buf.len()))
}

#[inline]
pub fn brk(addr: usize) -> usize {
    raw_brk(addr)
}

#[inline]
pub fn exec_name(buf: &mut [u8]) -> usize {
    let ret = raw_exec_name(buf.as_mut_ptr() as usize, buf.len());
    if ret == usize::MAX {
        0
    } else {
        ret
    }
}

#[inline]
pub fn exit(code: i32) -> ! {
    raw_exit(code as usize);
}

#[inline]
pub fn isatty(_fd: i32) -> bool {
    false
}

/// Block for `dur` (`SYS_NANOSLEEP`, no flags: no early wake on events).
/// `false` when a signal cut the sleep short.
#[inline]
pub fn nanosleep(dur: crate::time::Duration) -> bool {
    let ns = u64::try_from(dur.as_nanos()).unwrap_or(u64::MAX);
    raw_nanosleep(ns as usize, 0) == 0
}


/// Kernel `MyosStat`, written by `statat`. Times are seconds since the
/// epoch, 0 where the filesystem keeps none.
#[repr(C)]
#[derive(Default)]
pub struct StatBuf {
    pub st_mode: u32,
    pub st_nlink: u32,
    pub st_ino: u32,
    pub st_dev: u32,
    pub st_size: u64,
    pub st_atime: i64,
    pub st_mtime: i64,
    pub st_uid: u32,
    pub st_gid: u32,
}

fn ok(ret: usize) -> isize {
    if ret == usize::MAX { -1 } else { ret as isize }
}

/// An fd on `path` with [`O_WRONLY`], [`O_CREAT`], ... flags.
#[inline]
pub fn openat(dirfd: usize, path: &[u8], flags: usize) -> isize {
    ok(raw_syscall6(SYS_OPENAT, dirfd, path.as_ptr() as usize, path.len(), flags, 0, 0))
}

/// `stat` of `path` relative to `dirfd` ([`AT_SYMLINK_NOFOLLOW`]: of a
/// symlink itself; [`AT_EMPTY_PATH`] and no path: of the fd).
#[inline]
pub fn statat(dirfd: usize, path: &[u8], flags: usize, out: &mut StatBuf) -> isize {
    ok(raw_syscall6(SYS_STATAT, dirfd, path.as_ptr() as usize, path.len(), flags, out as *mut StatBuf as usize, 0))
}

/// A new directory ([`MKNOD_DIR`]) at `path`.
#[inline]
pub fn mknodat(path: &[u8], kind: usize) -> isize {
    ok(raw_syscall6(SYS_MKNODAT, AT_FDCWD, path.as_ptr() as usize, path.len(), kind, 0, 0))
}

/// Remove `path` ([`AT_REMOVEDIR`]: an empty directory).
#[inline]
pub fn unlinkat(path: &[u8], flags: usize) -> isize {
    ok(raw_syscall6(SYS_UNLINKAT, AT_FDCWD, path.as_ptr() as usize, path.len(), flags, 0, 0))
}

#[inline]
pub fn renameat(old: &[u8], new: &[u8]) -> isize {
    ok(raw_syscall6(SYS_RENAMEAT, AT_FDCWD, old.as_ptr() as usize, old.len(), AT_FDCWD, new.as_ptr() as usize, new.len()))
}

/// A symlink at `link` holding `target`.
#[inline]
pub fn symlinkat(target: &[u8], link: &[u8]) -> isize {
    ok(raw_syscall6(SYS_SYMLINKAT, target.as_ptr() as usize, target.len(), AT_FDCWD, link.as_ptr() as usize, link.len(), 0))
}

/// The link's target into `buf`: its length, or -1.
#[inline]
pub fn readlinkat(path: &[u8], buf: &mut [u8]) -> isize {
    ok(raw_syscall6(SYS_READLINKAT, AT_FDCWD, path.as_ptr() as usize, path.len(), buf.as_mut_ptr() as usize, buf.len(), 0))
}

/// Set the access and modification times (seconds, [`UTIME_NOW`] or
/// [`UTIME_OMIT`]) of `path` relative to `dirfd`, with `statat`'s flags.
#[inline]
pub fn utimensat(dirfd: usize, path: &[u8], times: &[i64; 2], flags: usize) -> isize {
    ok(raw_syscall6(SYS_UTIMENSAT, dirfd, path.as_ptr() as usize, path.len(), times.as_ptr() as usize, flags, 0))
}

/// Make the directory at `path` the cwd.
#[inline]
pub fn chdirat(path: &[u8]) -> isize {
    ok(raw_syscall6(SYS_CHDIRAT, AT_FDCWD, path.as_ptr() as usize, path.len(), 0, 0, 0))
}

/// The cwd into `buf` (with a NUL after it): its length, or -1.
#[inline]
pub fn getcwd(buf: &mut [u8]) -> isize {
    ok(raw_syscall3(SYS_GETCWD, buf.as_mut_ptr() as usize, buf.len(), 0))
}

/// The names in the directory at `path`, one per line, into `buf`: the
/// bytes written (all of `buf`: there may be more), or -1.
#[inline]
pub fn listdirat(path: &[u8], buf: &mut [u8]) -> isize {
    ok(raw_syscall6(SYS_LISTDIRAT, AT_FDCWD, path.as_ptr() as usize, path.len(), buf.as_mut_ptr() as usize, buf.len(), 0))
}

/// Replace the current process image with the program at `path`. Does not
/// return on success.
#[inline]
pub fn exec(path: &[u8], args: &[&[u8]]) -> ! {
    // `[argc, (ptr,len)…, envc, (ptr,len)…]`; the kernel enforces its limits
    // (and fails the exec past them).
    let mut pack = crate::vec::Vec::with_capacity(2 + 2 * args.len());
    pack.push(args.len());
    for s in args {
        pack.push(s.as_ptr() as usize);
        pack.push(s.len());
    }
    pack.push(0);
    raw_syscall6(SYS_EXECAT, AT_FDCWD, path.as_ptr() as usize, path.len(), pack.as_ptr() as usize, 0, 0);
    exit(127);
}

/// The new offset, or -1 (a pipe or a terminal has none).
#[inline]
pub fn lseek(fd: i32, offset: i64, whence: usize) -> i64 {
    let ret = raw_syscall3(SYS_LSEEK, fd as usize, offset as usize, whence);
    if ret == usize::MAX { -1 } else { ret as i64 }
}

/// Parent: child pid. Child: `0`. Error: `-1`.
#[inline]
pub fn fork() -> isize {
    let ret = raw_fork();
    if ret == usize::MAX {
        -1
    } else {
        ret as isize
    }
}

#[inline]
pub fn wait_status(status: &mut u8) -> isize {
    let ret = raw_wait(status as *mut u8 as usize);
    if ret == usize::MAX {
        -1
    } else {
        ret as isize
    }
}

#[inline]
pub fn pipe(fds: &mut [usize; 2]) -> isize {
    let ret = raw_pipe(fds.as_mut_ptr() as usize);
    if ret == usize::MAX {
        -1
    } else {
        0
    }
}

#[inline]
pub fn dup2(oldfd: i32, newfd: i32) -> isize {
    let ret = raw_dup2(oldfd as usize, newfd as usize);
    if ret == usize::MAX {
        -1
    } else {
        0
    }
}

#[inline]
pub fn wait() -> isize {
    let ret = raw_wait(0);
    if ret == usize::MAX {
        -1
    } else {
        ret as isize
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
fn raw_close(fd: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_CLOSE,
            in("rdi") fd,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn raw_close(fd: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") SYS_CLOSE,
            in("x0") fd,
            lateout("x0") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "x86_64")]
#[inline]
fn raw_write(fd: usize, ptr: usize, len: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_WRITE,
            in("rdi") fd,
            in("rsi") ptr,
            in("rdx") len,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn raw_write(fd: usize, ptr: usize, len: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") SYS_WRITE,
            in("x0") fd,
            in("x1") ptr,
            in("x2") len,
            lateout("x0") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "x86_64")]
#[inline]
fn raw_read(fd: usize, buf: usize, len: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_READ,
            in("rdi") fd,
            in("rsi") buf,
            in("rdx") len,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn raw_read(fd: usize, buf: usize, len: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") SYS_READ,
            in("x0") fd,
            in("x1") buf,
            in("x2") len,
            lateout("x0") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "x86_64")]
#[inline]
fn raw_brk(addr: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_BRK,
            in("rdi") addr,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn raw_brk(addr: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") SYS_BRK,
            in("x0") addr,
            lateout("x0") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "x86_64")]
#[inline]
fn raw_exec_name(buf: usize, len: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_EXECNAME,
            in("rdi") buf,
            in("rsi") len,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn raw_exec_name(buf: usize, len: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") SYS_EXECNAME,
            in("x0") buf,
            in("x1") len,
            lateout("x0") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "x86_64")]
#[inline]
fn raw_exit(code: usize) -> ! {
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_EXIT,
            in("rdi") code,
            options(noreturn),
        );
    }
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn raw_exit(code: usize) -> ! {
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") SYS_EXIT,
            in("x0") code,
            options(noreturn),
        );
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
fn raw_fork() -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_FORK,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            lateout("rdi") _,
            lateout("rsi") _,
            lateout("rdx") _,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn raw_fork() -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") SYS_FORK,
            lateout("x0") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "x86_64")]
#[inline]
fn raw_wait(status_ptr: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_WAIT,
            in("rdi") status_ptr,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn raw_wait(status_ptr: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") SYS_WAIT,
            in("x0") status_ptr,
            lateout("x0") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "x86_64")]
#[inline]
fn raw_pipe(fds_ptr: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_PIPE,
            in("rdi") fds_ptr,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn raw_pipe(fds_ptr: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") SYS_PIPE,
            in("x0") fds_ptr,
            lateout("x0") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "x86_64")]
#[inline]
fn raw_dup2(oldfd: usize, newfd: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_DUP2,
            in("rdi") oldfd,
            in("rsi") newfd,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn raw_dup2(oldfd: usize, newfd: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") SYS_DUP2,
            in("x0") oldfd,
            in("x1") newfd,
            lateout("x0") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn raw_close(fd: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") SYS_CLOSE,
            in("a0") fd,
            lateout("a0") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn raw_write(fd: usize, ptr: usize, len: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") SYS_WRITE,
            in("a0") fd,
            in("a1") ptr,
            in("a2") len,
            lateout("a0") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn raw_read(fd: usize, buf: usize, len: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") SYS_READ,
            in("a0") fd,
            in("a1") buf,
            in("a2") len,
            lateout("a0") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn raw_brk(addr: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") SYS_BRK,
            inout("a0") addr => ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn raw_exec_name(buf: usize, len: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") SYS_EXECNAME,
            in("a0") buf,
            in("a1") len,
            lateout("a0") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn raw_exit(code: usize) -> ! {
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") SYS_EXIT,
            in("a0") code,
            options(noreturn),
        );
    }
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn raw_fork() -> usize {
    unsafe extern "C" {
        fn sys_fork_raw() -> usize;
    }
    unsafe { sys_fork_raw() }
}

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    r#"
    .section .text.sys_fork_raw,"ax",@progbits
    .global sys_fork_raw
    .type sys_fork_raw, @function
sys_fork_raw:
    li a7, 6
    ecall
    ret
"#
);

#[cfg(target_arch = "riscv64")]
#[inline]
fn raw_wait(status_ptr: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") SYS_WAIT,
            inout("a0") status_ptr => ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn raw_pipe(fds_ptr: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") SYS_PIPE,
            inout("a0") fds_ptr => ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn raw_dup2(oldfd: usize, newfd: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") SYS_DUP2,
            in("a0") oldfd,
            in("a1") newfd,
            lateout("a0") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "x86_64")]
#[inline]
fn raw_nanosleep(ns: usize, flags: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_NANOSLEEP,
            in("rdi") ns,
            in("rsi") flags,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn raw_nanosleep(ns: usize, flags: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") SYS_NANOSLEEP,
            in("x0") ns,
            in("x1") flags,
            lateout("x0") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn raw_nanosleep(ns: usize, flags: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") SYS_NANOSLEEP,
            in("a0") ns,
            in("a1") flags,
            lateout("a0") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "x86_64")]
#[inline]
fn raw_syscall3(nr: usize, a0: usize, a1: usize, a2: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") nr,
            in("rdi") a0,
            in("rsi") a1,
            in("rdx") a2,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            lateout("rdi") _,
            lateout("rsi") _,
            lateout("rdx") _,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn raw_syscall3(nr: usize, a0: usize, a1: usize, a2: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") nr,
            in("x0") a0,
            in("x1") a1,
            in("x2") a2,
            lateout("x0") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn raw_syscall3(nr: usize, a0: usize, a1: usize, a2: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") nr,
            in("a0") a0,
            in("a1") a1,
            in("a2") a2,
            lateout("a0") ret,
            options(nostack),
        );
    }
    ret
}


#[cfg(target_arch = "x86_64")]
#[inline]
fn raw_syscall6(nr: usize, a0: usize, a1: usize, a2: usize, a3: usize, a4: usize, a5: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") nr,
            in("rdi") a0,
            in("rsi") a1,
            in("rdx") a2,
            in("r10") a3,
            in("r8") a4,
            in("r9") a5,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn raw_syscall6(nr: usize, a0: usize, a1: usize, a2: usize, a3: usize, a4: usize, a5: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") nr,
            inlateout("x0") a0 => ret,
            in("x1") a1,
            in("x2") a2,
            in("x3") a3,
            in("x4") a4,
            in("x5") a5,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn raw_syscall6(nr: usize, a0: usize, a1: usize, a2: usize, a3: usize, a4: usize, a5: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") nr,
            inlateout("a0") a0 => ret,
            in("a1") a1,
            in("a2") a2,
            in("a3") a3,
            in("a4") a4,
            in("a5") a5,
            options(nostack),
        );
    }
    ret
}
