#![no_std]

pub mod alloc;
pub mod args;
pub mod dns;
pub mod runtime;
pub mod thread;

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
/// `[ OK ] label` — same spacing as kernel `console::status_ok`.
pub fn status_ok(label: &str) {
    write(b"[ OK ] ");
    write(label.as_bytes());
    write(b"\n");
}

/// `[ FAIL ] label` — same spacing as kernel `console::status_fail`.
pub fn status_fail(label: &str) {
    write(b"[ FAIL ] ");
    write(label.as_bytes());
    write(b"\n");
}

/// `[ INFO ] label` — same spacing as kernel `console::status_info`.
pub fn status_info(label: &str) {
    write(b"[ INFO ] ");
    write(label.as_bytes());
    write(b"\n");
}

/// `[ WARN ] label` — same spacing as kernel `console::status_warn`.
pub fn status_warn(label: &str) {
    write(b"[ WARN ] ");
    write(label.as_bytes());
    write(b"\n");
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

/// The path calls (`kernel/src/user/at.rs`): a directory fd (`AT_FDCWD`:
/// the cwd) and a path relative to it.
const SYS_OPENAT: usize = 70;
const SYS_STATAT: usize = 71;
const SYS_MKNODAT: usize = 72;
const SYS_SYMLINKAT: usize = 73;
const SYS_UNLINKAT: usize = 74;
const SYS_RENAMEAT: usize = 75;
const SYS_READLINKAT: usize = 76;
const SYS_LISTDIRAT: usize = 79;
const SYS_EXECAT: usize = 80;
const AT_FDCWD: usize = -100isize as usize;
const AT_SYMLINK_NOFOLLOW: usize = 0x100;
const AT_REMOVEDIR: usize = 0x200;
const MKNOD_DIR: usize = 0;

pub fn open_flags(path: &[u8], flags: u32) -> Option<usize> {
    let fd = unsafe { sys6(SYS_OPENAT, AT_FDCWD, path.as_ptr() as usize, path.len(), flags as usize, 0, 0) };
    if fd == usize::MAX { None } else { Some(fd) }
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
            sys6(SYS_EXECAT, AT_FDCWD, path_ptr, path.len(), 0, 0, 0);
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
        sys6(SYS_EXECAT, AT_FDCWD, path_ptr, path.len(), pack.as_ptr() as usize, 0, 0);
    }
}

pub fn fork() -> Option<usize> {
    let pid = unsafe { sys_fork() };
    if pid == usize::MAX { None } else { Some(pid) }
}

