//! Syscall numbers, the dispatcher entered from the per-arch trap paths,
//! and the `sys_*` implementations.

use super::*;

const SYS_WRITE: usize = 0;
const SYS_EXIT: usize = 1;
const SYS_OPEN: usize = 2;
const SYS_READ: usize = 3;
const SYS_CLOSE: usize = 4;
const SYS_EXEC: usize = 5;
pub(super) const SYS_FORK: usize = 6;
const SYS_WAIT: usize = 7;
const SYS_LISTDIR: usize = 8;
const SYS_BRK: usize = 9;
const SYS_PIPE: usize = 10;
const SYS_DUP2: usize = 11;
pub const SYS_STAT: usize = 12;
const SYS_EXECNAME: usize = 13;
const SYS_DUPFD: usize = 14;
const SYS_CHDIR: usize = 15;
const SYS_GETCWD: usize = 16;
const SYS_MKDIR: usize = 17;
const SYS_RMDIR: usize = 18;
const SYS_UNLINK: usize = 19;
const SYS_RENAME: usize = 20;
const SYS_SYMLINK: usize = 21;
const SYS_READLINK: usize = 22;
const SYS_MMAP: usize = 23;
const SYS_MUNMAP: usize = 24;
const SYS_MPROTECT: usize = 25;
const SYS_LSEEK: usize = 26;
const SYS_MOUNT: usize = 27;
// 28 was `ioctl`, gone: a device's state is its `ctl` file (docs/tty.md).
const SYS_SETSID: usize = 29;
const SYS_SETPGID: usize = 30;
const SYS_GETPGID: usize = 31;
const SYS_GETSID: usize = 32;
const SYS_GETTIMEOFDAY: usize = 33;
const SYS_KILL: usize = 34;
const SYS_SIGACTION: usize = 35;
const SYS_GETPID: usize = 36;
const SYS_SIGPROCMASK: usize = 37;
/// `poll(fds, nfds, timeout_ms)`: set each `struct pollfd`'s `revents`
/// and return how many are non-zero, waiting for the first one when none
/// is (up to `timeout_ms`; negative: no limit, 0: just look).
const SYS_POLL: usize = 38;
/// Legacy (39, 41, 42): before the kernel delivered handlers, libgloss
/// polled for SIGCHLD from `select()`/`poll()` and ran the handler itself.
/// Kept for binaries built against that libgloss.
/// Take-and-clear the current task's pending `SIGCHLD` bit (returns 1/0).
const SYS_SIGCHLD_TAKE: usize = 39;
/// 1 if the calling task has an exited-and-unreaped child.
const SYS_SIGCHLD_PENDING: usize = 41;
/// Peer fd of a pipe end (the old SIGCHLD self-pipe wake).
const SYS_PIPE_PEER: usize = 42;
/// chroot(path): confine the caller (and its future children) to `path`.
const SYS_CHROOT: usize = 43;
/// mkfifo(path, mode): create a named pipe (tmpfs only).
const SYS_MKFIFO: usize = 44;
/// sigreturn(): the libc signal trampoline is done with the frame at the
/// user stack pointer (see `crate::signal`).
const SYS_SIGRETURN: usize = 45;

/// waitpid(status, options, pid): like `SYS_WAIT` but writes a POSIX `int`
/// status (`WIFSIGNALED`-aware) and can wait for one child. A new number, as
/// some `SYS_WAIT` callers leave the second and third registers unset.
const SYS_WAITPID: usize = 46;
/// sigpending() -> pending set.
const SYS_SIGPENDING: usize = 47;
/// sigsuspend(mask): wait for a signal with `mask` blocked (always EINTR).
const SYS_SIGSUSPEND: usize = 48;
/// sigwait(set) -> signal number taken from `set`.
const SYS_SIGWAIT: usize = 49;
/// sigaction with a 4th struct word, the libc trampoline that runs function
/// handlers (`SYS_SIGACTION` keeps the 3-word, DFL/IGN-only form).
const SYS_SIGACTION2: usize = 50;

/// linux_next_exec(): the caller's next successful exec starts the image
/// with the Linux personality (the `linux` launcher). Only with the optional
/// `linux-compat` feature; otherwise an unknown syscall.
const SYS_LINUX_NEXT_EXEC: usize = 51;
/// `nanosleep(ns, flags)`: block the task (its CPU halts) until the deadline
/// or a signal. `flags & 1` (`SLEEP_ANY_EVENT`) also ends the sleep on any
/// kernel event a poller may care about (console/pipe/pty/device traffic, an
/// exit), so `poll()`/`select()` loops in libgloss sleep between scans
/// instead of spinning on `gettimeofday`. Returns 0 (deadline or event),
/// `EINTR` when a signal acts.
const SYS_NANOSLEEP: usize = 52;
const SLEEP_ANY_EVENT: usize = 1;
/// `thread_spawn(&ThreadSpawn)`: start a thread in the calling process at
/// `entry(arg)` on the stack whose top is `stack`, with thread pointer `tls`
/// (`ThreadSpawn` is four `u64`s in that order: entry, stack, arg, tls). The
/// entry must not return: it ends with `thread_exit`. Returns the tid.
/// Threads share everything but their registers and stacks (see
/// `task::thread`).
const SYS_THREAD_SPAWN: usize = 53;
/// `thread_exit(code)`: end the calling thread; the process ends with its
/// last thread (`exit` ends all of them).
const SYS_THREAD_EXIT: usize = 54;
/// `wait_addr(addr, expected, timeout_ns)`: block while the 32-bit word at
/// `addr` holds `expected`, until `wake_addr` on it, the timeout (0 = none)
/// or a signal (`EINTR`). Returns 0 (woken, possibly spuriously),
/// `WAIT_ADDR_CHANGED` (the word did not hold `expected`) or
/// `WAIT_ADDR_TIMEOUT`. For user-space locks.
const SYS_WAIT_ADDR: usize = 55;
const WAIT_ADDR_CHANGED: usize = 1;
const WAIT_ADDR_TIMEOUT: usize = 2;
/// `wake_addr(addr, count)`: wake up to `count` threads of the calling
/// process waiting on `addr`. Returns how many it woke.
const SYS_WAKE_ADDR: usize = 56;
/// `gettid()`: the calling thread's id (`getpid` is its process's).
const SYS_GETTID: usize = 57;
/// `insmod(path, len)`: load the kernel module ELF at `path` (named after
/// its last path component). 0 ok, `SYSERR` on any failure.
const SYS_INSMOD: usize = 58;
/// `rmmod(name, len)`: unload the kernel module `name` when nothing it
/// registered is in place. 0 ok, `SYSERR` on any failure.
const SYS_RMMOD: usize = 59;

/// Wait options bit 0: `WNOHANG` (userspace `WNOHANG = 1`).
const WAIT_NOHANG: usize = 1;

/// The user registers the syscall return path loads back, for the signal
/// code to read and redirect (delivery, sigreturn, restart).
///
/// - x86_64: the block `syscall_entry` pushes: user rsp, rip (rcx), r8, r9,
///   rflags (r11). The result travels in rax, which also carried the number.
/// - aarch64 / riscv64: the full trap frame (`x0..x31`, pc at 32, sp at 34);
///   the number register is x8 / a7.
pub struct SyscallRegs(*mut u64);

impl SyscallRegs {
    const PC: usize = crate::arch::SYSCALL_PC;
    const SP: usize = crate::arch::SYSCALL_SP;
    const NR_REG: Option<usize> = crate::arch::SYSCALL_NR_REG;

    /// Length of the syscall instruction (`syscall` / `svc` / `ecall`).
    pub const INSN_LEN: usize = crate::arch::SYSCALL_INSN_LEN;

    /// Result-register value that makes a rewound syscall run again: the
    /// number on x86 (rax carries both), the first argument elsewhere.
    pub fn restart_value(nr: usize, a0: usize) -> usize {
        if crate::arch::SYSCALL_RESTART_IS_NR { nr } else { a0 }
    }

