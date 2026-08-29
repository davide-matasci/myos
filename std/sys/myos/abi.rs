//! Raw myos syscalls (matches `user/lib` numbering).

pub const SYS_WRITE: usize = 0;
pub const SYS_EXIT: usize = 1;
pub const SYS_OPEN: usize = 2;
pub const SYS_READ: usize = 3;
pub const SYS_CLOSE: usize = 4;
pub const SYS_BRK: usize = 9;

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
pub fn write(_fd: i32, buf: &[u8]) -> isize {
    let ret = raw_write(buf.as_ptr() as usize, buf.len());
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
fn raw_write(ptr: usize, len: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_WRITE,
            in("rdi") ptr,
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
fn raw_write(ptr: usize, len: usize) -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") SYS_WRITE,
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
