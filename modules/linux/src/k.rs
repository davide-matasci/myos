//! The kernel as this module sees it: thin safe wrappers over
//! [`myos_abi::KernelApi`], grouped like the kernel modules the layer used to
//! call directly (`task`, `user`, `fs`, `signal`, `time`, `rng`) so the
//! syscall handlers read the same.

use alloc::string::String;

use myos_abi::{KernelApi, MYOS_MAX_TASKS, StrRef};

pub const MAX_TASKS: usize = MYOS_MAX_TASKS;

static mut API: Option<&'static KernelApi> = None;

pub(crate) fn set_api(api: &'static KernelApi) {
    unsafe {
        *core::ptr::addr_of_mut!(API) = Some(api);
    }
}

pub(crate) fn api() -> &'static KernelApi {
    unsafe { (*core::ptr::addr_of!(API)).expect("linux: API") }
}

fn sref(s: &str) -> StrRef {
    StrRef {
        ptr: s.as_ptr(),
        len: s.len(),
    }
}

// Native syscall numbers (`kernel/src/user/syscall.rs`, append-only).
const SYS_EXIT: usize = 1;
const SYS_READ: usize = 3;
const SYS_FORK: usize = 6;
const SYS_BRK: usize = 9;
const SYS_GETCWD: usize = 16;
const SYS_MUNMAP: usize = 24;
const SYS_MPROTECT: usize = 25;
const SYS_LSEEK: usize = 26;
const SYS_SETSID: usize = 29;
const SYS_SETPGID: usize = 30;
const SYS_GETPGID: usize = 31;
const SYS_GETSID: usize = 32;
const SYS_GETTIMEOFDAY: usize = 33;
const SYS_WAITPID: usize = 46;

/// A native syscall that does not read the register block.
fn native(nr: usize, a0: usize, a1: usize, a2: usize) -> usize {
    unsafe { (api().native_syscall)(nr, a0, a1, a2, core::ptr::null_mut()) }
}

pub mod user {
    use super::*;

    pub const PAGE: usize = myos_abi::MYOS_PAGE;
    pub const MAX_ARGC: usize = myos_abi::MYOS_MAX_ARGC;
    pub const MAX_ENVC: usize = myos_abi::MYOS_MAX_ENVC;
    pub const MAX_EXEC_STRINGS: usize = myos_abi::MYOS_MAX_EXEC_STRINGS;

    /// The saved user registers of the syscall being handled (the kernel's
    /// block; layout per arch, see the arch modules).
    pub struct SyscallRegs(pub *mut u64);

    impl SyscallRegs {
        pub fn word(&self, i: usize) -> u64 {
            unsafe { *self.0.add(i) }
        }
        pub fn set_word(&mut self, i: usize, v: u64) {
            unsafe { *self.0.add(i) = v }
        }
    }

    pub fn buffer_ok(ptr: usize, len: usize) -> bool {
        unsafe { (api().user_buffer_ok)(ptr, len) != 0 }
    }

    pub fn copy_to_user(ptr: usize, bytes: &[u8]) -> bool {
        unsafe { (api().copy_to_user)(ptr, bytes.as_ptr(), bytes.len()) == 0 }
    }

    pub fn copy_from_user(ptr: usize, out: &mut [u8]) -> bool {
        unsafe { (api().copy_from_user)(ptr, out.as_mut_ptr(), out.len()) == 0 }
    }

    pub fn open_path(path: &str, flags: usize) -> usize {
        unsafe { (api().open_path)(sref(path), flags) }
    }

    pub fn chdir_path(path: &str) -> usize {
        unsafe { (api().chdir_path)(sref(path)) }
    }

    pub fn do_mmap(addr: usize, len: usize, prot: usize, flags: usize, fd: isize, off: usize) -> usize {
        unsafe { (api().mmap)(addr, len, prot, flags, fd, off) }
    }

    fn resolve(path: &str, mode: u32) -> Option<String> {
        let mut b = [0u8; 256];
        let n = unsafe { (api().path_resolve)(sref(path), mode, b.as_mut_ptr(), b.len()) };
        if n < 0 {
            return None;
        }
        Some(String::from(core::str::from_utf8(&b[..n as usize]).ok()?))
    }

    /// The VFS path behind `path` (cwd, chroot and symlinks applied).
    pub fn resolve_copied_path(path: &str) -> Option<String> {
        resolve(path, myos_abi::MYOS_PATH_REAL)
    }