    pub fn pc(&self) -> usize {
        unsafe { *self.0.add(Self::PC) as usize }
    }
    pub fn set_pc(&mut self, v: usize) {
        unsafe { *self.0.add(Self::PC) = v as u64 }
    }
    pub fn sp(&self) -> usize {
        unsafe { *self.0.add(Self::SP) as usize }
    }
    pub fn set_sp(&mut self, v: usize) {
        unsafe { *self.0.add(Self::SP) = v as u64 }
    }
    pub fn nr_reg(&self) -> usize {
        Self::NR_REG.map_or(0, |i| unsafe { *self.0.add(i) as usize })
    }
    pub fn set_nr_reg(&mut self, v: usize) {
        if let Some(i) = Self::NR_REG {
            unsafe { *self.0.add(i) = v as u64 }
        }
    }
    /// Word `i` of the saved user registers, for the Linux layer's syscall
    /// arguments and signal frames (layout per arch: see above).
    /// The raw block, for a personality module (`crate::personality`).
    pub fn as_ptr(&self) -> *mut u64 {
        self.0
    }
    /// A module's view of the live block (`KernelApi::native_syscall`).
    pub fn from_ptr(p: *mut u64) -> Self {
        Self(p)
    }
}

/// Record the live trap frame for fork/exec resume (aarch64/riscv; a no-op
/// on x86, so `signal::deliver_due` can clear it on every arch).
pub use crate::arch::set_syscall_frame;

#[unsafe(no_mangle)]
pub extern "C" fn syscall_dispatch(
    nr: usize,
    a0: usize,
    a1: usize,
    a2: usize,
    user_rip: usize,
    user_rsp: usize,
    regs: *mut u64,
) -> usize {
    task::save_user_context(user_rip, user_rsp);
    let mut regs = SyscallRegs(regs);
    // A task exec'd with a foreign personality (the Linux module) makes that
    // personality's syscalls (own numbers, errno returns); see `personality`.
    if let Some(ret) = crate::personality::dispatch(nr, a0, a1, a2, &mut regs) {
        return crate::signal::on_syscall_exit(&mut regs, nr, a0, ret);
    }
    let ret = native_dispatch(nr, a0, a1, a2, &mut regs);
    // Terminate, run a handler, or turn an interrupted wait into EINTR.
    crate::signal::on_syscall_exit(&mut regs, nr, a0, ret)
}

/// The native syscall table (also reachable by a personality module through
/// `KernelApi::native_syscall`).
pub(crate) fn native_dispatch(nr: usize, a0: usize, a1: usize, a2: usize, regs: &mut SyscallRegs) -> usize {
    match nr {
        SYS_WRITE => sys_write(a0, a1, a2),
        SYS_EXIT => sys_exit(a0),
        SYS_OPEN => sys_open(a0, a1, a2),
        SYS_READ => sys_read(a0, a1, a2),
        SYS_CLOSE => sys_close(a0),
        SYS_EXEC => sys_exec(a0, a1, a2),
        SYS_FORK => sys_fork(regs),
        SYS_WAIT => sys_wait(a0, a1),
        SYS_WAITPID => sys_waitpid(a0, a1, a2),
        SYS_LISTDIR => sys_listdir(a0, a1, a2),
        SYS_BRK => sys_brk(a0),
        SYS_PIPE => sys_pipe(a0),
        SYS_DUP2 => sys_dup2(a0, a1),
        SYS_DUPFD => sys_dupfd(a0, a1),
        SYS_STAT => sys_stat(a0, a1, a2),
        SYS_EXECNAME => sys_exec_name(a0, a1),
        SYS_CHDIR => sys_chdir(a0, a1),
        SYS_GETCWD => sys_getcwd(a0, a1),
        SYS_MKDIR => sys_mkdir(a0, a1, a2),
        SYS_RMDIR => sys_rmdir(a0, a1),
        SYS_UNLINK => sys_unlink(a0, a1),
        SYS_RENAME => sys_rename(a0, a1, a2),
        SYS_SYMLINK => sys_symlink(a0, a1, a2),
        SYS_READLINK => sys_readlink(a0, a1, a2),
        SYS_MMAP => sys_mmap(a0),
        SYS_MUNMAP => sys_munmap(a0, a1),
        SYS_MPROTECT => sys_mprotect(a0, a1, a2),
        SYS_LSEEK => sys_lseek(a0, a1, a2),
        SYS_MOUNT => sys_mount(a0),
        SYS_SETSID => sys_setsid(),
        SYS_SETPGID => sys_setpgid(a0, a1),
        SYS_GETPGID => sys_getpgid(a0),
        SYS_GETSID => sys_getsid(a0),
        SYS_GETTIMEOFDAY => sys_gettimeofday(a0, a1),
        SYS_KILL => sys_kill(a0, a1),
        SYS_SIGACTION => sys_sigaction(a0, a1, a2, false),
        SYS_SIGACTION2 => sys_sigaction(a0, a1, a2, true),
        SYS_GETPID => sys_getpid(),
        SYS_SIGPROCMASK => sys_sigprocmask(a0, a1, a2),
        SYS_POLL => sys_poll(a0, a1, a2 as isize),
        SYS_SIGCHLD_TAKE => sys_sigchld_take(),
        SYS_SIGCHLD_PENDING => sys_sigchld_pending(),
        SYS_PIPE_PEER => sys_pipe_peer(a0),
        SYS_CHROOT => sys_chroot(a0, a1),
        SYS_MKFIFO => sys_mkfifo(a0, a1, a2),
        SYS_SIGRETURN => crate::signal::sigreturn(regs),
        SYS_SIGPENDING => crate::signal::sigpending(),
        SYS_SIGSUSPEND => crate::signal::sigsuspend(a0 as u32),
        SYS_SIGWAIT => crate::signal::sigwait(a0 as u32),
        SYS_NANOSLEEP => sys_nanosleep(a0, a1),
        SYS_THREAD_SPAWN => sys_thread_spawn(regs, a0),
        SYS_THREAD_EXIT => task::thread_exit(a0 as u8),
        SYS_WAIT_ADDR => sys_wait_addr(a0, a1, a2),
        SYS_WAKE_ADDR => task::wake_addr(a0, a1),
        SYS_GETTID => task::current_tid(),
        SYS_INSMOD => sys_insmod(a0, a1),
        SYS_RMMOD => sys_rmmod(a0, a1),
        SYS_LINUX_NEXT_EXEC => {
            if crate::personality::request_next_exec() { 0 } else { SYSERR }
        }
        _ => SYSERR,
    }
}

fn sys_exit(code: usize) -> ! {
    task::user_exit(code as u8);
}

fn sys_thread_spawn(regs: &SyscallRegs, params: usize) -> usize {
    let mut b = [0u8; 32];
    if !buffer_ok(params, b.len()) || !copy_from_user(task::current_aspace(), params, &mut b) {
        return SYSERR;
    }
    let word = |i: usize| u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap()) as usize;
    let (entry, stack, arg, tls) = (word(0), word(1), word(2), word(3) as u64);
    if entry == 0 || stack == 0 {
        return SYSERR;
    }
    task::spawn_thread(thread_start(regs, entry, stack, arg), Some(tls)).unwrap_or(SYSERR)
}

fn sys_wait_addr(addr: usize, expected: usize, timeout_ns: usize) -> usize {
    let deadline = if timeout_ns == 0 {
        0
    } else {
        crate::time::monotonic_ns().saturating_add(timeout_ns as u64).max(1)
    };
    match task::wait_addr(addr, expected as u32, deadline) {
        // An interrupted wait becomes `EINTR` in `on_syscall_exit`.
        task::AddrWait::Woken | task::AddrWait::Interrupted => 0,
        task::AddrWait::Changed => WAIT_ADDR_CHANGED,
        task::AddrWait::TimedOut => WAIT_ADDR_TIMEOUT,
        task::AddrWait::Fault => SYSERR,
    }
}

fn sys_nanosleep(ns: usize, flags: usize) -> usize {
    // Cap at ~100 days so the deadline arithmetic cannot wrap.
    let ns = (ns as u64).min(100 * 86_400 * 1_000_000_000);
    let deadline = crate::time::monotonic_ns().saturating_add(ns).max(1);
    let any = flags & SLEEP_ANY_EVENT != 0;
    loop {
        if task::sleep_until(deadline, any) {
            return 0;
        }
        // Either an event (any-event mode) or a signal ended the wait.
        if crate::signal::interrupt_wait() {
            return 0; // turned into EINTR / a restart by on_syscall_exit
        }
        if any {
            return 0;
        }
    }
}

fn sys_write(fd: usize, ptr: usize, len: usize) -> usize {
    task::fd_write(fd, ptr, len)
}