pub fn wait() -> Option<usize> {
    let pid = unsafe { sys_wait(0) };
    if pid == usize::MAX { None } else { Some(pid) }
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

/// The size of the buffer callers list a directory into.
pub const LISTDIR_BUF: usize = 4096;

/// List directory entries at `path` (newline-separated) into `buf`: the
/// bytes written (all of `buf`: there may be more), `usize::MAX` on error.
pub fn listdir(path: &[u8], buf: &mut [u8]) -> usize {
    unsafe {
        sys6(SYS_LISTDIRAT, AT_FDCWD, path.as_ptr() as usize, path.len(), buf.as_mut_ptr() as usize, buf.len(), 0)
    }
}

/// A path call on `path` (relative to the cwd) with one more argument.
fn path_call(nr: usize, path: &[u8], arg: usize) -> bool {
    unsafe { sys6(nr, AT_FDCWD, path.as_ptr() as usize, path.len(), arg, 0, 0) != usize::MAX }
}

pub fn mkdir(path: &[u8]) -> bool {
    path_call(SYS_MKNODAT, path, MKNOD_DIR)
}

pub fn rmdir(path: &[u8]) -> bool {
    path_call(SYS_UNLINKAT, path, AT_REMOVEDIR)
}

pub fn unlink(path: &[u8]) -> bool {
    path_call(SYS_UNLINKAT, path, 0)
}

pub fn rename(old: &[u8], new: &[u8]) -> bool {
    let (o, n) = (old.as_ptr() as usize, new.as_ptr() as usize);
    unsafe { sys6(SYS_RENAMEAT, AT_FDCWD, o, old.len(), AT_FDCWD, n, new.len()) != usize::MAX }
}

pub fn symlink(target: &[u8], linkpath: &[u8]) -> bool {
    let (t, l) = (target.as_ptr() as usize, linkpath.as_ptr() as usize);
    unsafe { sys6(SYS_SYMLINKAT, t, target.len(), AT_FDCWD, l, linkpath.len(), 0) != usize::MAX }
}

pub fn readlink(path: &[u8], buf: &mut [u8]) -> Option<usize> {
    let (p, b) = (path.as_ptr() as usize, buf.as_mut_ptr() as usize);
    let n = unsafe { sys6(SYS_READLINKAT, AT_FDCWD, p, path.len(), b, buf.len(), 0) };
    if n == usize::MAX { None } else { Some(n) }
}

/// Load the kernel module ELF at `path` (`SYS_INSMOD` = 58), e.g.
/// `/lib/modules/hello`. The kernel prints the reason on failure.
pub fn insmod(path: &[u8]) -> bool {
    const CAP: usize = 128;
    let mut buf = [0u8; CAP];
    let n = copy_exec_bytes(&mut buf, path);
    if n == 0 {
        return false;
    }
    unsafe { sys3(58, buf.as_ptr() as usize, n, 0) != usize::MAX }
}

/// Unload the kernel module `name` (`SYS_RMMOD` = 59); it must not provide
/// anything any more (a device, a filesystem, a mount). The kernel prints
/// the reason on failure.
pub fn rmmod(name: &[u8]) -> bool {
    const CAP: usize = 64;
    let mut buf = [0u8; CAP];
    let n = copy_exec_bytes(&mut buf, name);
    if n == 0 {
        return false;
    }
    unsafe { sys3(59, buf.as_ptr() as usize, n, 0) != usize::MAX }
}

/// Mount `src` (a block device, `/dev/vda`) at `tgt`, an existing directory
/// that is not a mount point yet, using `fstype` (`fat`, `ext2`); `bind`
/// makes `src` visible at `tgt` too.
pub fn mount(src: &[u8], tgt: &[u8], fstype: &[u8]) -> bool {
    const CAP: usize = 128;
    let mut src_buf = [0u8; CAP];
    let mut tgt_buf = [0u8; CAP];
    let mut fs_buf = [0u8; 32];
    let sn = copy_exec_bytes(&mut src_buf, src);
    let tn = copy_exec_bytes(&mut tgt_buf, tgt);
    let fn_ = copy_exec_bytes(&mut fs_buf, fstype);
    if sn == 0 || tn == 0 || fn_ == 0 {
        return false;
    }
    let mut pack = [0usize; 6];
    pack[0] = src_buf.as_ptr() as usize;
    pack[1] = sn;
    pack[2] = tgt_buf.as_ptr() as usize;
    pack[3] = tn;
    pack[4] = fs_buf.as_ptr() as usize;
    pack[5] = fn_;
    unsafe { sys3(27, pack.as_ptr() as usize, 0, 0) != usize::MAX }
}


/// Detach the block-device mount at `path` (`SYS_UMOUNT` = 64): false when
/// nothing that can be unmounted is mounted there, or it is busy.
pub fn umount(path: &[u8]) -> bool {
    let mut buf = [0u8; 128];
    let n = copy_exec_bytes(&mut buf, path);
    if n == 0 {
        return false;
    }
    unsafe { sys3(64, buf.as_ptr() as usize, n, 0) != usize::MAX }
}

/// The mode bits of `path` (a symlink not followed), or `None` when it
/// does not exist.
pub fn stat_mode(path: &[u8]) -> Option<u32> {
    let mut buf = [0u8; 128];
    let n = copy_exec_bytes(&mut buf, path);
    // The kernel's `MyosStat`: st_mode first, 48 bytes in all.
    let mut out = [0u32; 12];
    let (p, o) = (buf.as_ptr() as usize, out.as_mut_ptr() as usize);
    let ret = unsafe { sys6(SYS_STATAT, AT_FDCWD, p, n, AT_SYMLINK_NOFOLLOW, o, 0) };
    (ret != usize::MAX).then_some(out[0])
}

/// Wall-clock time (`SYS_GETTIMEOFDAY` = 33). Returns `(sec, usec)` or `None`.
pub fn gettimeofday() -> Option<(i64, i64)> {
    let mut tv = [0i64; 2];
    let ret = unsafe { sys3(33, tv.as_mut_ptr() as usize, 0, 0) };
    if ret == usize::MAX {
        None
    } else {
        Some((tv[0], tv[1]))
    }
}

/// Wall-clock budget for a polling loop, replacing iteration counts (which
/// scale with syscall speed: a 400k-read loop used to last seconds, now well
/// under one). `wait()` sleeps 1 ms, ended early by any kernel event (a netd
/// reply landing in the channel), so the loop neither spins nor adds latency.
pub struct Timeout {
    end_us: i64,
    /// Fallback when there is no clock: remaining polls.
    polls_left: u64,
}

impl Timeout {
    pub fn ms(ms: i64) -> Self {
        let end_us = now_us().map(|n| n.saturating_add(ms.saturating_mul(1000))).unwrap_or(i64::MAX);
        Self {
            end_us,
            polls_left: (ms.max(1) as u64).saturating_mul(1000),
        }
    }

    pub fn expired(&self) -> bool {
        match now_us() {
            Some(n) => n >= self.end_us,
            None => self.polls_left == 0,
        }
    }

    /// Sleep briefly before the next poll (any kernel event ends it).
    pub fn wait(&mut self) {
        self.polls_left = self.polls_left.saturating_sub(1);
        sleep_ns(1_000_000, true);
    }
}

/// Wall clock in microseconds since the epoch (`None` without a clock).
pub fn now_us() -> Option<i64> {
    gettimeofday().map(|(s, us)| s.saturating_mul(1_000_000).saturating_add(us))
}

/// Sleep for `ns` nanoseconds (`SYS_NANOSLEEP` = 52). With `any_event` the
/// kernel also returns early when something a poller may care about happened
/// (console / pipe / device traffic, a child exit); callers re-poll then.
pub fn sleep_ns(ns: u64, any_event: bool) {
    let flags = if any_event { 1 } else { 0 };
    unsafe {
        sys3(52, ns as usize, flags, 0);
    }
}

pub const SEEK_SET: usize = 0;
pub const SEEK_CUR: usize = 1;
pub const SEEK_END: usize = 2;

/// Reposition the file offset. Returns the resulting offset, or `usize::MAX` on error.
pub fn lseek(fd: usize, offset: usize, whence: usize) -> usize {
    unsafe { sys3(26, fd, offset, whence) }
}

/// `struct pollfd` of [`poll`] (`SYS_POLL` = 38): Linux layout and bits.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PollFd {
    pub fd: i32,
    pub events: i16,
    pub revents: i16,
}