    /// Like [`resolve_copied_path`], a symlink in the last component not followed.
    pub fn resolve_copied_path_nofollow(path: &str) -> Option<String> {
        resolve(path, myos_abi::MYOS_PATH_REAL_NOFOLLOW)
    }

    /// Exec with the personality kept (`execve`): native result.
    pub fn exec_linux(path: &str, args: &[&[u8]], env: &[&[u8]]) -> usize {
        let to_refs = |v: &[&[u8]]| -> alloc::vec::Vec<StrRef> {
            v.iter().map(|s| StrRef { ptr: s.as_ptr(), len: s.len() }).collect()
        };
        let a = to_refs(args);
        let e = to_refs(env);
        unsafe { (api().personality_exec)(sref(path), a.as_ptr(), a.len(), e.as_ptr(), e.len()) }
    }

    pub fn sys_read(fd: usize, buf: usize, len: usize) -> usize {
        native(SYS_READ, fd, buf, len)
    }
    pub fn sys_mprotect(addr: usize, len: usize, prot: usize) -> usize {
        native(SYS_MPROTECT, addr, len, prot)
    }
    pub fn sys_munmap(addr: usize, len: usize) -> usize {
        native(SYS_MUNMAP, addr, len, 0)
    }
    pub fn sys_brk(req: usize) -> usize {
        native(SYS_BRK, req, 0, 0)
    }
    pub fn sys_gettimeofday(tv: usize, tz: usize) -> usize {
        native(SYS_GETTIMEOFDAY, tv, tz, 0)
    }
    pub fn sys_setpgid(pid: usize, pgid: usize) -> usize {
        native(SYS_SETPGID, pid, pgid, 0)
    }
    pub fn sys_getpgid(pid: usize) -> usize {
        native(SYS_GETPGID, pid, 0, 0)
    }
    pub fn sys_getsid(pid: usize) -> usize {
        native(SYS_GETSID, pid, 0, 0)
    }
    pub fn sys_setsid() -> usize {
        native(SYS_SETSID, 0, 0, 0)
    }
    pub fn sys_waitpid(status: usize, options: usize, pid: usize) -> usize {
        native(SYS_WAITPID, status, options, pid)
    }
    pub fn sys_lseek(fd: usize, off: usize, whence: usize) -> usize {
        native(SYS_LSEEK, fd, off, whence)
    }
    pub fn sys_getcwd(buf: usize, size: usize) -> usize {
        native(SYS_GETCWD, buf, size, 0)
    }
    /// fork: the child resumes after the syscall in `regs` with result 0.
    pub fn sys_fork(regs: &SyscallRegs) -> usize {
        unsafe { (api().native_syscall)(SYS_FORK, 0, 0, 0, regs.0) }
    }
}

pub mod task {
    use super::*;

    pub use myos_abi::MYOS_WAIT_ANY as WAIT_ANY;

    pub fn current_id() -> usize {
        unsafe { (api().current_tid)() }
    }
    pub fn current_tid() -> usize {
        current_id()
    }
    pub fn current_pid() -> usize {
        unsafe { (api().current_pid)() }
    }
    pub fn current_ppid() -> usize {
        unsafe { (api().current_ppid)() }
    }
    pub fn is_live_user(id: usize) -> bool {
        unsafe { (api().task_is_live_user)(id) != 0 }
    }
    pub fn yield_now() {
        unsafe { (api().task_yield)() }
    }
    /// `exit_group`: end the process.
    pub fn user_exit(code: u8) -> ! {
        native(SYS_EXIT, code as usize, 0, 0);
        loop {
            yield_now();
        }
    }
    pub fn thread_exit(code: u8) -> ! {
        unsafe { (api().thread_exit)(code) }
    }
    /// A thread resuming like the caller of the syscall in `regs` (result 0)
    /// on stack `sp`, with thread pointer `tls` (`None`: the caller's).
    pub fn spawn_thread_from(regs: &super::user::SyscallRegs, sp: usize, tls: Option<u64>) -> Option<usize> {
        let t = unsafe { (api().thread_spawn_from)(regs.0, sp, i32::from(tls.is_some()), tls.unwrap_or(0)) };
        (t >= 0).then_some(t as usize)
    }
    pub fn wait_seq() -> u64 {
        unsafe { (api().wait_seq)() }
    }
    pub fn block_until(key: usize, seq: u64, deadline_ns: u64) {
        unsafe { (api().block_until)(key, seq, deadline_ns) }
    }
    /// Sleep until the monotonic deadline (a signal may end it early).
    pub fn sleep_until(deadline_ns: u64, _any: bool) {
        unsafe { (api().task_sleep_until)(deadline_ns) }
    }