/// The real path behind `path`, symlinks followed.
pub(crate) fn resolve_copied_path(path: &str) -> Option<alloc::string::String> {
    let mut abs = [0u8; MAX_PATH];
    let n = fs::resolve_user_path(path, &mut abs)?;
    core::str::from_utf8(&abs[..n])
        .ok()
        .map(|s| alloc::string::String::from(s))
}

/// The real path behind `path`, a symlink in the last component not followed.
pub(crate) fn resolve_copied_path_nofollow(path: &str) -> Option<alloc::string::String> {
    let mut abs = [0u8; MAX_PATH];
    let n = fs::resolve_user_path_nofollow(path, &mut abs)?;
    core::str::from_utf8(&abs[..n])
        .ok()
        .map(|s| alloc::string::String::from(s))
}

fn copy_user_path(ptr: usize, len: usize) -> Option<[u8; MAX_PATH]> {
    if len == 0 || len > MAX_PATH {
        return None;
    }
    if !user_range_ok(ptr, len) {
        return None;
    }
    let mut buf = [0u8; MAX_PATH];
    let aspace = task::current_aspace();
    if !read_user_bytes(aspace, ptr, &mut buf[..len]) {
        return None;
    }
    Some(buf)
}

fn sys_open(ptr: usize, path_len: usize, flags: usize) -> usize {
    let Some(buf) = copy_user_path(ptr, path_len) else {
        return SYSERR;
    };
    let Ok(path) = core::str::from_utf8(&buf[..path_len]) else {
        return SYSERR;
    };
    open_path(path, flags)
}

fn sys_insmod(ptr: usize, path_len: usize) -> usize {
    let Some(buf) = copy_user_path(ptr, path_len) else {
        return SYSERR;
    };
    let Ok(path) = core::str::from_utf8(&buf[..path_len]) else {
        return SYSERR;
    };
    let Some(path) = resolve_copied_path(path) else {
        return SYSERR;
    };
    match crate::modules::insmod(&path) {
        Ok(()) => 0,
        Err(e) => {
            crate::console::status_fail(&alloc::format!("insmod {path}: {e}"));
            SYSERR
        }
    }
}

fn sys_rmmod(ptr: usize, name_len: usize) -> usize {
    let Some(buf) = copy_user_path(ptr, name_len) else {
        return SYSERR;
    };
    let Ok(name) = core::str::from_utf8(&buf[..name_len]) else {
        return SYSERR;
    };
    match crate::modules::rmmod(name) {
        Ok(()) => 0,
        Err(e) => {
            crate::console::status_fail(&alloc::format!("rmmod {name}: {e}"));
            SYSERR
        }
    }
}

/// open(2) of a cwd-relative or absolute path already in kernel memory.
pub(crate) fn open_path(path: &str, flags: usize) -> usize {
    let Some(path) = resolve_copied_path(path) else {
        return SYSERR;
    };
    // The pty ends (docs/tty.md): /dev/pts/clone allocates a pair and
    // returns its master, /dev/pts/N/data is the slave. The fd is a pty fd,
    // not a plain file fd (I/O routes via crate::pty). /dev/pts/N/master is
    // only ever what clone returned, never opened by name.
    let path_rel = path.trim_start_matches('/');
    if let Some(rest) = path_rel.strip_prefix("dev/pts/") {
        if rest == "clone" {
            return task::fd_open_pty_master().unwrap_or(SYSERR);
        }
        if let Some((index, member)) = rest.split_once('/') {
            if let Ok(id) = index.parse::<usize>() {
                match member {
                    "data" => return task::fd_open_pty_slave(id).unwrap_or(SYSERR),
                    "master" => return SYSERR,
                    _ => {}
                }
            }
        }
    }
    // /dev/tty in a pty session (a forkpty child and what it started: an SSH
    // login, the tty smoke) is that pty's slave, not the console.
    if path_rel == "dev/tty" {
        if let Some(id) = crate::pty::for_session(task::current_pid()) {
            return task::fd_open_pty_slave(id).unwrap_or(SYSERR);
        }
    }
    // Named FIFO: the fd is a pipe end, not a file vnode.
    if let Some(id) = fs::vfs::fifo_id(&path) {
        return match task::fd_open_fifo(id, flags as u32) {
            Ok(fd) => fd,
            Err(task::FifoOpenErr::NoReader) => SYSERR_ENXIO,
            Err(task::FifoOpenErr::Failed) => SYSERR,
        };
    }
    let Some(node) = fs::open(&path, flags as u32) else {
        return SYSERR;
    };
    match task::fd_open(node, flags as u32) {
        Some(fd) => fd,
        None => SYSERR,
    }
}

pub(crate) fn sys_read(fd: usize, buf: usize, len: usize) -> usize {
    task::fd_read(fd, buf, len)
}

fn sys_close(fd: usize) -> usize {
    if task::fd_close(fd) { 0 } else { SYSERR }
}

/// Become a session leader: `sid = pid` (task slot), new process group
/// (`pgid = pid`), clear controlling tty.
/// Fails with SYSERR if the caller is already a session leader (`sid == pid`).

pub(crate) fn sys_gettimeofday(tv_ptr: usize, _tz: usize) -> usize {
    const N: usize = 16; // two i64s
    if tv_ptr == 0 || !user_range_ok(tv_ptr, N) {
        return SYSERR;
    }
    let Some((secs, usec)) = crate::time::timeval() else {
        return SYSERR;
    };
    let mut raw = [0u8; N];
    raw[..8].copy_from_slice(&secs.to_le_bytes());
    raw[8..16].copy_from_slice(&usec.to_le_bytes());
    if !write_user_bytes(task::current_aspace(), tv_ptr, &raw) {
        return SYSERR;
    }
    0
}

pub(crate) fn sys_setsid() -> usize {
    match task::setsid() {
        Some(sid) => sid,
        None => SYSERR,
    }
}

pub(crate) fn sys_setpgid(pid: usize, pgid: usize) -> usize {
    if task::setpgid(pid, pgid) {
        0
    } else {
        SYSERR
    }
}

pub(crate) fn sys_getpgid(pid: usize) -> usize {
    match task::getpgid(pid) {
        Some(pgid) => pgid,
        None => SYSERR,
    }
}

pub(crate) fn sys_getsid(pid: usize) -> usize {
    match task::getsid(pid) {
        Some(sid) => sid,
        None => SYSERR,
    }
}

fn sys_getpid() -> usize {
    task::current_pid()
}

/// `kill(pid, sig)` — `pid` is interpreted as signed (`isize`) for pgid rules.
fn sys_kill(pid: usize, sig: usize) -> usize {
    if crate::signal::kill(pid as isize, sig as u32) {
        0
    } else {
        SYSERR
    }
}

/// `sigaction(sig, act, oact)`; see [`crate::signal::sigaction`].
fn sys_sigaction(sig: usize, act: usize, oact: usize, with_tramp: bool) -> usize {
    let act = if act == 0 { None } else { Some(act) };
    let oact = if oact == 0 { None } else { Some(oact) };
    if crate::signal::sigaction(sig as u32, act, oact, with_tramp) {
        0
    } else {
        SYSERR
    }
}

fn sys_sigprocmask(how: usize, set: usize, oset: usize) -> usize {
    let set = if set == 0 { None } else { Some(set) };
    let oset = if oset == 0 { None } else { Some(oset) };
    if crate::signal::sigprocmask(how, set, oset) {
        0
    } else {
        SYSERR
    }
}

pub(super) fn sys_exec(ptr: usize, path_len: usize, args_ptr: usize) -> usize {
    let Some(buf) = copy_user_path(ptr, path_len) else {
        return SYSERR;
    };
    let Ok(path) = core::str::from_utf8(&buf[..path_len]) else {
        return SYSERR;
    };
    let (arg_bufs, env_bufs) = match copy_user_exec_pack(args_ptr) {
        Ok(v) => v,
        Err(()) => return SYSERR,
    };
    let arg_refs: Vec<&[u8]> = arg_bufs.iter().map(|s| s.as_slice()).collect();
    let env_refs: Vec<&[u8]> = env_bufs.iter().map(|s| s.as_slice()).collect();
    exec_path(path, &arg_refs, &env_refs)
}

