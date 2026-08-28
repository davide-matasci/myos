#![no_std]
#![no_main]

// Keep the /msg buffer tiny and in `_start`, like user/init's 4-byte ELF
// mag read (that path already works on x86). A 64-byte array returned from
// an `inline(never)` helper page-faulted in ring 3 at RIP 0x800000142c
// right after `user ok` (CI #103).

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let msg = b"user ok\n";
    unsafe { sys_write(msg.as_ptr() as usize, msg.len()); }

    let path = b"/msg";
    let fd = unsafe { sys_open(path.as_ptr() as usize, path.len()) };
    if fd == usize::MAX {
        miss(b"fat nofd\n");
    }
    let mut buf = [0u8; 8];
    let n = unsafe { sys_read(fd, buf.as_mut_ptr() as usize, 8) };
    unsafe { sys_close(fd); }
    if n == usize::MAX {
        miss(b"fat nread\n");
    }
    if n == 0 {
        miss(b"fat empty\n");
    }
    unsafe { sys_write(buf.as_ptr() as usize, n); }
    let ok = b"fat ok\n";
    unsafe { sys_write(ok.as_ptr() as usize, ok.len()); }
    unsafe { sys_exit(); }
}

fn miss(m: &[u8]) -> ! {
    unsafe { sys_write(m.as_ptr() as usize, m.len()); }
    unsafe { sys_exit(); }
}

#[cfg(target_arch = "x86_64")]
unsafe fn sys_write(ptr: usize, len: usize) {
    core::arch::asm!(
        "syscall",
        in("rax") 0usize,
        in("rdi") ptr,
        in("rsi") len,
        out("rcx") _,
        out("r11") _,
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
        in("rdx") len,
        lateout("rax") ret,
        out("rcx") _,
        out("r11") _,
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
        options(nostack),
    );
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

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