    pub enum AddrWait {
        Woken,
        Changed,
        TimedOut,
        Interrupted,
        Fault,
    }

    pub fn wait_addr(addr: usize, expected: u32, deadline: u64) -> AddrWait {
        match unsafe { (api().wait_addr)(addr, expected, deadline) } {
            myos_abi::MYOS_WAIT_WOKEN => AddrWait::Woken,
            myos_abi::MYOS_WAIT_CHANGED => AddrWait::Changed,
            myos_abi::MYOS_WAIT_TIMEOUT => AddrWait::TimedOut,
            myos_abi::MYOS_WAIT_INTERRUPTED => AddrWait::Interrupted,
            _ => AddrWait::Fault,
        }
    }
    pub fn wake_addr(addr: usize, max: usize) -> usize {
        unsafe { (api().wake_addr)(addr, max) }
    }

    pub enum FdKind {
        Tty,
        Pipe,
        File { size: usize },
    }

    pub fn fd_kind(fd: usize) -> Option<FdKind> {
        let mut size = 0usize;
        match unsafe { (api().fd_kind)(fd, &mut size) } {
            myos_abi::MYOS_FD_TTY => Some(FdKind::Tty),
            myos_abi::MYOS_FD_PIPE => Some(FdKind::Pipe),
            myos_abi::MYOS_FD_FILE => Some(FdKind::File { size }),
            _ => None,
        }
    }
    pub fn fd_poll_bits(fd: usize) -> Option<u32> {
        let b = unsafe { (api().fd_poll_bits)(fd) };
        (b >= 0).then_some(b as u32)
    }
    pub fn fd_dup_min(fd: usize, min: usize) -> Option<usize> {
        let n = unsafe { (api().fd_dup_min)(fd, min) };
        (n >= 0).then_some(n as usize)
    }
    pub fn fd_dup2(old: usize, new: usize) -> bool {
        unsafe { (api().fd_dup2)(old, new) == 0 }
    }
    pub fn fd_close(fd: usize) -> bool {
        unsafe { (api().fd_close)(fd) == 0 }
    }
    pub fn fd_write(fd: usize, buf: usize, len: usize) -> usize {
        unsafe { (api().fd_write)(fd, buf, len) }
    }
    pub fn fd_ioctl(fd: usize, req: usize, arg: usize) -> usize {
        unsafe { (api().fd_ioctl)(fd, req, arg) }
    }
    pub fn pipe_open() -> Option<(usize, usize)> {
        let (mut r, mut w) = (0usize, 0usize);
        (unsafe { (api().pipe_open)(&mut r, &mut w) } == 0).then_some((r, w))
    }
    /// Read at `pos` of the file behind `fd` (position left alone); `None`
    /// when the fd is not a file.
    pub fn fd_pread(fd: usize, pos: usize, out: &mut [u8]) -> Option<usize> {
        let n = unsafe { (api().fd_pread)(fd, pos, out.as_mut_ptr(), out.len()) };
        (n >= 0).then_some(n as usize)
    }

    pub fn signal_get_action(id: usize, sig: u32) -> (usize, u32, u32) {
        let (mut h, mut f, mut m) = (0usize, 0u32, 0u32);
        unsafe { (api().signal_get_action)(id, sig, &mut h, &mut f, &mut m) };
        (h, f, m)
    }
    pub fn signal_set_action(id: usize, sig: u32, handler: usize, flags: u32, mask: u32, tramp: usize) -> bool {
        unsafe { (api().signal_set_action)(id, sig, handler, flags, mask, tramp) == 0 }
    }
    pub fn signal_blocked(id: usize) -> u32 {
        unsafe { (api().signal_blocked)(id) }
    }
    pub fn signal_set_blocked_mask(id: usize, mask: u32) {
        unsafe { (api().signal_set_blocked)(id, mask) }
    }
    pub fn signal_pending(id: usize) -> u32 {
        unsafe { (api().signal_pending)(id) }
    }
    pub fn signal_take_from(id: usize, set: u32) -> Option<u32> {
        let s = unsafe { (api().signal_take)(id, set) };
        (s >= 0).then_some(s as u32)
    }