/// Replace the current image with the ELF at `path` (cwd-relative or
/// absolute); returns only on failure.
pub(crate) fn exec_path(path: &str, arg_refs: &[&[u8]], env_refs: &[&[u8]]) -> usize {
    let Some(path) = resolve_copied_path(path) else {
        return SYSERR;
    };
    let basename = path.rsplit('/').next().unwrap_or(path.as_str()).as_bytes();
    task::set_exec_name(basename);
    // Static lookup for bootfs/`/t/tcc`; VFS read for tmpfs `tcc -o` output.
    // The file is read into the kernel heap; what gets mapped is still capped
    // by the image limits (`MAX_EXPAND_PAGES`).
    const EXEC_FILE_MAX: usize = 16 * 1024 * 1024;
    let owned;
    let bytes: &[u8] = if let Some(b) = fs::lookup(&path) {
        b
    } else if let Some(v) = fs::read_all(&path, EXEC_FILE_MAX) {
        owned = v;
        &owned
    } else {
        // /lib-style read-only mounts expose files through the vnode path
        // (open + size + read) even where the read_all direct-backend shortcut
        // fails; use it before giving up. Without this, every exec of a
        // prebuilt ELF under /lib/os-test/prebuilt fails with EACCES and the
        // boot smoke silently falls back to guest tcc.
        let Some(node) = fs::open(&path, 0) else {
            return SYSERR;
        };
        let Some(size) = fs::size_of(&node) else {
            return SYSERR;
        };
        if size == 0 || size > EXEC_FILE_MAX {
            return SYSERR;
        }
        let mut v = alloc::vec![0u8; size];
        let mut pos = 0usize;
        while pos < size {
            let n = fs::read(&node, pos, &mut v[pos..]);
            if n == 0 {
                return SYSERR;
            }
            pos += n;
        }
        owned = v;
        &owned
    };
    // A foreign-personality image that is dynamically linked also needs its
    // interpreter (the dynamic linker). Read it before the current image is
    // replaced, so a missing one fails the exec cleanly.
    let interp = match exec_interp(bytes) {
        Ok(i) => i,
        Err(()) => return SYSERR,
    };
    // A dynamically linked program is relocated by its dynamic linker.
    let relocate = interp.is_none();
    // The old image goes away from here on: the process's other threads
    // end first.
    if !task::exec_alone() {
        return SYSERR;
    }
    // Large in-place expand (ripgrep) can clobber tp; re-sync before any
    // current_slot()-backed lookup so we expand/replace the running task.
    crate::arch::sync_cpu_id_reg();
    let cur_aspace = task::current_aspace();
    let (base_u, _mapped_span, stack_off) = task::current_user_map();
    let old_brk = task::current_brk();
    let old_mmap = task::mmap_regions();
    let (aspace, entry, span, off) = {
        // Reuse the current aspace in place (reload / expand) so post-fork
        // `exec` does not leak the prior aspace + its frames on every exec.
        // riscv64 previously always took the `load_user_elf` fresh-aspace path
        // (introduced in 887caf5 for RISC-V CI parity), which leaked hundreds of
        // physical pages per exec (a 700+ page ripgrep ELF after `uutils false ok`
        // in `/heap`) and walked the bump allocator forward past the heavyweight
        // smoke ELFs. Keep the same in-place reload/expand/lazy-load sequence the
        // other arches use so the mapping/stack reservation logic is identical.
        {
            if cur_aspace != 0 {
                if let Some(v) = reload_user_elf(cur_aspace, bytes, base_u, stack_off, _mapped_span, relocate)
                    .map(|(entry, span, off)| (cur_aspace, entry, span, off))
                {
                    // POSIX exec drops anonymous maps. In-place reload used to
                    // clear the mmap table in `replace_user` without freeing
                    // frames — a quiet freelist leak (and, when expand later
                    // grows into the old mmap window, `reuse_or_alloc_frame`
                    // would adopt those pages as code then double-free them).
                    free_mmap_regions(cur_aspace, &old_mmap);
                    flush_user_tlb();
                    v
                } else {
                    // Free anonymous maps *before* expand remaps: when
                    // `stack_off` grows, the new code span can overlap the old
                    // mmap window and reuse_or_alloc would steal those frames.
                    free_mmap_regions(cur_aspace, &old_mmap);
                    task::clear_mmap();
                    flush_user_tlb();
                    if let Some(v) = expand_user_elf(cur_aspace, bytes, base_u, stack_off, relocate)
                        .map(|(entry, span, off)| (cur_aspace, entry, span, off))
                    {
                        v
                    } else if let Some(v) = load_user_elf(bytes, relocate) {
                        // Fresh aspace: reclaim code/stack/heap (mmap already freed).
                        reclaim_user_aspace(
                            cur_aspace,
                            base_u,
                            _mapped_span,
                            stack_off,
                            old_brk,
                            &[],
                        );
                        v
                    } else {
                        return SYSERR;
                    }
                }
            } else {
                let Some(v) = load_user_elf(bytes, relocate) else {
                    return SYSERR;
                };
                v
            }
        }
    };
    // The interpreter goes at the start of the new image's mmap window,
    // unrelocated; execution starts there (AT_ENTRY still names the program).
    let interp_map = match &interp {
        Some(ib) => {
            let at = mmap_base_va(base_u, off);
            match map_elf_unrelocated(aspace, ib, at) {
                Some((ientry, runs)) => Some((at as usize, ientry, runs)),
                None => return SYSERR,
            }
        }
        None => None,
    };
    // A foreign-personality image gets the SysV auxv its libc startup reads.
    let aux = exec_auxv(bytes, base_u, entry, interp_map.as_ref().map(|m| m.0));
    let entry = interp_map.as_ref().map_or(entry, |m| m.1);
    let Some((rsp, argv)) =
        build_argv_stack(aspace, base_u, off, arg_refs, env_refs, aux.entries())
    else {
        return SYSERR;
    };
    let argc = arg_refs.len();
    // A zero entry is never a valid userspace image (would sret to NULL → the
    // classic riscv64 `instruction page fault stval=0 sepc=0`). Refuse rather
    // than resume a corrupt realize/expand result.
    if entry == 0 {
        return SYSERR;
    }
    // expand_user_elf / reload of a large ELF (ripgrep) is deep enough that
    // LLVM may have clobbered tp since the sync above. replace_user and
    // set_loaded_aspace go through current_slot()/cpu_id() — re-pin before
    // mutating the running task and resuming.
    crate::arch::sync_cpu_id_reg();
    task::replace_user(aspace, entry, rsp, base_u, span, off, argc, argv);
    if let Some((_, _, runs)) = &interp_map {
        for &(va, pages, prot) in runs {
            if !task::mmap_add(va, pages, prot, None) {
                task::user_exit(127);
            }
        }
    }
    // aarch64: sret/eret through the live syscall frame (shallow exec);
    // riscv64 clears the frame instead and falls through (see there).
    crate::arch::exec_resume(entry, rsp, argc, argv);
    enter(entry, rsp, argc, argv);
}

/// Copy the native exec argument block `[argc, (ptr,len)…, envc, (ptr,len)…]`
/// in from the caller (see [`MAX_ARGC`], [`MAX_ENVC`], [`MAX_EXEC_STRINGS`]).
fn copy_user_exec_pack(args_ptr: usize) -> Result<(Vec<Vec<u8>>, Vec<Vec<u8>>), ()> {
    if args_ptr == 0 {
        return Ok((Vec::new(), Vec::new()));
    }
    let aspace = task::current_aspace();
    let word = core::mem::size_of::<usize>();
    let mut off = args_ptr;
    let mut budget = MAX_EXEC_STRINGS;
    let mut list = |max: usize, off: &mut usize| -> Result<Vec<Vec<u8>>, ()> {
        if !user_range_ok(*off, word) {
            return Err(());
        }
        let count = read_user_usize(aspace, *off).ok_or(())?;
        *off += word;
        if count > max || !user_range_ok(*off, count * 2 * word) {
            return Err(());
        }
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let p = read_user_usize(aspace, *off).ok_or(())?;
            let n = read_user_usize(aspace, *off + word).ok_or(())?;
            *off += 2 * word;
            // Each string also takes its NUL on the new stack.
            budget = budget.checked_sub(n.checked_add(1).ok_or(())?).ok_or(())?;
            if n != 0 && !user_range_ok(p, n) {
                return Err(());
            }
            let mut v = alloc::vec![0u8; n];
            if !read_user_bytes(aspace, p, &mut v) {
                return Err(());
            }
            out.push(v);
        }
        Ok(out)
    };
    let args = list(MAX_ARGC, &mut off)?;
    let env = list(MAX_ENVC, &mut off)?;
    Ok((args, env))
}

