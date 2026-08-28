#![no_std]

pub fn write(buf: &[u8]) {
    unsafe { sys_write(buf.as_ptr() as usize, buf.len()) }
}

pub fn exit() -> ! {
    unsafe { sys_exit() }
}

pub fn open(path: &[u8]) -> Option<usize> {
    let fd = unsafe { sys_open(path.as_ptr() as usize, path.len()) };
    if fd == usize::MAX {
        None
    } else {
        Some(fd)
    }
}

pub fn read(fd: usize, buf: &mut [u8]) -> usize {
    unsafe { sys_read(fd, buf.as_mut_ptr() as usize, buf.len()) }
}

pub fn close(fd: usize) {
    unsafe { sys_close(fd) }
}

pub fn exec(path: &[u8]) {
    unsafe { sys_exec(path.as_ptr() as usize, path.len()) }
}

pub fn fork() -> Option<usize> {
    let pid = unsafe { sys_fork() };
    if pid == usize::MAX {
        None
    } else {
        Some(pid)
    }
}

pub fn wait() -> Option<usize> {
    let pid = unsafe { sys_wait() };
    if pid == usize::MAX {
        None
    } else {
        Some(pid)
    }
}

// x86 syscall_entry clobbers rdi/rsi/rdx when shuffling args into the
// System-V dispatch. Wrappers lateout those so LLVM reloads them.
// sys_read uses inout("rdx") len plus lateout rdi/rsi so the length is
// not left 0 after the syscall.

#[cfg(target_arch = "x86_64")]
unsafe fn sys_write(ptr: usize, len: usize) {
    core::arch::asm!(
        "syscall",
        in("rax") 0usize,
        in("rdi") ptr,
        in("rsi") len,
        out("rcx") _,
        out("r11") _,
        lateout("rdi") _,
        lateout("rsi") _,
        lateout("rdx") _,
        options(nostack),
    );
}

#[cfg(target_arch = "x86_64")]
unsafe fn sys_exit() -> ! {
    core::arch::asm!(
        "syscall",
        in("rax") 1usize,
        options(noreturn, nostack),
    );
}

#[cfg(target_arch = "x86_64")]
unsafe fn sys_open(ptr: usize, len: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "syscall",
        in("rax") 2usize,
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
    ret
}

#[cfg(target_arch = "x86_64")]
unsafe fn sys_read(fd: usize, buf: usize, len: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "syscall",
        in("rax") 3usize,
        in("rdi") fd,
        in("rsi") buf,
        inout("rdx") len => _,
        lateout("rax") ret,
        out("rcx") _,
        out("r11") _,
        lateout("rdi") _,
        lateout("rsi") _,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "x86_64")]
unsafe fn sys_close(fd: usize) {
    core::arch::asm!(
        "syscall",
        in("rax") 4usize,
        in("rdi") fd,
        out("rcx") _,
        out("r11") _,
        lateout("rdi") _,
        lateout("rsi") _,
        lateout("rdx") _,
        options(nostack),
    );
}

#[cfg(target_arch = "x86_64")]
unsafe fn sys_exec(ptr: usize, len: usize) {
    core::arch::asm!(
        "syscall",
        in("rax") 5usize,
        in("rdi") ptr,
        in("rsi") len,
        out("rcx") _,
        out("r11") _,
        lateout("rdi") _,
        lateout("rsi") _,
        lateout("rdx") _,
        options(nostack),
    );
}

#[cfg(target_arch = "x86_64")]
unsafe fn sys_fork() -> usize {
    let ret: usize;
    core::arch::asm!(
        "syscall",
        in("rax") 6usize,
        lateout("rax") ret,
        out("rcx") _,
        out("r11") _,
        lateout("rdi") _,
        lateout("rsi") _,
        lateout("rdx") _,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "x86_64")]
unsafe fn sys_wait() -> usize {
    let ret: usize;
    core::arch::asm!(
        "syscall",
        in("rax") 7usize,
        lateout("rax") ret,
        out("rcx") _,
        out("r11") _,
        lateout("rdi") _,
        lateout("rsi") _,
        lateout("rdx") _,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "aarch64")]
unsafe fn sys_write(ptr: usize, len: usize) {
    core::arch::asm!(
        "svc #0",
        in("x8") 0usize,
        in("x0") ptr,
        in("x1") len,
        options(nostack),
    );
}

#[cfg(target_arch = "aarch64")]
unsafe fn sys_exit() -> ! {
    core::arch::asm!(
        "svc #0",
        in("x8") 1usize,
        options(noreturn, nostack),
    );
}

#[cfg(target_arch = "aarch64")]
unsafe fn sys_open(ptr: usize, len: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "svc #0",
        in("x8") 2usize,
        inout("x0") ptr => ret,
        in("x1") len,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "aarch64")]
unsafe fn sys_read(fd: usize, buf: usize, len: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "svc #0",
        in("x8") 3usize,
        inout("x0") fd => ret,
        in("x1") buf,
        in("x2") len,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "aarch64")]
unsafe fn sys_close(fd: usize) {
    core::arch::asm!(
        "svc #0",
        in("x8") 4usize,
        in("x0") fd,
        options(nostack),
    );
}

#[cfg(target_arch = "aarch64")]
unsafe fn sys_exec(ptr: usize, len: usize) {
    core::arch::asm!(
        "svc #0",
        in("x8") 5usize,
        in("x0") ptr,
        in("x1") len,
        options(nostack),
    );
}

#[cfg(target_arch = "aarch64")]
unsafe fn sys_fork() -> usize {
    let ret: usize;
    core::arch::asm!(
        "svc #0",
        in("x8") 6usize,
        lateout("x0") ret,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "aarch64")]
unsafe fn sys_wait() -> usize {
    let ret: usize;
    core::arch::asm!(
        "svc #0",
        in("x8") 7usize,
        lateout("x0") ret,
        options(nostack),
    );
    ret
}
