#![no_std]

pub mod alloc;
pub mod args;
pub mod runtime;

pub use alloc::Heap;
pub use args::{arg, argc};

/// x86 `_start`: naked entry reads argc/argv from the stack (same as std `pal/myos`).
#[macro_export]
macro_rules! x86_start {
    ($main:ident) => {
        #[cfg(target_arch = "x86_64")]
        #[unsafe(naked)]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn _start() -> ! {
            core::arch::naked_asm!(
                "mov rdi, [rsp]",
                "lea rsi, [rsp + 8]",
                "call {init}",
                "call {main}",
                init = sym $crate::args::init_argv_sysv,
                main = sym $main,
            );
        }
    };
}

const MAX_EXEC_ARGS: usize = 16;

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

/// Exec with argv. `args` are the argument strings (argv[0] is usually the command name).
pub fn exec(path: &[u8], args: &[&[u8]]) {
    let mut pack = [0usize; 1 + MAX_EXEC_ARGS * 2];
    pack[0] = args.len().min(MAX_EXEC_ARGS);
    for (i, a) in args.iter().take(MAX_EXEC_ARGS).enumerate() {
        pack[1 + i * 2] = a.as_ptr() as usize;
        pack[2 + i * 2] = a.len();
    }
    unsafe {
        sys_exec(
            path.as_ptr() as usize,
            path.len(),
            if args.is_empty() {
                0
            } else {
                pack.as_ptr() as usize
            },
        )
    }
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

/// List bootfs entries (newline-separated) into `buf`. Returns byte count.
pub fn listdir(buf: &mut [u8]) -> usize {
    unsafe { sys_listdir(buf.as_mut_ptr() as usize, buf.len()) }
}

/// Adjust the program break. `addr == 0` queries the current break.
pub fn brk(addr: usize) -> usize {
    unsafe { sys_brk(addr) }
}

/// Seed [`Heap`] from the current program break.
pub fn heap_init() {
    alloc::heap_init();
}

/// Read a line from stdin (fd 0), including the trailing `\n` if present.
pub fn read_line(buf: &mut [u8]) -> usize {
    let mut tmp = [0u8; 128];
    let mut n = 0usize;
    loop {
        let mut b = [0u8; 1];
        let r = read(0, &mut b);
        if r == usize::MAX || r == 0 {
            break;
        }
        let ch = b[0];
        if ch == 0x08 || ch == 127 {
            if n > 0 {
                n -= 1;
            }
            continue;
        }
        if n < tmp.len() {
            tmp[n] = ch;
            n += 1;
        }
        if ch == b'\n' || ch == b'\r' {
            break;
        }
    }
    let out = n.min(buf.len());
    buf[..out].copy_from_slice(&tmp[..out]);
    out
}

/// Print a short panic marker to serial (fd 1) then exit. Use as `#[panic_handler]`.
pub fn panic_die(info: &core::panic::PanicInfo) -> ! {
    write(b"user panic");
    if let Some(loc) = info.location() {
        write(b" at ");
        write(loc.file().as_bytes());
        write(b":");
        write_u32(loc.line());
    }
    write(b"\n");
    exit();
}

fn write_u32(mut n: u32) {
    if n == 0 {
        write(b"0");
        return;
    }
    let mut buf = [0u8; 10];
    let mut len = 0usize;
    while n > 0 {
        buf[len] = b'0' + (n % 10) as u8;
        n /= 10;
        len += 1;
    }
    while len > 0 {
        len -= 1;
        write(&[buf[len]]);
    }
}

// x86 syscall_entry clobbers rdi/rsi/rdx when shuffling args into the
// System-V dispatch. Wrappers lateout those so LLVM reloads them.

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
unsafe fn sys_exec(ptr: usize, len: usize, args: usize) {
    core::arch::asm!(
        "syscall",
        in("rax") 5usize,
        in("rdi") ptr,
        in("rsi") len,
        in("rdx") args,
        out("rcx") _,
        out("r11") _,
        lateout("rdi") _,
        lateout("rsi") _,
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

#[cfg(target_arch = "x86_64")]
unsafe fn sys_listdir(buf: usize, len: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "syscall",
        in("rax") 8usize,
        in("rdi") buf,
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
unsafe fn sys_brk(addr: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "syscall",
        in("rax") 9usize,
        in("rdi") addr,
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
unsafe fn sys_exec(ptr: usize, len: usize, args: usize) {
    core::arch::asm!(
        "svc #0",
        in("x8") 5usize,
        in("x0") ptr,
        in("x1") len,
        in("x2") args,
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

#[cfg(target_arch = "aarch64")]
unsafe fn sys_listdir(buf: usize, len: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "svc #0",
        in("x8") 8usize,
        inout("x0") buf => ret,
        in("x1") len,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "aarch64")]
unsafe fn sys_brk(addr: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "svc #0",
        in("x8") 9usize,
        inout("x0") addr => ret,
        options(nostack),
    );
    ret
}