fn sys_listdir(path_ptr: usize, path_len: usize, buf: usize) -> usize {
    // Match libgloss MYOS_DIRBUF / myos_user::LISTDIR_BUF so /s listings
    // (~100 names) are not truncated. Callers must pass a mapped buffer of
    // this size (user_range_ok); a 512-byte stack buf fails the check.
    const LISTDIR_CAP: usize = 4096;
    if buf == 0 || !user_range_ok(buf, LISTDIR_CAP) {
        return SYSERR;
    }
    let path = if path_len == 0 {
        alloc::string::String::from(".")
    } else {
        let Some(pbuf) = copy_user_path(path_ptr, path_len) else {
            return SYSERR;
        };
        match core::str::from_utf8(&pbuf[..path_len]) {
            Ok(p) => alloc::string::String::from(p),
            Err(_) => return SYSERR,
        }
    };
    let Some(path) = resolve_copied_path(&path) else {
        return SYSERR;
    };
    let mut kbuf = [0u8; LISTDIR_CAP];
    let n = fs::listdir(&path, &mut kbuf).min(LISTDIR_CAP);
    let aspace = task::current_aspace();
    if !write_user_bytes(aspace, buf, &kbuf[..n]) {
        return SYSERR;
    }
    n
}

#[repr(C)]
struct MyosStatBuf {
    st_mode: u32,
    st_size: u32,
    st_ino: u32,
    st_nlink: u32,
    st_dev: u32,
}

fn sys_stat(path_ptr: usize, path_len: usize, out_ptr: usize) -> usize {
    if out_ptr == 0 || !user_range_ok(out_ptr, core::mem::size_of::<MyosStatBuf>()) {
        return SYSERR;
    }
    let Some(buf) = copy_user_path(path_ptr, path_len) else {
        return SYSERR;
    };
    let Ok(path) = core::str::from_utf8(&buf[..path_len]) else {
        return SYSERR;
    };
    // Native `stat` and `lstat` share this call: the last component is not
    // followed (a symlink reports itself), as before symlinks were followed.
    let Some(path) = resolve_copied_path_nofollow(path) else {
        return SYSERR;
    };
    let Some(info) = fs::stat(&path) else {
        return SYSERR;
    };
    let out = MyosStatBuf {
        st_mode: info.mode,
        st_size: info.size,
        st_ino: info.ino,
        st_nlink: info.nlink,
        st_dev: info.dev,
    };
    if !write_user_bytes(task::current_aspace(), out_ptr, unsafe {
        core::slice::from_raw_parts(
            &out as *const MyosStatBuf as *const u8,
            core::mem::size_of::<MyosStatBuf>(),
        )
    }) {
        return SYSERR;
    }
    0
}

/// The calling thread's user registers as a new task starts with them:
/// resuming after this syscall with result 0 (a forked child; a thread
/// changes the stack pointer and entry, see [`thread_start`]).
pub(crate) fn caller_regs(regs: &SyscallRegs) -> task::UserRegs {
    crate::arch::caller_regs(regs.0)
}

/// Registers for a native thread that starts at `entry(arg)` on the stack
/// whose top is `stack`, as if called (it must not return).
fn thread_start(regs: &SyscallRegs, entry: usize, stack: usize, arg: usize) -> task::UserRegs {
    let mut r = caller_regs(regs);
    r.rip = entry;
    crate::arch::regs_thread_start(&mut r, stack & !15, arg);
    r
}

pub(crate) fn sys_fork(regs: &SyscallRegs) -> usize {
    match task::fork_current(caller_regs(regs)) {
        Some(pid) => pid,
        None => SYSERR,
    }
}

/// `wait(status, options)`: any child; `status` gets the exit-code byte
/// (`128 + sig` for a signal death).
fn sys_wait(status_ptr: usize, options: usize) -> usize {
    if status_ptr != 0 && !user_range_ok(status_ptr, 1) {
        return SYSERR;
    }
    task::wait_child(
        if status_ptr == 0 { None } else { Some(status_ptr) },
        options & WAIT_NOHANG != 0,
        None,
        false,
    )
}

/// `waitpid(status, options, pid)`: `pid > 0` waits for that child, anything
/// else for any child; `status` gets a POSIX `int` status.
pub(crate) fn sys_waitpid(status_ptr: usize, options: usize, pid: usize) -> usize {
    if status_ptr != 0 && !user_range_ok(status_ptr, 4) {
        return SYSERR;
    }
    task::wait_child(
        if status_ptr == 0 { None } else { Some(status_ptr) },
        options & WAIT_NOHANG != 0,
        if (pid as isize) > 0 { Some(pid) } else { None },
        true,
    )
}

fn sys_pipe(fds_ptr: usize) -> usize {
    if !user_range_ok(fds_ptr, 2 * core::mem::size_of::<usize>()) {
        return SYSERR;
    }
    let Some((r, w)) = task::pipe_open() else {
        return SYSERR;
    };
    let mut buf = [0u8; 2 * core::mem::size_of::<usize>()];
    buf[..core::mem::size_of::<usize>()].copy_from_slice(&r.to_le_bytes());
    buf[core::mem::size_of::<usize>()..].copy_from_slice(&w.to_le_bytes());
    if !copy_to_user(task::current_aspace(), fds_ptr, &buf) {
        return SYSERR;
    }
    0
}

/// Most fds one `poll` looks at.
const POLL_MAX: usize = 256;

fn sys_poll(fds_ptr: usize, nfds: usize, timeout_ms: isize) -> usize {
    if nfds > POLL_MAX || (nfds != 0 && !user_range_ok(fds_ptr, nfds * 8)) {
        return SYSERR;
    }
    let aspace = task::current_aspace();
    // `struct pollfd { int fd; short events; short revents; }`
    let mut fds = alloc::vec![0u8; nfds * 8];
    if nfds != 0 && !copy_from_user(aspace, fds_ptr, &mut fds) {
        return SYSERR;
    }
    let deadline = if timeout_ms > 0 { task::deadline_ms(timeout_ms as u64) } else { 0 };
    loop {
        // Read before the scan: a wake during it makes the block below
        // return at once, so no event is lost.
        let seq = task::wait_seq();
        let mut ready = 0;
        let mut tty = false;
        for pfd in fds.chunks_exact_mut(8) {
            let fd = i32::from_ne_bytes([pfd[0], pfd[1], pfd[2], pfd[3]]);
            let events = u16::from_ne_bytes([pfd[4], pfd[5]]);
            let revents = if fd < 0 {
                0
            } else {
                let (bits, is_tty) = task::fd_poll(fd as usize);
                tty |= is_tty && events & task::POLLIN != 0;
                bits & (events | task::POLLERR | task::POLLHUP | task::POLLNVAL)
            };
            pfd[6..8].copy_from_slice(&revents.to_ne_bytes());
            if revents != 0 {
                ready += 1;
            }
        }
        let timed_out =
            timeout_ms == 0 || (deadline != 0 && crate::time::monotonic_ns() >= deadline);
        if ready > 0 || timed_out {
            if nfds != 0 && !copy_to_user(aspace, fds_ptr, &fds) {
                return SYSERR;
            }
            return ready;
        }
        // A signal that runs a handler (or terminates) ends the wait. Never
        // a restart, whatever SA_RESTART says (POSIX): servers wait in
        // select() for a handler's flag (dropbear's SIGTERM exit).
        if task::signal_wakeable(task::current_id()) {
            return crate::signal::SYSERR_EINTR;
        }
        // Sleep until anything happens (pipe, pty, console, device and
        // module traffic, an exit all wake pollers) or the deadline. The
        // keyboard is polled, not interrupt-driven: a watched console tty
        // re-checks at its rate, as `input::read` does.
        let mut until = deadline;
        if tty && crate::input::keyboard_present() {
            let keyboard = task::deadline_ms(10);
            until = if until == 0 { keyboard } else { until.min(keyboard) };
        }
        task::block_until(task::WAIT_ANY, seq, until);
    }
}

fn sys_sigchld_take() -> usize {
    sys_sigchld_take_inner()
}

fn sys_sigchld_pending() -> usize {
    if task::has_exited_child(task::current_pid()) { 1 } else { 0 }
}

