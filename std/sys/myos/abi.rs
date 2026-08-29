//! Raw myos syscalls (matches `user/lib` numbering).

pub const SYS_WRITE: usize = 0;
pub const SYS_EXIT: usize = 1;
pub const SYS_OPEN: usize = 2;
pub const SYS_READ: usize = 3;
pub const SYS_CLOSE: usize = 4;
pub const SYS_EXEC: usize = 5;
pub const SYS_FORK: usize = 6;
pub const SYS_WAIT: usize = 7;
pub const SYS_LISTDIR: usize = 8;
pub const SYS_BRK: usize = 9;
pub const SYS_PIPE: usize = 10;
pub const SYS_DUP2: usize = 11;

const MAX_EXEC_ARGS: usize = 16;

pub const STDIN_FILENO: i32 = 0;
pub const STDOUT_FILENO: i32 = 1;
pub const STDERR_FILENO: i32 = 2;

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

#[inline]
pub fn write(fd: i32, buf: &[u8]) -> isize {
    let ret = raw_write(fd as usize, buf.as_ptr() as usize, buf.len());
    if ret == usize::MAX {
        -1
    } else {
        ret as isize
    }
}

#[inline]
pub fn read(fd: i32, buf: &mut [u8]) -> isize {
    let ret = raw_read(fd as usize, buf.as_mut_ptr() as usize, buf.len());
    if ret == usize::MAX {
        -1
    } else {
        ret as isize
    }
}

#[inline]
pub fn brk(addr: usize) -> usize {
    raw_brk(addr)
}

#[inline]
pub fn exit(code: i32) -> ! {
    raw_exit(code as usize);
}

#[inline]
pub fn isatty(_fd: i32) -> bool {
    false
}

#[inline]
pub fn open(path: &[u8]) -> isize {
    let ret = raw_open(path.as_ptr() as usize, path.len());
    if ret == usize::MAX {
        -1
    } else {
        ret as isize
    }
}

/// Replace the current process image. Does not return on success.
#[inline]
pub fn exec(path: &[u8], args: &[&[u8]]) -> ! {
    let mut pack = [0usize; 1 + MAX_EXEC_ARGS * 2];
    pack[0] = args.len().min(MAX_EXEC_ARGS);
    for (i, arg) in args.iter().take(MAX_EXEC_ARGS).enumerate() {
        pack[1 + i * 2] = arg.as_ptr() as usize;
        pack[2 + i * 2] = arg.len();
    }
    raw_exec(
        path.as_ptr() as usize,
        path.len(),
        if args.is_empty() {
            0
        } else {
            pack.as_ptr() as usize
        },
    );
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
fn raw_open(ptr: usize, len: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_OPEN,
            in("rdi") ptr,
            in("rsi") len,
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
fn raw_open(ptr: usize, len: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") SYS_OPEN,
            in("x0") ptr,
            in("x1") len,
            lateout("x0") ret,
            options(nostack),
        );
    }
    ret
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

#[cfg(target_arch = "x86_64")]
#[inline]
fn raw_exec(path: usize, path_len: usize, args: usize) -> ! {
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_EXEC,
            in("rdi") path,
            in("rsi") path_len,
            in("rdx") args,
            options(noreturn),
        );
        core::hint::unreachable_unchecked();
    }
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn raw_exec(path: usize, path_len: usize, args: usize) -> ! {
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") SYS_EXEC,
            in("x0") path,
            in("x1") path_len,
            in("x2") args,
            options(noreturn),
        );
        core::hint::unreachable_unchecked();
    }
}
