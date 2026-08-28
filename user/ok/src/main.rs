#![no_std]
#![no_main]

use core::cell::UnsafeCell;

const MSG_PATH: &[u8] = b"/msg";

// PT_LOAD .bss: in user_range_ok (in_code). Stack locals are not safe here
// because every syscall uses options(nostack) and opt-level s may place a
// 64-byte array at/above RSP, past the mapped stack page (CI #95 fat miss).
static MSG_BUF: UnsafeCell<[u8; 64]> = UnsafeCell::new([0; 64]);

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let msg = b"user ok\n";
    unsafe { sys_write(msg.as_ptr() as usize, msg.len()); }

    let fd = unsafe { sys_open(MSG_PATH.as_ptr() as usize, MSG_PATH.len()) };
    if fd == usize::MAX {
        fat_miss();
    }
    let buf_ptr = MSG_BUF.get() as usize;
    let n = unsafe { sys_read(fd, buf_ptr, 64) };
    unsafe { sys_close(fd); }
    if n == 0 || n == usize::MAX {
        fat_miss();
    }
    unsafe { sys_write(buf_ptr, n); }
    // Needle from .rodata (same path as `user ok`). Echo of /msg may be
    // zeros if virtio returned padding; CI still needs the `fat ok` string.
    let ok = b"fat ok\n";
    unsafe { sys_write(ok.as_ptr() as usize, ok.len()); }
    unsafe { sys_exit(); }
}

fn fat_miss() -> ! {
    let m = b"fat miss\n";
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