fn sys_pipe_peer(fd: usize) -> usize {
    match task::fd_pipe_peer(fd) {
        Some(p) => p,
        None => SYSERR,
    }
}

fn sys_sigchld_take_inner() -> usize {
    let bit = 1u32 << crate::signal::SIGCHLD;
    if task::signal_take_pending(task::current_pid(), bit) {
        1
    } else {
        0
    }
}

fn sys_dup2(oldfd: usize, newfd: usize) -> usize {
    if task::fd_dup2(oldfd, newfd) {
        0
    } else {
        SYSERR
    }
}

fn sys_dupfd(oldfd: usize, minfd: usize) -> usize {
    match task::fd_dup_min(oldfd, minfd) {
        Some(fd) => fd,
        None => SYSERR,
    }
}

fn sys_chdir(path_ptr: usize, path_len: usize) -> usize {
    let Some(buf) = copy_user_path(path_ptr, path_len) else {
        return SYSERR;
    };
    let Ok(path) = core::str::from_utf8(&buf[..path_len]) else {
        return SYSERR;
    };
    chdir_path(path)
}

pub(crate) fn chdir_path(path: &str) -> usize {
    let Some(real) = resolve_copied_path(path) else {
        return SYSERR;
    };
    let Some(info) = fs::stat(&real) else {
        return SYSERR;
    };
    if (info.mode & fs::S_IFMT) != S_IFDIR {
        return SYSERR;
    }
    // cwd is kept in the task's own (possibly chrooted) view of the tree.
    let mut virt = [0u8; MAX_PATH];
    let Some(vn) = fs::resolve_user_path_virtual(path, &mut virt) else {
        return SYSERR;
    };
    if !task::set_cwd(&virt[..vn]) {
        return SYSERR;
    }
    0
}

/// mkfifo(path, mode): create a named pipe. Only tmpfs (`/tmp`) supports
/// FIFOs; the mode is not tracked (everything is root on myos).
fn sys_mkfifo(path_ptr: usize, path_len: usize, _mode: usize) -> usize {
    let Some(path) = copy_resolved_user_path(path_ptr, path_len) else {
        return SYSERR;
    };
    if fs::stat(&path).is_some() || !fs::vfs::mkfifo(&path) {
        return SYSERR;
    }
    0
}

/// chroot(2): make `path` (a directory) the caller's `/`. Everything is root
/// on myos, so there is no privilege check. The cwd keeps pointing at the
/// same directory when it lies inside the new root, and moves to the new `/`
/// otherwise (so `..` from the cwd cannot walk out of the jail).
fn sys_chroot(path_ptr: usize, path_len: usize) -> usize {
    let Some(buf) = copy_user_path(path_ptr, path_len) else {
        return SYSERR;
    };
    let Ok(path) = core::str::from_utf8(&buf[..path_len]) else {
        return SYSERR;
    };
    let Some(real) = resolve_copied_path(path) else {
        return SYSERR;
    };
    let Some(info) = fs::stat(&real) else {
        return SYSERR;
    };
    if (info.mode & fs::S_IFMT) != S_IFDIR || real.len() > task::ROOT_CAP {
        return SYSERR;
    }
    // Real path of the current cwd, to re-express it under the new root.
    let Some(real_cwd) = resolve_copied_path(".") else {
        return SYSERR;
    };
    if !task::set_root(real.as_bytes()) {
        return SYSERR;
    }
    let new_cwd = if real == "/" {
        real_cwd.as_str()
    } else if real_cwd == real {
        "/"
    } else if real_cwd.starts_with(real.as_str())
        && real_cwd.as_bytes().get(real.len()) == Some(&b'/')
    {
        &real_cwd[real.len()..]
    } else {
        "/"
    };
    if !task::set_cwd(new_cwd.as_bytes()) {
        return SYSERR;
    }
    0
}

pub(crate) fn sys_getcwd(buf_ptr: usize, buf_len: usize) -> usize {
    if buf_ptr == 0 || buf_len == 0 {
        return SYSERR;
    }
    if !user_range_ok(buf_ptr, buf_len) {
        return SYSERR;
    }
    let mut cwd = [0u8; MAX_PATH];
    let n = task::cwd(&mut cwd);
    // POSIX getcwd needs room for the pathname and a trailing NUL.
    if n + 1 > buf_len {
        return SYSERR;
    }
    let mut tmp = [0u8; MAX_PATH + 1];
    tmp[..n].copy_from_slice(&cwd[..n]);
    tmp[n] = 0;
    if !write_user_bytes(task::current_aspace(), buf_ptr, &tmp[..n + 1]) {
        return SYSERR;
    }
    n
}

/// A user path for calls that act on the last component itself (mkdir,
/// rmdir, unlink, rename, symlink, readlink, mkfifo): symlinks are followed
/// on the way there, not in the last component.
fn copy_resolved_user_path(ptr: usize, len: usize) -> Option<alloc::string::String> {
    let buf = copy_user_path(ptr, len)?;
    let path = core::str::from_utf8(&buf[..len]).ok()?;
    resolve_copied_path_nofollow(path)
}

/// Pack two lengths into one register: `(len_a << 16) | len_b` (each <= MAX_PATH).
fn unpack_two_lens(packed: usize) -> Option<(usize, usize)> {
    let a = packed >> 16;
    let b = packed & 0xffff;
    if a == 0 || a > MAX_PATH || b == 0 || b > MAX_PATH {
        return None;
    }
    Some((a, b))
}

fn sys_mkdir(path_ptr: usize, path_len: usize, _mode: usize) -> usize {
    let Some(path) = copy_resolved_user_path(path_ptr, path_len) else {
        return SYSERR;
    };
    if fs::mkdir(&path) { 0 } else { SYSERR }
}

fn sys_rmdir(path_ptr: usize, path_len: usize) -> usize {
    let Some(path) = copy_resolved_user_path(path_ptr, path_len) else {
        return SYSERR;
    };
    if fs::rmdir(&path) { 0 } else { SYSERR }
}

fn sys_unlink(path_ptr: usize, path_len: usize) -> usize {
    let Some(path) = copy_resolved_user_path(path_ptr, path_len) else {
        return SYSERR;
    };
    if fs::unlink(&path) { 0 } else { SYSERR }
}

/// `a0`=old_ptr, `a1`=new_ptr, `a2`=(old_len<<16)|new_len
fn sys_rename(old_ptr: usize, new_ptr: usize, packed_lens: usize) -> usize {
    let Some((old_len, new_len)) = unpack_two_lens(packed_lens) else {
        return SYSERR;
    };
    let Some(old) = copy_resolved_user_path(old_ptr, old_len) else {
        return SYSERR;
    };
    let Some(new) = copy_resolved_user_path(new_ptr, new_len) else {
        return SYSERR;
    };
    if fs::rename(&old, &new) { 0 } else { SYSERR }
}

/// `a0`=target_ptr, `a1`=link_ptr, `a2`=(target_len<<16)|link_len
fn sys_symlink(target_ptr: usize, link_ptr: usize, packed_lens: usize) -> usize {
    let Some((target_len, link_len)) = unpack_two_lens(packed_lens) else {
        return SYSERR;
    };
    // Target is opaque text (may be relative); do not cwd-resolve it.
    let Some(tbuf) = copy_user_path(target_ptr, target_len) else {
        return SYSERR;
    };
    let Ok(target) = core::str::from_utf8(&tbuf[..target_len]) else {
        return SYSERR;
    };
    let Some(linkpath) = copy_resolved_user_path(link_ptr, link_len) else {
        return SYSERR;
    };
    if fs::symlink(target, &linkpath) {
        0
    } else {
        SYSERR
    }
}

/// `a0`=path_ptr, `a1`=buf_ptr, `a2`=(path_len<<16)|buf_len
fn sys_readlink(path_ptr: usize, buf_ptr: usize, packed: usize) -> usize {
    // The buffer may be bigger than a path (libc passes `PATH_MAX`): only
    // the path length is bounded; the target fits in `MAX_PATH` bytes.
    let (path_len, buf_len) = (packed >> 16, packed & 0xffff);
    if path_len == 0 || path_len > MAX_PATH {
        return SYSERR;
    }
    if buf_ptr == 0 || buf_len == 0 || !user_range_ok(buf_ptr, buf_len) {
        return SYSERR;
    }
    let Some(path) = copy_resolved_user_path(path_ptr, path_len) else {
        return SYSERR;
    };
    let mut tmp = [0u8; MAX_PATH];
    let cap = buf_len.min(tmp.len());
    let Some(n) = fs::readlink(&path, &mut tmp[..cap]) else {
        return SYSERR;
    };
    if !write_user_bytes(task::current_aspace(), buf_ptr, &tmp[..n]) {
        return SYSERR;
    }
    n
}

