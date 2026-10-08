//! Syscall numbers, the dispatcher entered from the per-arch trap paths,
//! and the `sys_*` implementations.

use super::*;

// The numbers only grow; a call that went keeps its number unused. Gone:
// 2 open, 5 exec, 8 listdir, 12 stat, 15 chdir, 17 mkdir, 18 rmdir,
// 19 unlink, 20 rename, 21 symlink, 22 readlink, 43 chroot (a namespace,
// `ns`), 44 mkfifo, 61 stat2, 62 utimens, 63 futimens, 66 stat3: the path
// calls are the `*at` ones (`at`, 70 on). 28 was `ioctl`: a device's state
// is its `ctl` file (docs/tty.md). 0 write and 3 read: `pwrite` and
// `pread` (82, 81) do both, at the file position or at an offset.
const SYS_EXIT: usize = 1;
const SYS_CLOSE: usize = 4;
pub(super) const SYS_FORK: usize = 6;
const SYS_WAIT: usize = 7;
const SYS_BRK: usize = 9;
const SYS_PIPE: usize = 10;
const SYS_DUP2: usize = 11;
const SYS_EXECNAME: usize = 13;
const SYS_DUPFD: usize = 14;
const SYS_GETCWD: usize = 16;
const SYS_MMAP: usize = 23;
const SYS_MUNMAP: usize = 24;
const SYS_MPROTECT: usize = 25;
const SYS_LSEEK: usize = 26;
const SYS_MOUNT: usize = 27;
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
/// `getppid()`: the calling process's parent.
const SYS_GETPPID: usize = 60;
/// `umount(path, len)`: detach the block-device mount at `path`.
const SYS_UMOUNT: usize = 64;
/// `settimeofday(tv)`: set the wall clock to `tv` (two `i64`s, seconds and
/// microseconds, as `gettimeofday` writes them). The RTC is not written.
const SYS_SETTIMEOFDAY: usize = 65;
/// `setuser(buf, len)`: `buf` is the user's name, a NUL and the password.
/// Run the caller as that user in their login domain (docs/security.md).
const SYS_SETUSER: usize = 67;
/// `ns(spec, len)`: replace the caller's namespace by the bindings of
/// `spec`, one per line: `TARGET SOURCE RIGHTS` (`/dev/sda /dev/sda
/// read,write`), the sources named in the current namespace, the rights
/// at most the current ones there (`task::ns`).
const SYS_NS: usize = 68;
/// `policy_load(path, len)`: read the policy at `path` and make it the
/// system's (`write` on `kernel.policy`).
const SYS_POLICY_LOAD: usize = 69;
/// `pread(fd, buf, len, offset, flags)`: read up to `len` bytes into
/// `buf`, at the file position, which advances, or with [`FILE_AT`] at
/// `offset`, the position left as it is (`ESPIPE` on a pipe or terminal).
/// The bytes read; 0 at the end of the file.
const SYS_PREAD: usize = 81;
/// `pwrite(fd, buf, len, offset, flags)`: [`SYS_PREAD`]'s write (at the
/// end with `O_APPEND`, unless [`FILE_AT`]). The bytes written.
const SYS_PWRITE: usize = 82;
/// `ftruncate(fd, size)`: make the file `fd` is open on for writing `size`
/// bytes long, cut or grown with zeros.
const SYS_FTRUNCATE: usize = 83;
/// `fdflags(fd, op, flags)`: the fd's own flags ([`FD_CLOEXEC`]):
/// [`FD_GET`] returns them, [`FD_SET`] replaces them.
const SYS_FDFLAGS: usize = 84;
/// `flock(fd, op)`: [`LOCK_SH`] or [`LOCK_EX`] the whole file for the
/// fd's open file description, or [`LOCK_UN`]; with [`LOCK_NB`] `EAGAIN`
/// rather than a wait when someone else holds it (`fs::lock`).
const SYS_FLOCK: usize = 85;
/// `lockctl(fd, cmd, lock)`: a record lock on the fd's file, `lock` a
/// `myos_abi::MyosLockRange` (kind, start, length, owner): [`LOCKCTL_GET`] writes the first conflicting lock into it
/// (or an unlock), [`LOCKCTL_SET`] sets it (`EAGAIN` when someone else's
/// conflicts), [`LOCKCTL_WAIT`] waits for it. The process owns the lock,
/// or with [`LOCKCTL_OFD`] the fd's open file description.
const SYS_LOCKCTL: usize = 86;
/// `power(action)`: power off, reboot or halt (`myos_abi::MYOS_POWER_*`,
/// `write` on `kernel.power`): the processes are stopped and the disks
/// unmounted first (`crate::power`). Returns only on failure.
const SYS_POWER: usize = 87;
/// `itimer(which, new, old)`: the process's interval timer `which` (only
/// [`ITIMER_REAL`], `SIGALRM` on the monotonic clock). `new` (if not 0)
/// points at two `u64`, the microseconds until it fires (0 disarms it) and
/// the interval it fires at from then on (0 = once); `old` (if not 0)
/// receives what was left of the timer before and its interval.
const SYS_ITIMER: usize = 88;
const ITIMER_REAL: usize = 0;
/// `set_tp(value)`: make `value` the calling thread's thread pointer (its
/// TLS base: the FS base on x86_64, `tpidr_el0` on aarch64, `tp` on
/// riscv64), as `thread_spawn`'s `tls` is a new thread's. x86_64 user code
/// cannot write the FS base itself.
const SYS_SET_TP: usize = 89;
/// `yield()`: let the other tasks ready on this CPU run first.
const SYS_YIELD: usize = 90;
/// `clock_monotonic()`: nanoseconds since boot on the monotonic clock (the
/// one timers and `/proc/cpu` count on), which setting the wall clock does
/// not move.
const SYS_CLOCK_MONOTONIC: usize = 91;
/// `msync(addr, len, flags)`: write the shared file mappings in the range
/// back to their files (the flags are taken and ignored: every msync is a
/// synchronous one).
const SYS_MSYNC: usize = 92;
const LOCK_SH: usize = 1;
const LOCK_EX: usize = 2;
const LOCK_NB: usize = 4;
const LOCK_UN: usize = 8;
const LOCKCTL_GET: usize = 0;
const LOCKCTL_SET: usize = 1;
const LOCKCTL_WAIT: usize = 2;
const LOCKCTL_OFD: usize = 0x10;
/// `pread`/`pwrite` flags: at the offset given, not the file position.
const FILE_AT: usize = 1;
/// `fdflags` operations, and its one flag: the fd closes at exec (also
/// `dupfd`'s and `pipe`'s flags argument).
const FD_GET: usize = 0;
const FD_SET: usize = 1;
const FD_CLOEXEC: usize = 1;

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
    const TP_REG: Option<usize> = crate::arch::SYSCALL_TP_REG;

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
    /// The fourth to sixth syscall arguments (the first three are the
    /// dispatcher's own); zeros for a call a module makes without a block
    /// (`KernelApi::native_syscall`).
    pub fn args_3_5(&self) -> [usize; 3] {
        if self.0.is_null() {
            return [0; 3];
        }
        crate::arch::SYSCALL_ARGS_3_5.map(|i| unsafe { *self.0.add(i) as usize })
    }
    pub fn nr_reg(&self) -> usize {
        Self::NR_REG.map_or(0, |i| unsafe { *self.0.add(i) as usize })
    }
    pub fn set_nr_reg(&mut self, v: usize) {
        if let Some(i) = Self::NR_REG {
            unsafe { *self.0.add(i) = v as u64 }
        }
    }
    /// The thread pointer the return path loads, where it is in the block
    /// (riscv64's `tp`; elsewhere `task::tp` sets it).
    pub fn set_tp(&mut self, v: usize) {
        if let (Some(i), false) = (Self::TP_REG, self.0.is_null()) {
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
        SYS_EXIT => sys_exit(a0),
        SYS_CLOSE => sys_close(a0),
        SYS_FORK => sys_fork(regs),
        SYS_WAIT => sys_wait(a0, a1),
        SYS_WAITPID => sys_waitpid(a0, a1, a2),
        SYS_BRK => sys_brk(a0),
        SYS_PIPE => sys_pipe(a0, a1),
        SYS_DUP2 => sys_dup2(a0, a1),
        SYS_DUPFD => sys_dupfd(a0, a1, a2),
        SYS_EXECNAME => sys_exec_name(a0, a1),
        SYS_GETCWD => sys_getcwd(a0, a1),
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
        SYS_GETPPID => task::current_ppid(),
        SYS_UMOUNT => sys_umount(a0, a1),
        SYS_SETTIMEOFDAY => sys_settimeofday(a0),
        SYS_SETUSER => sys_setuser(a0, a1),
        SYS_NS => sys_ns(a0, a1),
        SYS_POLICY_LOAD => sys_policy_load(a0, a1),
        SYS_PREAD | SYS_PWRITE => {
            let [offset, flags, _] = regs.args_3_5();
            let at = (flags & FILE_AT != 0).then_some(offset);
            if nr == SYS_PREAD { task::fd_read(a0, a1, a2, at) } else { task::fd_write(a0, a1, a2, at) }
        }
        SYS_FTRUNCATE => {
            if task::fd_set_size(a0, a1) { 0 } else { SYSERR }
        }
        SYS_FDFLAGS => sys_fdflags(a0, a1, a2),
        SYS_FLOCK => sys_flock(a0, a1),
        SYS_LOCKCTL => sys_lockctl(a0, a1, a2),
        SYS_POWER => sys_power(a0),
        SYS_ITIMER => sys_itimer(a0, a1, a2),
        SYS_SET_TP => {
            task::tp::set(a0 as u64);
            regs.set_tp(a0);
            0
        }
        SYS_YIELD => {
            task::yield_now();
            0
        }
        SYS_CLOCK_MONOTONIC => crate::time::monotonic_ns() as usize,
        SYS_MSYNC => sys_msync(a0, a1),
        at::SYS_OPENAT..=at::SYS_EXECAT => {
            let [a3, a4, a5] = regs.args_3_5();
            match nr {
                at::SYS_OPENAT => at::sys_openat(a0, a1, a2, a3),
                at::SYS_STATAT => at::sys_statat(a0, a1, a2, a3, a4),
                at::SYS_MKNODAT => at::sys_mknodat(a0, a1, a2, a3),
                at::SYS_SYMLINKAT => at::sys_symlinkat(a0, a1, a2, a3, a4),
                at::SYS_UNLINKAT => at::sys_unlinkat(a0, a1, a2, a3),
                at::SYS_RENAMEAT => at::sys_renameat(a0, a1, a2, a3, a4, a5),
                at::SYS_READLINKAT => at::sys_readlinkat(a0, a1, a2, a3, a4),
                at::SYS_UTIMENSAT => at::sys_utimensat(a0, a1, a2, a3, a4),
                at::SYS_CHDIRAT => at::sys_chdirat(a0, a1, a2, a3),
                at::SYS_LISTDIRAT => at::sys_listdirat(a0, a1, a2, a3, a4, a5),
                _ => at::sys_execat(a0, a1, a2, a3, a4),
            }
        }
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
    let Some(tid) = task::spawn_thread(thread_start(regs, entry, stack, arg), Some(tls)) else {
        return SYSERR;
    };
    task::place_thread(tid);
    tid
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

use crate::sec::Rights;

/// May the current process do `need` to `real` (a real path)? Its domain's
/// rights under the policy, narrowed by its namespace (docs/security.md).
pub(super) fn may(real: &str, need: Rights) -> bool {
    crate::sec::allowed(real, need)
}

/// The rights `open` `flags` ask for on an existing file.
fn open_rights(flags: u32) -> Rights {
    let mut need = Rights::NONE;
    if !fs::open_writable(flags) || flags & 3 == 2 {
        need = need | Rights::READ;
    }
    if fs::open_writable(flags) {
        need = need | if fs::open_append(flags) { Rights::APPEND } else { Rights::WRITE };
    }
    if flags & O_TRUNC != 0 {
        need = need | Rights::WRITE;
    }
    need
}

const O_CREAT: u32 = 0o100;
const O_EXCL: u32 = 0o200;
const O_TRUNC: u32 = 0o1000;
/// The new fd closes at exec.
const O_CLOEXEC: u32 = 0o2000000;
/// A symlink in the last component is not followed but refused (`ELOOP`).
const O_NOFOLLOW: u32 = 0o400000;
/// Only a directory opens (`ENOTDIR`).
const O_DIRECTORY: u32 = 0o200000;
const S_IFLNK: u32 = 0o120000;

pub(super) fn copy_user_path(ptr: usize, len: usize) -> Option<[u8; MAX_PATH]> {
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
    if !may(&path, Rights::READ) || !crate::sec::allowed_object("kernel.modules", None, Rights::WRITE) {
        return SYSERR;
    }
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
    if !crate::sec::allowed_object("kernel.modules", None, Rights::WRITE) {
        return SYSERR;
    }
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
    let excl = open_excl(flags);
    let tree = if excl { fs::vfs::hold_write() } else { fs::vfs::hold_read() };
    let real = if open_follows(flags) { resolve_copied_path(path) } else { resolve_copied_path_nofollow(path) };
    match real {
        Some(real) => open_real(real, None, flags, tree),
        None => SYSERR,
    }
}

/// Whether open `flags` ask for a new file (`O_CREAT|O_EXCL`): the caller
/// holds the tree for writing from resolving to creating, so that no one
/// else creates the name in between, and does not follow a symlink there
/// (one at the name means it is taken).
pub(super) fn open_excl(flags: usize) -> bool {
    flags as u32 & (O_CREAT | O_EXCL) == O_CREAT | O_EXCL
}

/// Whether an open with `flags` follows a symlink in the last component:
/// not for `O_NOFOLLOW` (a symlink there is `ELOOP`) nor for a new file
/// ([`open_excl`]).
pub(super) fn open_follows(flags: usize) -> bool {
    !open_excl(flags) && flags as u32 & O_NOFOLLOW == 0
}

/// Open the file at `path` (real, resolved) for the caller, with the tree
/// held since it was resolved (`tree`; let go before a wait). `cap`: the
/// rights of the directory fd it was found beneath, for a file the
/// caller's namespace cannot name (see `at`); the new fd grants them too.
pub(super) fn open_real(
    path: alloc::string::String,
    cap: Option<Rights>,
    flags: usize,
    tree: fs::vfs::TreeGuard,
) -> usize {
    // A new file needs `create` (and what it is opened for) on the label
    // its path gives it.
    let mut need = open_rights(flags as u32);
    let exists = fs::stat(&path).is_some();
    if flags as u32 & O_CREAT != 0 && !exists {
        need = need | Rights::CREATE;
    }
    let rights = cap.unwrap_or_else(|| task::ns_rights(&path));
    if !crate::sec::allowed_in(&path, need, rights) {
        return SYSERR;
    }
    if exists && open_excl(flags) {
        return SYSERR_EEXIST;
    }
    // Not followed (`open_follows`): what the name is itself.
    let kind = fs::stat(&path).map(|st| st.mode & fs::S_IFMT);
    if flags as u32 & O_NOFOLLOW != 0 && kind == Some(S_IFLNK) {
        return SYSERR_ELOOP;
    }
    if flags as u32 & O_DIRECTORY != 0 && kind.is_some_and(|k| k != S_IFDIR) {
        return SYSERR_ENOTDIR;
    }
    // A directory is written through its calls (mkdir, unlink, ...), never
    // opened for it (libc: EISDIR).
    if kind == Some(S_IFDIR) && (fs::open_writable(flags as u32) || flags as u32 & (O_CREAT | O_TRUNC) != 0) {
        return SYSERR;
    }
    // An `append` grant without `write`: the fd appends and does no more.
    let append_only = need.contains(Rights::APPEND) && !crate::sec::rights_in(&path, rights).contains(Rights::WRITE);
    let fd = open_fd(path, rights, flags, tree);
    if fd < task::MAX_FDS && flags as u32 & O_CLOEXEC != 0 {
        task::fd_set_cloexec(fd, true);
    }
    if fd < task::MAX_FDS && append_only {
        task::fd_set_append_only(fd);
    }
    fd
}

/// [`open_real`] once the checks passed: the fd, of whatever kind the file
/// makes it.
fn open_fd(path: alloc::string::String, rights: Rights, flags: usize, tree: fs::vfs::TreeGuard) -> usize {
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
    // Named FIFO: the fd is a pipe end, not a file vnode. Opening one waits
    // for its other end: not with the tree held.
    if let Some(id) = fs::vfs::fifo_id(&path) {
        drop(tree);
        return match task::fd_open_fifo(id, flags as u32) {
            Ok(fd) => fd,
            Err(task::FifoOpenErr::NoReader) => SYSERR_ENXIO,
            Err(task::FifoOpenErr::Failed) => SYSERR,
        };
    }
    let Some(node) = fs::open(&path, flags as u32) else {
        return SYSERR;
    };
    match task::fd_open(node, flags as u32, rights) {
        Some(fd) => fd,
        None => SYSERR,
    }
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

fn sys_settimeofday(tv_ptr: usize) -> usize {
    if !crate::sec::allowed_object("kernel.clock", None, Rights::WRITE) {
        return SYSERR;
    }
    let mut raw = [0u8; 16];
    if !user_range_ok(tv_ptr, raw.len()) || !read_user_bytes(task::current_aspace(), tv_ptr, &mut raw) {
        return SYSERR;
    }
    let secs = i64::from_le_bytes(raw[..8].try_into().unwrap());
    let usec = i64::from_le_bytes(raw[8..].try_into().unwrap());
    if crate::time::set_wall(secs, usec) { 0 } else { SYSERR }
}

fn sys_setuser(ptr: usize, len: usize) -> usize {
    let mut buf = [0u8; 256];
    if len == 0 || len > buf.len() || !user_range_ok(ptr, len) || !read_user_bytes(task::current_aspace(), ptr, &mut buf[..len]) {
        return SYSERR;
    }
    let buf = &buf[..len];
    let (name, password) = match buf.iter().position(|&b| b == 0) {
        Some(i) => (&buf[..i], &buf[i + 1..]),
        None => (buf, &[][..]),
    };
    let Ok(name) = core::str::from_utf8(name) else {
        return SYSERR;
    };
    if crate::sec::setuser(name, password) { 0 } else { SYSERR }
}

/// Longest [`SYS_NS`] spec and policy file read.
const NS_SPEC_MAX: usize = 4096;
const POLICY_MAX: usize = 64 * 1024;

fn sys_ns(ptr: usize, len: usize) -> usize {
    let mut spec = alloc::vec![0u8; len.min(NS_SPEC_MAX)];
    if len == 0 || len > NS_SPEC_MAX || !user_range_ok(ptr, len) || !read_user_bytes(task::current_aspace(), ptr, &mut spec) {
        return SYSERR;
    }
    let Ok(spec) = core::str::from_utf8(&spec) else {
        return SYSERR;
    };
    let mut binds = Vec::new();
    for line in spec.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let mut w = line.split_whitespace();
        let (Some(target), Some(source), rights) = (w.next(), w.next(), w.next().unwrap_or("all")) else {
            return SYSERR;
        };
        let Some(rights) = Rights::parse_list(rights) else {
            return SYSERR;
        };
        let mut t = [0u8; MAX_PATH];
        let Some(tn) = fs::vfs::resolve_against_cwd("/", target, &mut t) else {
            return SYSERR;
        };
        // The source as the caller names it now, with no more rights than
        // the caller has there.
        let Some(real) = resolve_copied_path(source) else {
            return SYSERR;
        };
        if real.starts_with('@') || fs::stat(&real).is_none() {
            return SYSERR;
        }
        let target = alloc::string::String::from(core::str::from_utf8(&t[..tn]).unwrap_or("/"));
        let rights = rights & task::ns_rights(&real);
        binds.push(task::ns::Binding { target, source: real, rights });
    }
    // The cwd stays where it is when the new namespace still names it, and
    // is its `/` otherwise (`chroot`).
    let ns = task::ns::Namespace { binds };
    let keeps = resolve_copied_path(".").is_some_and(|c| ns.to_virtual(&c).is_some());
    task::set_ns(Some(ns));
    if keeps { 0 } else { chdir_path("/") }
}

fn sys_policy_load(ptr: usize, len: usize) -> usize {
    let Some(buf) = copy_user_path(ptr, len) else {
        return SYSERR;
    };
    let Ok(path) = core::str::from_utf8(&buf[..len]) else {
        return SYSERR;
    };
    let Some(path) = resolve_copied_path(path) else {
        return SYSERR;
    };
    if !may(&path, Rights::READ) {
        return SYSERR;
    }
    let Some(text) = fs::read_all(&path, POLICY_MAX) else {
        return SYSERR;
    };
    let Ok(text) = core::str::from_utf8(&text) else {
        return SYSERR;
    };
    match crate::sec::load(text) {
        Ok(()) => 0,
        Err(e) => {
            crate::console::status_fail(&alloc::format!("policy {path}: {e}"));
            SYSERR
        }
    }
}

pub(crate) fn sys_setsid() -> usize {
    match task::setsid() {
        Some(sid) => sid,
        None => SYSERR,
    }
}

pub(crate) fn sys_setpgid(pid: usize, pgid: usize) -> usize {
    match task::setpgid(pid, pgid) {
        Ok(()) => 0,
        Err(task::SetpgidError::Execd) => SYSERR_EACCES,
        Err(task::SetpgidError::Refused) => SYSERR,
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

/// `setitimer`/`getitimer` for `ITIMER_REAL`: two little-endian u64s
/// (microseconds left, interval) read from `new_ptr`, the previous pair
/// written to `old_ptr`; either may be 0.
fn sys_itimer(which: usize, new_ptr: usize, old_ptr: usize) -> usize {
    if which != ITIMER_REAL {
        return SYSERR;
    }
    let aspace = task::current_aspace();
    let new = if new_ptr == 0 {
        None
    } else {
        let mut raw = [0u8; 16];
        if !user_range_ok(new_ptr, raw.len()) || !read_user_bytes(aspace, new_ptr, &mut raw) {
            return SYSERR;
        }
        let us = |b: &[u8]| u64::from_le_bytes(b.try_into().unwrap()).saturating_mul(1000);
        Some((us(&raw[..8]), us(&raw[8..])))
    };
    if old_ptr != 0 && !user_range_ok(old_ptr, 16) {
        return SYSERR;
    }
    let (left, every) = task::itimer_swap(task::current_pid(), new);
    if old_ptr != 0 {
        let mut raw = [0u8; 16];
        raw[..8].copy_from_slice(&left.div_ceil(1000).to_le_bytes());
        raw[8..].copy_from_slice(&(every / 1000).to_le_bytes());
        if !write_user_bytes(aspace, old_ptr, &raw) {
            return SYSERR;
        }
    }
    0
}

/// `kill(pid, sig)` — `pid` is interpreted as signed (`isize`) for pgid rules.
fn sys_power(action: usize) -> usize {
    let Some(action) = crate::power::Action::from_raw(action) else {
        return SYSERR;
    };
    if !crate::sec::allowed_object("kernel.power", None, Rights::WRITE) {
        return SYSERR;
    }
    crate::power::perform(action);
    SYSERR
}

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

/// Replace the current image with the ELF at `path` (cwd-relative or
/// absolute), or run the script there through its `#!` interpreter; returns
/// only on failure.
pub(crate) fn exec_path(path: &str, arg_refs: &[&[u8]], env_refs: &[&[u8]]) -> usize {
    exec_path_depth(path, arg_refs, env_refs, 0, None)
}

/// Longest `#!` line read (Linux reads 256 bytes).
const SHEBANG_MAX: usize = 256;

/// `#!interpreter [arg]`: exec the interpreter with the arg (if any) and the
/// script's path in front of the script's arguments (`argv[0]` dropped), as
/// other Unix kernels do. The interpreter has to be a program, not another
/// script.
/// A script's own `exec` rule wins over its interpreter's (`script_ctx`).
fn exec_script(
    script: &str,
    line: &[u8],
    arg_refs: &[&[u8]],
    env_refs: &[&[u8]],
    depth: u8,
    script_ctx: Option<crate::sec::Ctx>,
) -> usize {
    if depth > 0 {
        return SYSERR;
    }
    let line = &line[..line.len().min(SHEBANG_MAX)];
    let line = line.split(|&b| b == b'\n').next().unwrap_or(&[]).trim_ascii();
    let (interp, arg) = match line.iter().position(|b| b.is_ascii_whitespace()) {
        Some(i) => (&line[..i], line[i..].trim_ascii()),
        None => (line, &[][..]),
    };
    let Ok(interp) = core::str::from_utf8(interp) else {
        return SYSERR;
    };
    if interp.is_empty() {
        return SYSERR;
    }
    let mut args: Vec<&[u8]> = Vec::with_capacity(arg_refs.len() + 2);
    args.push(interp.as_bytes());
    if !arg.is_empty() {
        args.push(arg);
    }
    args.push(script.as_bytes());
    args.extend(arg_refs.iter().skip(1));
    exec_path_depth(interp, &args, env_refs, depth + 1, script_ctx)
}

fn exec_path_depth(
    path: &str,
    arg_refs: &[&[u8]],
    env_refs: &[&[u8]],
    depth: u8,
    script_ctx: Option<crate::sec::Ctx>,
) -> usize {
    // The program's name in the caller's view: what a script's interpreter
    // is handed (the real path may be one the caller's namespace lacks).
    let mut virt = [0u8; MAX_PATH];
    let Some(vn) = fs::resolve_user_path_virtual(None, path, &mut virt) else {
        return SYSERR;
    };
    let Ok(virt) = core::str::from_utf8(&virt[..vn]) else {
        return SYSERR;
    };
    let Some(path) = resolve_copied_path(path) else {
        return SYSERR;
    };
    // `exec` on the program (a script's interpreter too); its `exec` rule
    // may move the process into another domain (docs/security.md).
    if !may(&path, Rights::EXEC) {
        return SYSERR;
    }
    let new_ctx = script_ctx.or_else(|| crate::sec::exec_ctx(&path));
    let basename = path.rsplit('/').next().unwrap_or(path.as_str()).as_bytes();
    task::set_exec_name(basename);
    // A dynamically linked Linux program is mapped from its file, like the
    // shared objects its dynamic linker maps: that linker is the image.
    let mapped = mapped_program(&path);
    // Static lookup for rootfs (`/bin/tcc/tcc`); VFS read for tmpfs `tcc -o` output.
    // The file is read into the kernel heap; what gets mapped is still capped
    // by the image limits (`MAX_EXPAND_PAGES`). A successful exec never
    // returns, so the copy is freed explicitly before the new image runs.
    const EXEC_FILE_MAX: usize = 16 * 1024 * 1024;
    let mut owned: Option<Vec<u8>> = None;
    let bytes: &[u8] = if let Some(m) = &mapped {
        &m.interp
    } else if let Some(b) = fs::lookup(&path) {
        b
    } else if let Some(v) = fs::read_all(&path, EXEC_FILE_MAX) {
        owned.insert(v)
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
        owned.insert(v)
    };
    if let Some(line) = bytes.strip_prefix(b"#!") {
        // Only the line is kept (the script's file freed): the interpreter's
        // exec does not return here either.
        let line = line[..line.len().min(SHEBANG_MAX)].to_vec();
        drop(owned);
        return exec_script(virt, &line, arg_refs, env_refs, depth, new_ctx);
    }
    // Not a loadable ELF: fail before anything of the current image goes,
    // or the caller is left with no code to return to.
    if elf::image_span(bytes).is_err() {
        return SYSERR;
    }
    // A foreign-personality image that is dynamically linked also needs its
    // interpreter (the dynamic linker). Read it before the current image is
    // replaced, so a missing one fails the exec cleanly.
    let interp = match mapped {
        Some(_) => None,
        None => match exec_interp(bytes) {
            Ok(i) => i,
            Err(()) => return SYSERR,
        },
    };
    // A dynamically linked program is relocated by its dynamic linker (which
    // relocates itself).
    let relocate = interp.is_none() && mapped.is_none();
    // The old image goes away from here on: the process's other threads
    // end first.
    if !task::exec_alone() {
        return SYSERR;
    }
    if let Some(ctx) = new_ctx {
        task::set_sec_ctx(ctx);
    }
    // What the new image may not inherit (`FD_CLOEXEC`).
    task::fd_close_on_exec();
    task::mark_execd();
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
                    sync_shared(&old_mmap);
                    free_mmap_regions(cur_aspace, &old_mmap);
                    flush_user_tlb();
                    v
                } else {
                    // Free anonymous maps *before* expand remaps: when
                    // `stack_off` grows, the new code span can overlap the old
                    // mmap window and reuse_or_alloc would steal those frames.
                    sync_shared(&old_mmap);
                    free_mmap_regions(cur_aspace, &old_mmap);
                    task::clear_mmap();
                    flush_user_tlb();
                    if let Some(v) = expand_user_elf(cur_aspace, bytes, base_u, stack_off, relocate)
                        .map(|(entry, span, off)| (cur_aspace, entry, span, off))
                    {
                        v
                    } else if let Some(v) = load_user_elf(bytes, relocate) {
                        // Fresh aspace: reclaim code/stack/heap (mmap already
                        // freed), with the task on the new one already.
                        task::set_current_aspace(v.0);
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
    // A mapped program goes at the start of the mmap window; the dynamic
    // linker, the image, starts and finds it through AT_PHDR / AT_ENTRY.
    let program_at = mmap_base_va(base_u, off);
    // A foreign-personality image gets the SysV auxv its libc startup reads.
    let aux = match &mapped {
        Some(m) => match m.entry_at(program_at) {
            Some(program_entry) => exec_auxv(&m.head, program_at, program_entry, Some(base_u as usize)),
            None => return SYSERR,
        },
        None => exec_auxv(bytes, base_u, entry, interp_map.as_ref().map(|m| m.0)),
    };
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
    // Nothing of the file or of the dynamic linker's is read from here on.
    drop(owned);
    drop(interp);
    task::replace_user(aspace, entry, rsp, base_u, span, off, argc, argv);
    task::set_current_name(path.rsplit('/').next().unwrap_or(path.as_str()).as_bytes());
    if let Some(m) = &mapped {
        if !m.map(aspace, program_at) {
            task::user_exit(127);
        }
    }
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
pub(super) fn copy_user_exec_pack(args_ptr: usize) -> Result<(Vec<Vec<u8>>, Vec<Vec<u8>>), ()> {
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

fn sys_pipe(fds_ptr: usize, flags: usize) -> usize {
    if !user_range_ok(fds_ptr, 2 * core::mem::size_of::<usize>()) {
        return SYSERR;
    }
    let Some((r, w)) = task::pipe_open(flags & FD_CLOEXEC != 0) else {
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

fn sys_dupfd(oldfd: usize, minfd: usize, flags: usize) -> usize {
    match task::fd_dup_min(oldfd, minfd, flags & FD_CLOEXEC != 0) {
        Some(fd) => fd,
        None => SYSERR,
    }
}

fn lock_result(r: Result<(), fs::lock::Refused>) -> usize {
    match r {
        Ok(()) => 0,
        Err(fs::lock::Refused::Busy) => SYSERR_EAGAIN,
        Err(fs::lock::Refused::Interrupted) => crate::signal::SYSERR_EINTR,
    }
}

fn sys_flock(fd: usize, op: usize) -> usize {
    use fs::lock::{Kind, Owner, TO_END};
    let Some((node, desc)) = task::fd_lock_target(fd) else {
        return SYSERR;
    };
    let owner = Owner::Flock(desc);
    let kind = match op & !LOCK_NB {
        LOCK_SH => Kind::Shared,
        LOCK_EX => Kind::Exclusive,
        LOCK_UN => return lock_result(fs::lock::set(node, owner, None, 0, TO_END, false)),
        _ => return SYSERR,
    };
    match fs::lock::set(node, owner, Some(kind), 0, TO_END, false) {
        Err(fs::lock::Refused::Busy) if op & LOCK_NB == 0 => {
            // A conversion lets go of the old lock before it waits (as
            // Linux does), so two processes converting cannot wait for
            // each other forever.
            let _ = fs::lock::set(node, owner, None, 0, TO_END, false);
            lock_result(fs::lock::set(node, owner, Some(kind), 0, TO_END, true))
        }
        r => lock_result(r),
    }
}

fn sys_lockctl(fd: usize, cmd: usize, ptr: usize) -> usize {
    const N: usize = core::mem::size_of::<LockRange>();
    let mut raw = [0u8; N];
    if !buffer_ok(ptr, N) || !copy_from_user(task::current_aspace(), ptr, &mut raw) {
        return SYSERR;
    }
    let mut lock: LockRange = unsafe { core::mem::transmute(raw) };
    let ret = lockctl(fd, cmd, &mut lock);
    if ret == 0 && cmd & !LOCKCTL_OFD == LOCKCTL_GET {
        let out: [u8; N] = unsafe { core::mem::transmute(lock) };
        if !copy_to_user(task::current_aspace(), ptr, &out) {
            return SYSERR;
        }
    }
    ret
}

/// [`SYS_LOCKCTL`]'s lock (the module ABI's, which `KernelApi::fd_lockctl`
/// takes too).
type LockRange = myos_abi::MyosLockRange;

/// [`SYS_LOCKCTL`] with the lock in kernel memory.
pub(crate) fn lockctl(fd: usize, cmd: usize, lock: &mut LockRange) -> usize {
    use fs::lock::{Kind, Owner, TO_END};
    let Some((node, desc)) = task::fd_lock_target(fd) else {
        return SYSERR;
    };
    let owner = if cmd & LOCKCTL_OFD != 0 { Owner::Ofd(desc) } else { Owner::Process(task::current_pid()) };
    let kind = match lock.kind {
        0 => Some(Kind::Shared),
        1 => Some(Kind::Exclusive),
        2 => None,
        _ => return SYSERR,
    };
    let end = if lock.len == 0 { TO_END } else { lock.start.saturating_add(lock.len) };
    match cmd & !LOCKCTL_OFD {
        LOCKCTL_GET => {
            let Some(kind) = kind else {
                return SYSERR;
            };
            match fs::lock::conflict(node, owner, kind, lock.start, end) {
                Some(l) => {
                    lock.kind = if l.kind == Kind::Exclusive { 1 } else { 0 };
                    lock.start = l.start;
                    lock.len = if l.end == TO_END { 0 } else { l.end - l.start };
                    lock.pid = match l.owner {
                        Owner::Process(pid) => pid as i64,
                        _ => -1,
                    };
                }
                None => lock.kind = 2,
            }
            0
        }
        LOCKCTL_SET | LOCKCTL_WAIT => {
            let wait = cmd & !LOCKCTL_OFD == LOCKCTL_WAIT;
            lock_result(fs::lock::set(node, owner, kind, lock.start, end, wait))
        }
        _ => SYSERR,
    }
}

fn sys_fdflags(fd: usize, op: usize, flags: usize) -> usize {
    match op {
        FD_GET => match task::fd_cloexec(fd) {
            Some(cloexec) => if cloexec { FD_CLOEXEC } else { 0 },
            None => SYSERR,
        },
        FD_SET if task::fd_set_cloexec(fd, flags & FD_CLOEXEC != 0) => 0,
        _ => SYSERR,
    }
}

/// chdir(2) of a path already in kernel memory (the Linux layer).
pub(crate) fn chdir_path(path: &str) -> usize {
    at::chdir_at(at::AT_FDCWD, path, 0)
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
    // POSIX getcwd needs room for the pathname and a trailing NUL. The cwd
    // may be gone (removed).
    if n == 0 || n + 1 > buf_len {
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
                // Out of memory: grant only what is already mapped rather than
                // aborting the kernel. The break stops at this page, so the
                // caller's allocator sees the growth fall short (ENOMEM).
                let Some(frame) = mm::try_alloc_frame_user(4) else {
                    if mapped_any {
                        flush_user_tlb_added();
                    }
                    task::set_brk(va as u64);
                    return va;
                };
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
            flush_user_tlb_added();
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

/// The write back of the shared file mappings among `mmap` that touch
/// `[lo, hi)` (every one by default), before they go.
fn sync_shared_in(lo: usize, hi: usize) {
    for node in task::mmap_shared_files(lo, hi) {
        fs::pagecache::sync(&node);
    }
}

fn sync_shared(mmap: &[task::MmapRegion]) {
    if mmap.iter().any(|r| r.prot & task::MMAP_SHARED != 0) {
        sync_shared_in(0, usize::MAX);
    }
}

pub(crate) fn do_mmap(hint: usize, len: usize, prot: usize, flags: usize, fd: isize, offset: usize) -> usize {
    if len == 0 {
        return SYSERR;
    }
    // A private file mapping is a copy: the pages are filled from the file
    // when first touched and never written back. A shared one through a
    // writable fd is the file (the page cache's frames for its pages,
    // written back when the mapping goes: `MMAP_SHARED`); through a
    // read-only fd it is a private one that may not be written. A shared
    // mapping of a device (a module's `mmap` hook: `/dev/fb/data`) maps the
    // device's own pages at once.
    let file = if flags & MAP_ANON != 0 {
        None
    } else {
        if offset % PAGE != 0 || fd < 0 {
            return SYSERR;
        }
        // Mapping a file reads it.
        match task::fd_file_node(fd as usize).filter(|_| task::fd_readable(fd as usize)) {
            Some(node) => Some(node),
            None => return SYSERR,
        }
    };
    let device = match &file {
        Some(node) if flags & MAP_SHARED != 0 => fs::device_frame(node, offset).is_some(),
        _ => false,
    };
    let mut shared = 0;
    if file.is_some() && !device && flags & MAP_SHARED != 0 {
        if task::fd_writable(fd as usize) {
            shared = task::MMAP_SHARED;
        } else if prot & PROT_WRITE != 0 {
            return SYSERR;
        }
    } else if !device && flags & MAP_PRIVATE == 0 && flags & MAP_FIXED == 0 {
        // Require PRIVATE or FIXED; tcc uses MAP_PRIVATE|MAP_ANON.
        return SYSERR;
    }
    if flags & MAP_SHARED != 0 && flags & MAP_PRIVATE != 0 {
        return SYSERR;
    }
    let (base, _span, stack_off) = task::current_user_map();
    let area_lo = mmap_base_va(base, stack_off) as usize;
    let area_hi = mmap_limit_va(base, stack_off) as usize;
    let pages = len.div_ceil(PAGE);
    if pages == 0 || pages > MMAP_AREA_PAGES {
        return SYSERR;
    }
    let map_len = pages * PAGE;
    // A file or device mapping reads from `offset + (0..map_len)`: an offset
    // near usize::MAX would overflow that (a kernel abort, overflow checks
    // on), and one past the u32 page index the region stores (vm.rs
    // `fpage`) would be cut. Both are refused, as Linux's EINVAL.
    if file.is_some() && (offset.checked_add(map_len).is_none() || offset / PAGE > u32::MAX as usize) {
        return SYSERR;
    }
    if let (true, Some(node)) = (device, &file) {
        // Every page must be the device's (none past its end).
        if !(0..pages).all(|i| fs::device_frame(node, offset + i * PAGE).is_some()) {
            return SYSERR;
        }
    }
    let aspace = task::current_aspace();
    let fixed = flags & MAP_FIXED != 0;
    if fixed {
        if hint == 0 || hint % PAGE != 0 {
            return SYSERR;
        }
        if hint < area_lo || hint.saturating_add(map_len) > area_hi {
            return SYSERR;
        }
        // MAP_FIXED replaces whatever is mapped there (a dynamic linker maps
        // each segment over the span it reserved first). Its pages go before
        // its record, as in `sys_munmap`.
        sync_shared_in(hint, hint + map_len);
        let old = task::mmap_regions();
        release_mmap_range(aspace, &old, hint as u64, pages);
        if !task::mmap_remove(hint as u64, pages as u32) {
            return SYSERR;
        }
    }
    // Record the region, at `hint` or in the lowest free gap (found and
    // recorded in one step: another thread may be mapping too), before its
    // pages are mapped: its address, or `None` when the table is full.
    let record = |prot: u32, file: Option<(&fs::Vnode, usize)>| {
        if fixed {
            task::mmap_add(hint as u64, pages as u32, prot, file).then_some(hint)
        } else {
            task::mmap_add_free(area_lo, area_hi, pages as u32, prot, file)
        }
    };
    // Only a MAP_FIXED one replaces mappings: the others only add some.
    let flush = || if fixed { flush_user_tlb() } else { flush_user_tlb_added() };
    if let (true, Some(node)) = (device, &file) {
        let Some(va) = record(prot as u32 | task::MMAP_DEVICE, None) else {
            return SYSERR;
        };
        for i in 0..pages {
            if let Some(frame) = fs::device_frame(node, offset + i * PAGE) {
                map_user_page_prot(aspace, (va + i * PAGE) as u64, frame, prot);
            }
        }
        flush();
        return va;
    }
    // The pages get their frames on first touch (`fault_in`), so a large
    // reservation or a big library costs only what is used.
    if let Some(va) = record(prot as u32 | shared, file.as_ref().map(|node| (node, offset))) {
        flush();
        return va;
    }
    // A file mapping fails there when the mapped-file table is full: read
    // the file in whole now, as an anonymous region (not a shared one,
    // which is the file or nothing).
    let Some(node) = file.filter(|_| shared == 0) else {
        return SYSERR;
    };
    let Some(va) = record(prot as u32, None) else {
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
    flush();
    va
}

/// `madvise(MADV_DONTNEED)`: free the frames of `[addr, addr + len)` in the
/// mmap window but keep its regions, so a page reads as new on its next
/// touch (`fault_in`: zeroed, or from the file). Allocators release memory
/// this way and count on it reading as zero afterwards. A device's pages
/// stay.
pub(crate) fn mmap_discard(addr: usize, len: usize) -> bool {
    if addr % PAGE != 0 {
        return false;
    }
    let pages = len.div_ceil(PAGE);
    // No range past the window is valid; checked first, the multiply below
    // cannot overflow (a user length near usize::MAX would).
    if pages > MMAP_AREA_PAGES {
        return false;
    }
    let (base, _span, stack_off) = task::current_user_map();
    let area_lo = mmap_base_va(base, stack_off) as usize;
    let area_hi = mmap_limit_va(base, stack_off) as usize;
    if addr < area_lo || addr.saturating_add(pages * PAGE) > area_hi {
        return false;
    }
    let aspace = task::current_aspace();
    for i in 0..pages {
        let va = addr + i * PAGE;
        if task::mmap_backing(va).is_some_and(|(prot, _)| prot & task::MMAP_DEVICE == 0) {
            free_mapped_page(aspace, va as u64);
        }
    }
    flush_user_tlb();
    true
}

pub(crate) fn sys_munmap(addr: usize, len: usize) -> usize {
    if addr % PAGE != 0 || len == 0 {
        return SYSERR;
    }
    let pages = len.div_ceil(PAGE);
    if pages > MMAP_AREA_PAGES {
        return SYSERR;
    }
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
    // The pages go first, the region's record only after them: once the
    // record is gone another thread's `mmap` may get the range, and must not
    // find this mapping's pages still in it (it would write to a frame freed
    // here a moment later, and read a new zeroed page after). A split past
    // the table's limit then fails with the pages dropped: the range reads
    // as new, as after `madvise`.
    let old = task::mmap_regions();
    // A shared mapping's pages go back to the file (whether or not another
    // mapping of the process still holds them).
    sync_shared_in(addr, addr + map_len);
    release_mmap_range(task::current_aspace(), &old, addr as u64, pages);
    flush_user_tlb();
    if !task::mmap_remove(addr as u64, pages as u32) {
        return SYSERR;
    }
    0
}

/// `msync(addr, len, flags)`: the shared file mappings in `[addr, addr +
/// len)` are written back to their files. The flags (`MS_SYNC`, `MS_ASYNC`,
/// `MS_INVALIDATE`) make no difference: the write back is done when the
/// call returns, and the mappings are the file.
pub(crate) fn sys_msync(addr: usize, len: usize) -> usize {
    if addr % PAGE != 0 {
        return SYSERR;
    }
    let pages = len.div_ceil(PAGE);
    if pages > MMAP_AREA_PAGES {
        return SYSERR;
    }
    let (base, _span, stack_off) = task::current_user_map();
    let area_lo = mmap_base_va(base, stack_off) as usize;
    let area_hi = mmap_limit_va(base, stack_off) as usize;
    if addr < area_lo || addr.saturating_add(pages * PAGE) > area_hi {
        return SYSERR;
    }
    sync_shared_in(addr, addr + pages * PAGE);
    0
}

pub(crate) fn sys_mprotect(addr: usize, len: usize, prot: usize) -> usize {
    if addr % PAGE != 0 || len == 0 {
        return SYSERR;
    }
    let pages = len.div_ceil(PAGE);
    if pages > MMAP_AREA_PAGES {
        return SYSERR;
    }
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
        let Some(mut phys) = virt_to_phys(aspace, va) else {
            // An mmap page not touched yet takes the new protection when
            // it is paged in.
            if va >= area_lo {
                off += PAGE;
                continue;
            }
            return SYSERR;
        };
        // A page shared through the page cache becomes this process's own
        // before it may be written; a shared mapping's stays the file's,
        // dirty from now on.
        if prot & PROT_WRITE != 0 && fs::pagecache::is_cached(phys) {
            if task::mmap_backing(va as usize).is_some_and(|(p, _)| p & task::MMAP_SHARED != 0) {
                fs::pagecache::dirtied(phys);
            } else {
                let own = mm::alloc_frame_site(4);
                unsafe { core::ptr::copy_nonoverlapping(mm::hhdm(phys), mm::hhdm(own), PAGE) };
                free_mapped_page(aspace, va);
                phys = own;
            }
        }
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
    // A mount changes the tree for every process: `write` on kernel.mounts,
    // and `mount` on the directory mounted over; a bind reads its source, a
    // disk is read and written.
    let source_need = if fstype == "bind" { Rights::READ } else { Rights::READ | Rights::WRITE };
    if !crate::sec::allowed_object("kernel.mounts", None, Rights::WRITE)
        || !may(&tgt, Rights::MOUNT)
        || !may(&src, source_need)
    {
        return SYSERR;
    }
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
    if fstype.is_empty() {
        return SYSERR;
    }
    if fs::mount_fstype(dev, tgt.trim_start_matches('/'), fstype, &src) {
        0
    } else {
        SYSERR
    }
}

/// `umount(2)`: `a0`/`a1` the mount point. 0, or `SYSERR` when nothing
/// that can be unmounted is mounted there or the mount is busy.
fn sys_umount(ptr: usize, len: usize) -> usize {
    let Some(buf) = copy_user_path(ptr, len) else {
        return SYSERR;
    };
    let Ok(path) = core::str::from_utf8(&buf[..len]) else {
        return SYSERR;
    };
    let Some(path) = resolve_copied_path(path) else {
        return SYSERR;
    };
    if !crate::sec::allowed_object("kernel.mounts", None, Rights::WRITE) || !may(&path, Rights::MOUNT) {
        return SYSERR;
    }
    if fs::vfs::unmount(&path) { 0 } else { SYSERR }
}

/// The auxiliary vector for an image about to start with a foreign
/// personality (empty for native images): a libc startup (musl's static-PIE
/// one, or the dynamic linker of a dynamic program) finds the program
/// headers (PT_DYNAMIC, PT_TLS) through `AT_PHDR`, and the dynamic linker its
/// own load address through `AT_BASE`.
/// A dynamically linked, position-independent Linux program that `exec`
/// maps from its file instead of reading it whole: only its headers and its
/// interpreter (the dynamic linker, loaded as the image) are read. Its
/// segments are paged in on first touch, so its size is bounded only by the
/// mmap window (Alpine's cargo spans 34 MiB).
struct MappedProgram {
    node: fs::Vnode,
    /// The start of the file: the ELF and program headers.
    head: Vec<u8>,
    interp: alloc::borrow::Cow<'static, [u8]>,
}

/// The headers of a program: what `exec` reads of a mapped one.
const PROGRAM_HEAD_MAX: usize = 64 * 1024;

/// `path` as a [`MappedProgram`], or `None` for any other exec (a native
/// one, a static or non-PIE program: read whole as before).
fn mapped_program(path: &str) -> Option<MappedProgram> {
    if !crate::personality::pending() {
        return None;
    }
    let node = fs::open(path, 0)?;
    let size = fs::size_of(&node)?;
    let mut head = alloc::vec![0u8; size.min(PROGRAM_HEAD_MAX)];
    let mut got = 0;
    while got < head.len() {
        let n = fs::read(&node, got, &mut head[got..]);
        if n == 0 {
            return None;
        }
        got += n;
    }
    if !elf::is_pie(&head) || elf::image_span(&head).is_err() {
        return None;
    }
    let (off, len) = elf::interp_range(&head)?;
    let mut name = alloc::vec![0u8; len.min(MAX_PATH)];
    if fs::read(&node, off, &mut name) != name.len() {
        return None;
    }
    let name = name.split(|&b| b == 0).next()?;
    let interp_path = resolve_copied_path(core::str::from_utf8(name).ok()?)?;
    // The dynamic linker runs as the image: `exec` on it, as on the program.
    if !may(&interp_path, Rights::EXEC) {
        return None;
    }
    let interp = match fs::lookup(&interp_path) {
        Some(b) => alloc::borrow::Cow::Borrowed(b),
        None => alloc::borrow::Cow::Owned(fs::read_all(&interp_path, 16 << 20)?),
    };
    Some(MappedProgram { node, head, interp })
}

impl MappedProgram {
    /// Its entry point when its lowest page is mapped at `at`.
    fn entry_at(&self, at: u64) -> Option<usize> {
        let span = elf::image_span(&self.head).ok()?;
        let lo = span.min_vaddr & !(PAGE as u64 - 1);
        Some(at.checked_add(span.entry.checked_sub(lo)?)? as usize)
    }

    /// Record its segments as mmap regions with their lowest page at `at`
    /// (the region table of the new image, so after `replace_user`). The
    /// whole pages of file data are paged in from the file; the page where a
    /// segment's file data ends is filled now, the rest of it zero (not the
    /// file's next bytes); what is left of its memory size is anonymous.
    fn map(&self, aspace: u64, at: u64) -> bool {
        let page = PAGE as u64;
        let Ok(span) = elf::image_span(&self.head) else {
            return false;
        };
        let Some(bias) = at.checked_sub(span.min_vaddr & !(page - 1)) else {
            return false;
        };
        let mut ok = true;
        let _ = elf::for_each_load_segment(&self.head, |seg| {
            let prot = elf::pf_to_prot(seg.flags) as u32;
            let lo = seg.vaddr & !(page - 1);
            let file_off = seg.offset - (seg.vaddr - lo);
            let file_end = if seg.filesz == 0 { lo } else { seg.vaddr + seg.filesz };
            let whole_end = file_end & !(page - 1);
            let mem_end = (seg.vaddr + seg.memsz).div_ceil(page) * page;
            let pages = |a: u64, b: u64| ((b - a) / page) as u32;
            if whole_end > lo {
                ok &= task::mmap_add(bias + lo, pages(lo, whole_end), prot, Some((&self.node, file_off as usize)));
            }
            let mut next = whole_end;
            if file_end > whole_end {
                // alloc_frame returns a zeroed frame.
                let frame = mm::alloc_frame_site(4);
                let len = (file_end - whole_end) as usize;
                let dst = unsafe { core::slice::from_raw_parts_mut(mm::hhdm(frame), len) };
                let _ = fs::read(&self.node, (file_off + (whole_end - lo)) as usize, dst);
                map_user_page_prot(aspace, bias + whole_end, frame, prot as usize);
                sync_icache(mm::hhdm(frame) as usize, PAGE);
                ok &= task::mmap_add(bias + whole_end, 1, prot, None);
                next += page;
            }
            if mem_end > next {
                ok &= task::mmap_add(bias + next, pages(next, mem_end), prot, None);
            }
        });
        flush_user_tlb();
        ok
    }
}

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
        // `off <= phoff && phoff < off + filesz`, written so a crafted
        // `filesz` cannot overflow the add; the aux values are handed to
        // userspace, so wrapping a malformed one only faults that program.
        if ty == 1 && off <= phoff && phoff - off < filesz {
            phdr = va.wrapping_add(phoff - off);
        }
    }
    const AT_PHDR: usize = 3;
    const AT_PHENT: usize = 4;
    const AT_PHNUM: usize = 5;
    const AT_PAGESZ: usize = 6;
    const AT_ENTRY: usize = 9;
    const AT_CLKTCK: usize = 17;
    aux.push(AT_PHDR, bias.wrapping_add(phdr) as usize);
    aux.push(AT_PHENT, phent as usize);
    aux.push(AT_PHNUM, phnum as usize);
    aux.push(AT_PAGESZ, PAGE);
    aux.push(AT_ENTRY, entry);
    aux.push(AT_CLKTCK, 100);
    // Every process is root. libc reads the ids here at startup, and musl
    // treats a process whose four are not all given as setuid ("secure"):
    // it ignores LD_LIBRARY_PATH and LD_PRELOAD.
    const AT_UID: usize = 11;
    const AT_EUID: usize = 12;
    const AT_GID: usize = 13;
    const AT_EGID: usize = 14;
    const AT_SECURE: usize = 23;
    for id in [AT_UID, AT_EUID, AT_GID, AT_EGID, AT_SECURE] {
        aux.push(id, 0);
    }
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
    // The dynamic linker runs as the image: `exec` on it, as on the program.
    if !may(&real, Rights::EXEC) {
        return Err(());
    }
    if let Some(b) = fs::lookup(&real) {
        return Ok(Some(Cow::Borrowed(b)));
    }
    const INTERP_MAX: usize = 16 << 20;
    fs::read_all(&real, INTERP_MAX).map(|v| Some(Cow::Owned(v))).ok_or(())
}
