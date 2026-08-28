#![no_std]
#![no_main]

const MSG_PATH: &[u8] = b"/msg";

// PT_LOAD .bss: in user_range_ok (in_code). addr_of_mut so the pointer is
// RIP-relative, not a GOT slot that needs .rela.dyn (#97 UnsafeCell::get).
static mut MSG_BUF: [u8; 64] = [0; 64];

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let msg = b"user ok\n";
    unsafe { sys_write(msg.as_ptr() as usize, msg.len()); }

    let path = b"/msg";
    let fd = unsafe { sys_open(path.as_ptr() as usize, path.len()) };
    if fd == usize::MAX {
        miss(b"fat nofd\n");
    }
    let buf_ptr = core::ptr::addr_of_mut!(MSG_BUF) as *mut u8 as usize;
    let n = unsafe { sys_read(fd, buf_ptr, 64) };
    unsafe { sys_close(fd); }
    if n == usize::MAX {
        miss(b"fat nread\n");
    }
    if n == 0 {
        miss(b"fat empty\n");
    }
    unsafe { sys_write(buf_ptr, n); }
    let ok = b"fat ok\n";
    unsafe { sys_write(ok.as_ptr() as usize, ok.len()); }
    unsafe { sys_exit(); }
    let _ = MSG_PATH;
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