    pub mod fpu {
        pub const BYTES: usize = myos_abi::MYOS_FP_BYTES;
        /// # Safety
        /// `buf` is 16-byte aligned and `BYTES` long.
        pub unsafe fn save(buf: *mut u8) {
            unsafe { (super::api().fpu_save)(buf) }
        }
        /// # Safety
        /// As [`save`], holding an image this CPU accepts.
        pub unsafe fn restore(buf: *const u8) {
            unsafe { (super::api().fpu_restore)(buf) }
        }
    }

    /// The thread pointer (`arch_prctl`; x86_64 only, the others set TLS
    /// in `clone`).
    #[cfg(target_arch = "x86_64")]
    pub mod tp {
        pub fn get() -> u64 {
            unsafe { (super::api().thread_pointer_get)() }
        }
        pub fn set(v: u64) {
            unsafe { (super::api().thread_pointer_set)(v) }
        }
    }
}

pub mod fs {
    use super::*;

    pub const S_IFMT: u32 = 0o170000;

    pub struct StatInfo {
        pub mode: u32,
        pub size: usize,
        pub ino: usize,
        pub nlink: u32,
        pub dev: usize,
    }

    pub fn stat(path: &str) -> Option<StatInfo> {
        let mut st = myos_abi::PathStat::default();
        if unsafe { (api().vfs_stat)(sref(path), &mut st) } != 0 {
            return None;
        }
        Some(StatInfo {
            mode: st.mode,
            size: st.size as usize,
            ino: st.ino as usize,
            nlink: st.nlink,
            dev: st.dev as usize,
        })
    }
    pub fn listdir(path: &str, buf: &mut [u8]) -> usize {
        let n = unsafe { (api().vfs_listdir)(sref(path), buf.as_mut_ptr(), buf.len()) };
        n.max(0) as usize
    }
    pub fn mkdir(path: &str) -> bool {
        unsafe { (api().vfs_mkdir)(sref(path)) == 0 }
    }
    pub fn rmdir(path: &str) -> bool {
        unsafe { (api().vfs_rmdir)(sref(path)) == 0 }
    }
    pub fn unlink(path: &str) -> bool {
        unsafe { (api().vfs_unlink)(sref(path)) == 0 }
    }
    pub fn rename(old: &str, new: &str) -> bool {
        unsafe { (api().vfs_rename)(sref(old), sref(new)) == 0 }
    }
    pub fn symlink(target: &str, link: &str) -> bool {
        unsafe { (api().vfs_symlink)(sref(target), sref(link)) == 0 }
    }
    pub fn readlink(path: &str, buf: &mut [u8]) -> Option<usize> {
        let n = unsafe { (api().vfs_readlink)(sref(path), buf.as_mut_ptr(), buf.len()) };
        (n >= 0).then_some(n as usize)
    }
    /// The task's own absolute view of `path` (cwd applied, chroot-relative).
    pub fn resolve_user_path_virtual(path: &str, out: &mut [u8]) -> Option<usize> {
        let n = unsafe { (api().path_resolve)(sref(path), myos_abi::MYOS_PATH_VIRTUAL, out.as_mut_ptr(), out.len()) };
        (n >= 0).then_some(n as usize)
    }
}

pub mod signal {
    use super::*;

    pub const SYSERR_EINTR: usize = myos_abi::MYOS_SYSERR_EINTR;
    pub const HANDLER_IGN: usize = myos_abi::MYOS_HANDLER_IGN;
    pub const SIGSEGV: u32 = myos_abi::MYOS_SIGSEGV;

    pub fn interrupt_wait() -> bool {
        unsafe { (api().signal_interrupt_wait)() != 0 }
    }
    pub fn kill(pid: isize, sig: u32) -> bool {
        unsafe { (api().signal_kill)(pid, sig) == 0 }
    }
    pub fn sigsuspend(mask: u32) -> usize {
        unsafe { (api().signal_sigsuspend)(mask) }
    }
    pub fn terminate(sig: u32) -> ! {
        unsafe { (api().signal_terminate)(sig) }
    }
}

pub mod time {
    use super::*;

    pub fn monotonic_ns() -> u64 {
        unsafe { (api().monotonic_ns)() }
    }
    /// `(sec, usec)` wall clock, `None` without an RTC.
    pub fn timeval() -> Option<(i64, i64)> {
        let us = unsafe { (api().wall_time_us)() };
        (us != 0).then(|| ((us / 1_000_000) as i64, (us % 1_000_000) as i64))
    }
}

pub mod rng {
    use super::*;

    pub fn fill(out: &mut [u8]) {
        unsafe { (api().rng_fill)(out.as_mut_ptr(), out.len()) }
    }
}