fn sys_exec_name(buf: usize, len: usize) -> usize {
    if len == 0 || !user_range_ok(buf, len) {
        return SYSERR;
    }
    let mut tmp = [0u8; 32];
    let n = task::exec_name(&mut tmp).min(len).min(tmp.len());
    if n == 0 {
        return 0;
    }
    if !write_user_bytes(task::current_aspace(), buf, &tmp[..n]) {
        return SYSERR;
    }
    n
}

pub(crate) fn sys_brk(req: usize) -> usize {
    let (base, _span, stack_off) = task::current_user_map();
    let heap_base = heap_base_va(base, stack_off) as usize;
    let heap_limit = heap_limit_va(base, stack_off) as usize;
    let mut cur = task::current_brk() as usize;
    if cur == 0 {
        cur = heap_base;
        task::set_brk(cur as u64);
    }
    if req == 0 {
        return cur;
    }
    if req < heap_base || req > heap_limit {
        return cur;
    }
    if req > cur {
        let aspace = task::current_aspace();
        let map_end = align_up_usize(req, PAGE);
        let mut va = if cur == heap_base {
            heap_base
        } else {
            align_up_usize(cur, PAGE)
        };
        let mut mapped_any = false;
        while va < map_end {
            if virt_to_phys(aspace, va as u64).is_none() {
                let frame = mm::alloc_frame_site(4);
                // alloc_frame returns a zeroed frame.
                map_heap_page(aspace, va as u64, frame);
                mapped_any = true;
            }
            va += PAGE;
        }
        // RISC-V requires SFENCE.VMA after invalid→valid PTE updates. mmap /
        // expand already flush; brk used to skip it. With on-demand heap (and
        // the 2 MiB TLS arena on aarch64/riscv) that left stale non-present
        // TLB entries → intermittent load faults mid-heap (HTTPS montmul).
        if mapped_any {
            flush_user_tlb();
        }
    } else if req < cur {
        // Shrink: free the pages above the new break, so mapped heap always
        // stays within [heap_base, brk) and exit/fork only walk that far.
        let aspace = task::current_aspace();
        let mut va = align_up_usize(req, PAGE);
        let end = align_up_usize(cur, PAGE);
        let freed_any = va < end;
        while va < end {
            free_mapped_page(aspace, va as u64);
            va += PAGE;
        }
        if freed_any {
            flush_user_tlb();
        }
    }
    task::set_brk(req as u64);
    req
}

/// `a0` points at a user `myos_mmap_args` {addr,len,prot,flags,fd,offset}.
fn sys_mmap(args_ptr: usize) -> usize {
    const N: usize = 6 * core::mem::size_of::<u64>();
    if !user_range_ok(args_ptr, N) {
        return SYSERR;
    }
    let mut raw = [0u8; N];
    if !read_user_bytes(task::current_aspace(), args_ptr, &mut raw) {
        return SYSERR;
    }
    let mut words = [0u64; 6];
    for i in 0..6 {
        let o = i * 8;
        words[i] = u64::from_le_bytes(raw[o..o + 8].try_into().unwrap());
    }
    let hint = words[0] as usize;
    let len = words[1] as usize;
    let prot = words[2] as usize;
    let flags = words[3] as usize;
    let fd = words[4] as isize;
    let offset = words[5] as usize;
    do_mmap(hint, len, prot, flags, fd, offset)
}

pub(crate) fn do_mmap(hint: usize, len: usize, prot: usize, flags: usize, fd: isize, offset: usize) -> usize {
    if len == 0 {
        return SYSERR;
    }
    // File mappings are private copies: the pages are filled from the file
    // when first touched and never written back. A shared mapping of a
    // device (a module's `mmap` hook: `/dev/fb/data`) maps the device's own
    // pages at once.
    let file = if flags & MAP_ANON != 0 {
        None
    } else {
        if offset % PAGE != 0 || fd < 0 {
            return SYSERR;
        }
        match task::fd_file_node(fd as usize) {
            Some(node) => Some(node),
            None => return SYSERR,
        }
    };
    let device = match &file {
        Some(node) if flags & MAP_SHARED != 0 => fs::device_frame(node, offset).is_some(),
        _ => false,
    };
    if !device && flags & MAP_PRIVATE == 0 && flags & MAP_FIXED == 0 {
        // Require PRIVATE or FIXED; tcc uses MAP_PRIVATE|MAP_ANON.
        return SYSERR;
    }
    let (base, _span, stack_off) = task::current_user_map();
    let area_lo = mmap_base_va(base, stack_off) as usize;
    let area_hi = mmap_limit_va(base, stack_off) as usize;
    let pages = len.div_ceil(PAGE);
    if pages == 0 || pages > MMAP_AREA_PAGES {
        return SYSERR;
    }
    if let (true, Some(node)) = (device, &file) {
        // Every page must be the device's (none past its end).
        if !(0..pages).all(|i| fs::device_frame(node, offset + i * PAGE).is_some()) {
            return SYSERR;
        }
    }
    let map_len = pages * PAGE;
    let aspace = task::current_aspace();
    let va = if flags & MAP_FIXED != 0 {
        if hint == 0 || hint % PAGE != 0 {
            return SYSERR;
        }
        if hint < area_lo || hint.saturating_add(map_len) > area_hi {
            return SYSERR;
        }
        // MAP_FIXED replaces whatever is mapped there (a dynamic linker maps
        // each segment over the span it reserved first).
        let old = task::mmap_regions();
        if !task::mmap_remove(hint as u64, pages as u32) {
            return SYSERR;
        }
        release_mmap_range(aspace, &old, hint as u64, pages);
        hint
    } else {
        match task::mmap_alloc(area_lo, area_hi, map_len) {
            Some(v) => v,
            None => return SYSERR,
        }
    };
    if let (true, Some(node)) = (device, &file) {
        for i in 0..pages {
            if let Some(frame) = fs::device_frame(node, offset + i * PAGE) {
                map_user_page_prot(aspace, (va + i * PAGE) as u64, frame, prot);
            }
        }
        if !task::mmap_add(va as u64, pages as u32, prot as u32 | task::MMAP_DEVICE, None) {
            for i in 0..pages {
                unmap_user_page(aspace, (va + i * PAGE) as u64);
            }
            flush_user_tlb();
            return SYSERR;
        }
        flush_user_tlb();
        return va;
    }
    // The pages get their frames on first touch (`fault_in`), so a large
    // reservation or a big library costs only what is used.
    if task::mmap_add(va as u64, pages as u32, prot as u32, file.as_ref().map(|node| (node, offset))) {
        flush_user_tlb();
        return va;
    }
    // A file mapping fails there when the mapped-file table is full: read
    // the file in whole now, as an anonymous region.
    let Some(node) = file else {
        return SYSERR;
    };
    let mut mapped = 0usize;
    while mapped < map_len {
        let page_va = (va + mapped) as u64;
        // alloc_frame returns a zeroed frame; past the end of the file it
        // stays zero.
        let frame = mm::alloc_frame_site(4);
        let dst = unsafe { core::slice::from_raw_parts_mut(mm::hhdm(frame), PAGE) };
        let _ = fs::read(&node, offset + mapped, dst);
        map_user_page_prot(aspace, page_va, frame, prot);
        sync_icache(page_va as usize, PAGE);
        sync_icache(mm::hhdm(frame) as usize, PAGE);
        mapped += PAGE;
    }
    if !task::mmap_add(va as u64, pages as u32, prot as u32, None) {
        for i in 0..pages {
            free_mapped_page(aspace, (va + i * PAGE) as u64);
        }
        flush_user_tlb();
        return SYSERR;
    }
    flush_user_tlb();
    va
}