pub const POLLIN: i16 = 0x1;
pub const POLLOUT: i16 = 0x4;
pub const POLLHUP: i16 = 0x10;

/// Wait until one of `fds` is ready for what its `events` ask, or
/// `timeout_ms` passed (negative: no limit, 0: just look). Returns how many
/// are ready, their `revents` set, or `usize::MAX` on error.
pub fn poll(fds: &mut [PollFd], timeout_ms: i32) -> usize {
    unsafe { sys3(38, fds.as_mut_ptr() as usize, fds.len(), timeout_ms as isize as usize) }
}

/// Must match newlib `<signal.h>` / kernel `signal.rs` / rustix_compat.
pub const SIGINT: u32 = 2;
pub const SIGKILL: u32 = 9;
pub const SIGTERM: u32 = 15;

/// `getpid` (SYS_GETPID = 36): current task id.
pub fn getpid() -> usize {
    unsafe { sys3(36, 0, 0, 0) }
}

/// `kill(pid, sig)` (SYS_KILL = 34). Returns `true` on success.
pub fn kill(pid: usize, sig: u32) -> bool {
    unsafe { sys3(34, pid, sig as usize, 0) != usize::MAX }
}

/// `raise(sig)` via kill(getpid(), sig).
pub fn raise(sig: u32) -> bool {
    kill(getpid(), sig)
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
unsafe fn sys6(nr: usize, a0: usize, a1: usize, a2: usize, a3: usize, a4: usize, a5: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "syscall",
        inout("rax") nr => ret,
        in("rdi") a0,
        in("rsi") a1,
        in("rdx") a2,
        in("r10") a3,
        in("r8") a4,
        in("r9") a5,
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
unsafe fn sys6(nr: usize, a0: usize, a1: usize, a2: usize, a3: usize, a4: usize, a5: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "svc #0",
        in("x8") nr,
        inout("x0") a0 => ret,
        in("x1") a1,
        in("x2") a2,
        in("x3") a3,
        in("x4") a4,
        in("x5") a5,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "riscv64")]
unsafe fn sys6(nr: usize, a0: usize, a1: usize, a2: usize, a3: usize, a4: usize, a5: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "ecall",
        in("a7") nr,
        inout("a0") a0 => ret,
        in("a1") a1,
        in("a2") a2,
        in("a3") a3,
        in("a4") a4,
        in("a5") a5,
        options(nostack),
    );
    ret
}

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
    // a1/rsi = options; must be 0 (blocking). Leaving rsi unset made WNOHANG
    // spuriously active when bit0 was set in leftover register state.
    core::arch::asm!(
        "syscall",
        in("rax") 7usize,
        in("rdi") status_ptr,
        in("rsi") 0usize,
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
    // a1/x1 = options; zero = blocking wait (see x86_64 sys_wait comment).
    core::arch::asm!(
        "svc #0",
        in("x8") 7usize,
        in("x0") status_ptr,
        in("x1") 0usize,
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
unsafe fn sys_fork() -> usize {
    sys_fork_raw()
}

#[cfg(target_arch = "riscv64")]
unsafe fn sys_wait(status_ptr: usize) -> usize {
    let ret: usize;
    // a1 = options; zero = blocking wait (see x86_64 sys_wait comment).
    core::arch::asm!(
        "ecall",
        in("a7") 7usize,
        inout("a0") status_ptr => ret,
        in("a1") 0usize,
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
