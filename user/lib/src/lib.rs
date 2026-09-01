#![no_std]

pub mod alloc;
pub mod args;
pub mod runtime;

pub use alloc::Heap;
pub use args::{arg, argc};

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
unsafe extern "C" {
    fn sys_fork_raw() -> usize;
}

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
const MAX_EXEC_ENV: usize = 32;

pub fn write(buf: &[u8]) {
    write_fd(1, buf);
}

pub fn write_fd(fd: usize, buf: &[u8]) -> usize {
    unsafe { sys_write(fd, buf.as_ptr() as usize, buf.len()) }
}

pub fn exit() -> ! {
    exit_code(0);
}

pub fn exit_code(code: u8) -> ! {
    unsafe { sys_exit(code as usize) }
}


pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const O_CREAT: u32 = 0o100;
pub const O_TRUNC: u32 = 0o1000;
pub const O_APPEND: u32 = 0o2000;

pub fn open(path: &[u8]) -> Option<usize> {
    open_flags(path, 0)
}

pub fn open_flags(path: &[u8], flags: u32) -> Option<usize> {
    let fd = unsafe { sys_open(path.as_ptr() as usize, path.len(), flags as usize) };
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
/// Returns on failure (command missing or invalid); does not return on success.
pub fn exec(path: &[u8], args: &[&[u8]]) {
    exec_env(path, args, &[]);
}

const MAX_EXEC_ARG_LEN: usize = 128;
const MAX_EXEC_ENV_LEN: usize = 128;

/// Slide ET_EXEC link VAs to the runtime user base (AArch64/RISC-V nested ELFs).
///
/// `aarch64-unknown-none` / `riscv64imac-unknown-none-elf` user programs are
/// ET_EXEC with no relocs; the kernel slides PT_LOAD as a unit to `0x4000_0000`.
/// ADR'd path refs are already correct, but `&[b"arg"]` fat pointers stored in
/// `.rodata` keep link VAs (`~0x0020_xxxx` / `~0x0001_xxxx`). Reading them
/// before this fixup causes an alignment/translation abort (CI FAR=`0x20016f`).
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[inline]
fn et_exec_fixup_ptr(ptr: usize) -> usize {
    const USER_BASE: usize = 0x4000_0000;
    if ptr == 0 || ptr >= USER_BASE {
        return ptr;
    }
    #[cfg(target_arch = "aarch64")]
    const LINK_BASE: usize = 0x0020_0000;
    #[cfg(target_arch = "riscv64")]
    const LINK_BASE: usize = 0x0001_0000;
    ptr.wrapping_sub(LINK_BASE).wrapping_add(USER_BASE)
}

#[cfg(target_arch = "x86_64")]
#[inline]
fn et_exec_fixup_ptr(ptr: usize) -> usize {
    ptr
}

#[inline]
fn copy_exec_bytes(dst: &mut [u8], src: &[u8]) -> usize {
    let n = src.len().min(dst.len());
    let src_ptr = et_exec_fixup_ptr(src.as_ptr() as usize) as *const u8;
    unsafe {
        core::ptr::copy_nonoverlapping(src_ptr, dst.as_mut_ptr(), n);
    }
    n
}

/// Like [`exec`], but passes a `KEY=value` environment block to the new image.
pub fn exec_env(path: &[u8], args: &[&[u8]], env: &[&[u8]]) {
    let argc = args.len().min(MAX_EXEC_ARGS);
    let envc = env.len().min(MAX_EXEC_ENV);
    let path_ptr = et_exec_fixup_ptr(path.as_ptr() as usize);
    if argc == 0 && envc == 0 {
        unsafe {
            sys_exec(path_ptr, path.len(), 0);
        }
        return;
    }
    // Nested ELFs may be ET_EXEC (no PIE): static slice pointers keep link-time
    // VAs. Fix them up, then copy onto the stack so the kernel sees user VAs.
    let mut arg_buf = [[0u8; MAX_EXEC_ARG_LEN]; MAX_EXEC_ARGS];
    let mut arg_ptrs = [0usize; MAX_EXEC_ARGS];
    let mut arg_lens = [0usize; MAX_EXEC_ARGS];
    for (i, a) in args.iter().take(MAX_EXEC_ARGS).enumerate() {
        let n = copy_exec_bytes(&mut arg_buf[i], a);
        arg_ptrs[i] = arg_buf[i].as_ptr() as usize;
        arg_lens[i] = n;
    }
    let mut env_buf = [[0u8; MAX_EXEC_ENV_LEN]; MAX_EXEC_ENV];
    let mut env_ptrs = [0usize; MAX_EXEC_ENV];
    let mut env_lens = [0usize; MAX_EXEC_ENV];
    for (i, e) in env.iter().take(MAX_EXEC_ENV).enumerate() {
        let n = copy_exec_bytes(&mut env_buf[i], e);
        env_ptrs[i] = env_buf[i].as_ptr() as usize;
        env_lens[i] = n;
    }
    let mut pack = [0usize; 1 + MAX_EXEC_ARGS * 2 + 1 + MAX_EXEC_ENV * 2];
    pack[0] = argc;
    for i in 0..argc {
        pack[1 + i * 2] = arg_ptrs[i];
        pack[2 + i * 2] = arg_lens[i];
    }
    let env_base = 1 + argc * 2;
    pack[env_base] = envc;
    for i in 0..envc {
        pack[env_base + 1 + i * 2] = env_ptrs[i];
        pack[env_base + 2 + i * 2] = env_lens[i];
    }
    unsafe {
        sys_exec(path_ptr, path.len(), pack.as_ptr() as usize);
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
    let pid = unsafe { sys_wait(0) };
    if pid == usize::MAX {
        None
    } else {
        Some(pid)
    }
}

/// Wait for a child and return `(pid, exit_code)`.
pub fn wait_status() -> Option<(usize, u8)> {
    let mut status = 0u8;
    let pid = unsafe { sys_wait(&mut status as *mut u8 as usize) };
    if pid == usize::MAX {
        None
    } else {
        Some((pid, status))
    }
}

pub fn pipe() -> Option<(usize, usize)> {
    let mut fds = [0usize; 2];
    let ret = unsafe { sys_pipe(fds.as_mut_ptr() as usize) };
    if ret == usize::MAX {
        None
    } else {
        Some((fds[0], fds[1]))
    }
}

pub fn dup2(oldfd: usize, newfd: usize) -> bool {
    unsafe { sys_dup2(oldfd, newfd) != usize::MAX }
}

/// Must match kernel `SYS_LISTDIR` / libgloss `MYOS_DIRBUF`.
pub const LISTDIR_BUF: usize = 4096;

/// List directory entries at `path` (newline-separated) into `buf`.
/// `buf` must hold at least [`LISTDIR_BUF`] bytes (kernel listdir cap).
pub fn listdir(path: &[u8], buf: &mut [u8]) -> usize {
    if buf.len() < LISTDIR_BUF {
        return usize::MAX;
    }
    unsafe {
        sys_listdir(
            path.as_ptr() as usize,
            path.len(),
            buf.as_mut_ptr() as usize,
        )
    }
}

fn pack_lens(a: usize, b: usize) -> usize {
    (a << 16) | b
}

pub fn mkdir(path: &[u8]) -> bool {
    unsafe { sys3(17, path.as_ptr() as usize, path.len(), 0o755) != usize::MAX }
}

pub fn rmdir(path: &[u8]) -> bool {
    unsafe { sys3(18, path.as_ptr() as usize, path.len(), 0) != usize::MAX }
}

pub fn unlink(path: &[u8]) -> bool {
    unsafe { sys3(19, path.as_ptr() as usize, path.len(), 0) != usize::MAX }
}

pub fn rename(old: &[u8], new: &[u8]) -> bool {
    let packed = pack_lens(old.len(), new.len());
    unsafe {
        sys3(20, old.as_ptr() as usize, new.as_ptr() as usize, packed) != usize::MAX
    }
}

pub fn symlink(target: &[u8], linkpath: &[u8]) -> bool {
    let packed = pack_lens(target.len(), linkpath.len());
    unsafe {
        sys3(
            21,
            target.as_ptr() as usize,
            linkpath.as_ptr() as usize,
            packed,
        ) != usize::MAX
    }
}

pub fn readlink(path: &[u8], buf: &mut [u8]) -> Option<usize> {
    let packed = pack_lens(path.len(), buf.len());
    let n = unsafe {
        sys3(
            22,
            path.as_ptr() as usize,
            buf.as_mut_ptr() as usize,
            packed,
        )
    };
    if n == usize::MAX {
        None
    } else {
        Some(n)
    }
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
unsafe fn sys3(nr: usize, a0: usize, a1: usize, a2: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "syscall",
        inout("rax") nr => ret,
        in("rdi") a0,
        in("rsi") a1,
        in("rdx") a2,
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
unsafe fn sys3(nr: usize, a0: usize, a1: usize, a2: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "svc #0",
        in("x8") nr,
        inout("x0") a0 => ret,
        in("x1") a1,
        in("x2") a2,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "riscv64")]
unsafe fn sys3(nr: usize, a0: usize, a1: usize, a2: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "ecall",
        in("a7") nr,
        inout("a0") a0 => ret,
        in("a1") a1,
        in("a2") a2,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "x86_64")]
unsafe fn sys_write(fd: usize, ptr: usize, len: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "syscall",
        inout("rax") 0usize => ret,
        in("rdi") fd,
        in("rsi") ptr,
        in("rdx") len,
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
unsafe fn sys_exit(code: usize) -> ! {
    core::arch::asm!(
        "syscall",
        in("rax") 1usize,
        in("rdi") code,
        options(noreturn, nostack),
    );
}

#[cfg(target_arch = "x86_64")]
unsafe fn sys_open(ptr: usize, len: usize, flags: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "syscall",
        in("rax") 2usize,
        in("rdi") ptr,
        in("rsi") len,
        in("rdx") flags,
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
        inout("rdx") args => _,
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
unsafe fn sys_wait(status_ptr: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "syscall",
        in("rax") 7usize,
        in("rdi") status_ptr,
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
unsafe fn sys_pipe(fds_ptr: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "syscall",
        in("rax") 10usize,
        in("rdi") fds_ptr,
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
unsafe fn sys_dup2(oldfd: usize, newfd: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "syscall",
        in("rax") 11usize,
        in("rdi") oldfd,
        in("rsi") newfd,
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
unsafe fn sys_listdir(path: usize, path_len: usize, buf: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "syscall",
        in("rax") 8usize,
        in("rdi") path,
        in("rsi") path_len,
        in("rdx") buf,
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
unsafe fn sys_write(fd: usize, ptr: usize, len: usize) -> usize {
    let mut fd = fd;
    core::arch::asm!(
        "svc #0",
        in("x8") 0usize,
        inout("x0") fd,
        in("x1") ptr,
        in("x2") len,
        options(nostack),
    );
    fd
}

#[cfg(target_arch = "aarch64")]
unsafe fn sys_exit(code: usize) -> ! {
    core::arch::asm!(
        "svc #0",
        in("x8") 1usize,
        in("x0") code,
        options(noreturn, nostack),
    );
}

#[cfg(target_arch = "aarch64")]
unsafe fn sys_open(ptr: usize, len: usize, flags: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "svc #0",
        in("x8") 2usize,
        inout("x0") ptr => ret,
        in("x1") len,
        in("x2") flags,
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
unsafe fn sys_wait(status_ptr: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "svc #0",
        in("x8") 7usize,
        in("x0") status_ptr,
        lateout("x0") ret,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "aarch64")]
unsafe fn sys_pipe(fds_ptr: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "svc #0",
        in("x8") 10usize,
        in("x0") fds_ptr,
        lateout("x0") ret,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "aarch64")]
unsafe fn sys_dup2(oldfd: usize, newfd: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "svc #0",
        in("x8") 11usize,
        in("x0") oldfd,
        in("x1") newfd,
        lateout("x0") ret,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "aarch64")]
unsafe fn sys_listdir(path: usize, path_len: usize, buf: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "svc #0",
        in("x8") 8usize,
        inout("x0") path => ret,
        in("x1") path_len,
        in("x2") buf,
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

#[cfg(target_arch = "riscv64")]
unsafe fn sys_write(fd: usize, ptr: usize, len: usize) -> usize {
    let mut fd = fd;
    core::arch::asm!(
        "ecall",
        in("a7") 0usize,
        inout("a0") fd,
        in("a1") ptr,
        in("a2") len,
        options(nostack),
    );
    fd
}

#[cfg(target_arch = "riscv64")]
unsafe fn sys_exit(code: usize) -> ! {
    core::arch::asm!(
        "ecall",
        in("a7") 1usize,
        in("a0") code,
        options(noreturn, nostack),
    );
}

#[cfg(target_arch = "riscv64")]
unsafe fn sys_open(ptr: usize, len: usize, flags: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "ecall",
        in("a7") 2usize,
        inout("a0") ptr => ret,
        in("a1") len,
        in("a2") flags,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "riscv64")]
unsafe fn sys_read(fd: usize, buf: usize, len: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "ecall",
        in("a7") 3usize,
        inout("a0") fd => ret,
        in("a1") buf,
        in("a2") len,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "riscv64")]
unsafe fn sys_close(fd: usize) {
    core::arch::asm!(
        "ecall",
        in("a7") 4usize,
        in("a0") fd,
        options(nostack),
    );
}

#[cfg(target_arch = "riscv64")]
unsafe fn sys_exec(ptr: usize, len: usize, args: usize) {
    core::arch::asm!(
        "ecall",
        in("a7") 5usize,
        in("a0") ptr,
        in("a1") len,
        in("a2") args,
        options(nostack),
    );
}

#[cfg(target_arch = "riscv64")]
unsafe fn sys_fork() -> usize {
    sys_fork_raw()
}

#[cfg(target_arch = "riscv64")]
unsafe fn sys_wait(status_ptr: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "ecall",
        in("a7") 7usize,
        inout("a0") status_ptr => ret,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "riscv64")]
unsafe fn sys_pipe(fds_ptr: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "ecall",
        in("a7") 10usize,
        inout("a0") fds_ptr => ret,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "riscv64")]
unsafe fn sys_dup2(oldfd: usize, newfd: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "ecall",
        in("a7") 11usize,
        in("a0") oldfd,
        in("a1") newfd,
        lateout("a0") ret,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "riscv64")]
unsafe fn sys_listdir(path: usize, path_len: usize, buf: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "ecall",
        in("a7") 8usize,
        inout("a0") path => ret,
        in("a1") path_len,
        in("a2") buf,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "riscv64")]
unsafe fn sys_brk(addr: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "ecall",
        in("a7") 9usize,
        inout("a0") addr => ret,
        options(nostack),
    );
    ret
}
