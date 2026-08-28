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
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_CLOSE,
            in("rdi") fd as usize,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    if ret == usize::MAX {
        -1
    } else {
        0
    }
}

#[inline]
pub fn write(fd: i32, buf: &[u8]) -> isize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_WRITE,
            in("rdi") fd as usize,
            in("rsi") buf.as_ptr() as usize,
            in("rdx") buf.len(),
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    if ret == usize::MAX {
        -1
    } else {
        ret as isize
    }
}

#[inline]
pub fn read(fd: i32, buf: &mut [u8]) -> isize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_READ,
            in("rdi") fd as usize,
            in("rsi") buf.as_mut_ptr() as usize,
            in("rdx") buf.len(),
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    if ret == usize::MAX {
        -1
    } else {
        ret as isize
    }
}

#[inline]
pub fn brk(addr: usize) -> usize {
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

#[inline]
pub fn exit(code: i32) -> ! {
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") SYS_EXIT,
            in("rdi") code as usize,
            options(noreturn),
        );
    }
}

#[inline]
pub fn isatty(_fd: i32) -> bool {
    false
}