pub(crate) fn sys_munmap(addr: usize, len: usize) -> usize {
    if addr % PAGE != 0 || len == 0 {
        return SYSERR;
    }
    let pages = len.div_ceil(PAGE);
    let map_len = pages * PAGE;
    // Only the mmap window. Allowing munmap of brk/code/stack punched
    // holes while brk_cur still covered them (load faults) and — worse —
    // `unmap_user_page` alone never returned frames to the freelist, which
    // drained riscv UEFI RAM across exec-heavy smoke and surfaced as random
    // user exceptions under HTTPS. Within the window, any range may be
    // unmapped (holes included), as POSIX allows.
    let (base, _span, stack_off) = task::current_user_map();
    let area_lo = mmap_base_va(base, stack_off) as usize;
    let area_hi = mmap_limit_va(base, stack_off) as usize;
    if addr < area_lo || addr.saturating_add(map_len) > area_hi {
        return SYSERR;
    }
    let old = task::mmap_regions();
    if !task::mmap_remove(addr as u64, pages as u32) {
        return SYSERR;
    }
    release_mmap_range(task::current_aspace(), &old, addr as u64, pages);
    flush_user_tlb();
    0
}

pub(crate) fn sys_mprotect(addr: usize, len: usize, prot: usize) -> usize {
    if addr % PAGE != 0 || len == 0 {
        return SYSERR;
    }
    let pages = len.div_ceil(PAGE);
    let map_len = pages * PAGE;
    let (base, _span, stack_off) = task::current_user_map();
    let lo = base as usize;
    let hi = mmap_limit_va(base, stack_off) as usize;
    if addr < lo || addr.saturating_add(map_len) > hi {
        return SYSERR;
    }
    let aspace = task::current_aspace();
    let area_lo = mmap_base_va(base, stack_off);
    let mut off = 0;
    while off < map_len {
        let va = (addr + off) as u64;
        let Some(phys) = virt_to_phys(aspace, va) else {
            // An mmap page not touched yet takes the new protection when
            // it is paged in.
            if va >= area_lo {
                off += PAGE;
                continue;
            }
            return SYSERR;
        };
        map_user_page_prot(aspace, va, phys, prot);
        if prot & PROT_EXEC != 0 {
            // mprotect RW→RX: clean D-cache, invalidate I-cache for this range.
            sync_icache(va as usize, PAGE);
            sync_icache(mm::hhdm(phys) as usize, PAGE);
        }
        off += PAGE;
    }
    let recorded = task::mmap_set_prot(addr as u64, pages as u32, prot as u32);
    flush_user_tlb();
    if recorded { 0 } else { SYSERR }
}

pub(crate) fn sys_lseek(fd: usize, offset: usize, whence: usize) -> usize {
    task::fd_lseek(fd, offset as i64, whence)
}

/// `a0` points at `{src_ptr, src_len, tgt_ptr, tgt_len, fstype_ptr, fstype_len}`.
fn sys_mount(args_ptr: usize) -> usize {
    const N: usize = 6 * core::mem::size_of::<usize>();
    if !user_range_ok(args_ptr, N) {
        return SYSERR;
    }
    let mut raw = [0u8; N];
    if !read_user_bytes(task::current_aspace(), args_ptr, &mut raw) {
        return SYSERR;
    }
    let mut words = [0usize; 6];
    for i in 0..6 {
        let o = i * core::mem::size_of::<usize>();
        words[i] = usize::from_le_bytes(
            raw[o..o + core::mem::size_of::<usize>()]
                .try_into()
                .unwrap(),
        );
    }
    let Some(src_buf) = copy_user_path(words[0], words[1]) else {
        return SYSERR;
    };
    let Some(tgt_buf) = copy_user_path(words[2], words[3]) else {
        return SYSERR;
    };
    let Some(fs_buf) = copy_user_path(words[4], words[5]) else {
        return SYSERR;
    };
    let Ok(src) = core::str::from_utf8(&src_buf[..words[1]]) else {
        return SYSERR;
    };
    let Ok(tgt) = core::str::from_utf8(&tgt_buf[..words[3]]) else {
        return SYSERR;
    };
    let Ok(fstype) = core::str::from_utf8(&fs_buf[..words[5]]) else {
        return SYSERR;
    };
    let Some(src) = resolve_copied_path(src) else {
        return SYSERR;
    };
    let Some(tgt) = resolve_copied_path(tgt) else {
        return SYSERR;
    };
    if fstype == "bind" {
        return if fs::vfs::bind(&src, &tgt) { 0 } else { SYSERR };
    }
    let Some(st) = fs::stat(&src) else {
        return SYSERR;
    };
    if st.mode & fs::S_IFMT != fs::S_IFBLK {
        return SYSERR;
    }
    let Some(dev) = fs::blk_id_from_path(&src) else {
        return SYSERR;
    };
    let prefix = tgt.trim_start_matches('/');
    if prefix.is_empty() || prefix.contains('/') || fstype.is_empty() {
        return SYSERR;
    }
    if fs::mount_fstype(dev, prefix, fstype, &src) {
        0
    } else {
        SYSERR
    }
}

/// The auxiliary vector for an image about to start with a foreign
/// personality (empty for native images): a libc startup (musl's static-PIE
/// one, or the dynamic linker of a dynamic program) finds the program
/// headers (PT_DYNAMIC, PT_TLS) through `AT_PHDR`, and the dynamic linker its
/// own load address through `AT_BASE`.
fn exec_auxv(elf_bytes: &[u8], base: u64, entry: usize, interp_base: Option<usize>) -> AuxV {
    let mut aux = AuxV::new();
    if !crate::personality::pending() {
        return aux;
    }
    let u16_at = |o: usize| elf_bytes.get(o..o + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as u64);
    let u32_at = |o: usize| elf_bytes.get(o..o + 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()) as u64);
    let u64_at = |o: usize| elf_bytes.get(o..o + 8).map_or(0, |b| u64::from_le_bytes(b.try_into().unwrap()));
    let phoff = u64_at(0x20);
    let phent = u16_at(0x36);
    let phnum = u16_at(0x38);
    let Ok(span) = elf::image_span(elf_bytes) else {
        return aux;
    };
    let bias = base.wrapping_sub(span.min_vaddr);
    // Where the program headers sit in memory: PT_PHDR, else the PT_LOAD
    // whose file range covers them.
    let mut phdr = 0u64;
    for i in 0..phnum {
        let p = (phoff + i * phent) as usize;
        let (ty, off, va, filesz) = (u32_at(p), u64_at(p + 8), u64_at(p + 16), u64_at(p + 32));
        if ty == 6 {
            phdr = va;
            break;
        }
        if ty == 1 && off <= phoff && phoff < off + filesz {
            phdr = va + (phoff - off);
        }
    }
    const AT_PHDR: usize = 3;
    const AT_PHENT: usize = 4;
    const AT_PHNUM: usize = 5;
    const AT_PAGESZ: usize = 6;
    const AT_ENTRY: usize = 9;
    const AT_CLKTCK: usize = 17;
    aux.push(AT_PHDR, (bias + phdr) as usize);
    aux.push(AT_PHENT, phent as usize);
    aux.push(AT_PHNUM, phnum as usize);
    aux.push(AT_PAGESZ, PAGE);
    aux.push(AT_ENTRY, entry);
    aux.push(AT_CLKTCK, 100);
    if let Some(b) = interp_base {
        const AT_BASE: usize = 7;
        aux.push(AT_BASE, b);
    }
    aux
}

/// For a foreign-personality exec of a dynamically linked program: the bytes
/// of its interpreter (`PT_INTERP`, e.g. `/lib/ld-musl-x86_64.so.1`), which
/// `sys_exec` maps next to it and starts instead. `Ok(None)` for a static
/// image or a native exec; `Err` if the interpreter cannot be read.
fn exec_interp(elf_bytes: &[u8]) -> Result<Option<alloc::borrow::Cow<'static, [u8]>>, ()> {
    use alloc::borrow::Cow;
    if !crate::personality::pending() {
        return Ok(None);
    }
    let Some(path) = elf::interp_path(elf_bytes) else {
        return Ok(None);
    };
    let path = core::str::from_utf8(path).map_err(|_| ())?;
    let real = resolve_copied_path(path).ok_or(())?;
    if let Some(b) = fs::lookup(&real) {
        return Ok(Some(Cow::Borrowed(b)));
    }
    const INTERP_MAX: usize = 16 << 20;
    fs::read_all(&real, INTERP_MAX).map(|v| Some(Cow::Owned(v))).ok_or(())
}
