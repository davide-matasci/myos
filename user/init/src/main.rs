#![no_std]
#![no_main]

const PATH: &[u8] = b"/ok";
const ELF_MAG: [u8; 4] = [0x7f, b'E', b'L', b'F'];

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let fd = unsafe { sys_open(PATH.as_ptr() as usize, PATH.len()) };
    if fd == usize::MAX {
        spin();
    }
    let mut mag = [0u8; 4];
    let n = unsafe { sys_read(fd, mag.as_mut_ptr() as usize, 4) };
    if n != 4 || mag != ELF_MAG {
        spin();
    }
    unsafe { sys_close(fd); }
    unsafe { sys_exec(PATH.as_ptr() as usize, PATH.len()); }
    spin();
}

fn spin() -> ! {
    loop {}
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

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
