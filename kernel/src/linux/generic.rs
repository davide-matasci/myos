//! The `asm-generic` syscall numbers and `struct stat` that aarch64 and
//! riscv64 share. No legacy calls (`open`, `fork`, `poll`, `dup2`, ...):
//! libc uses the `*at` / `clone` / `ppoll` / `dup3` forms.

use super::abi::{err, result, EFAULT, EINVAL, ENOMEM, ENOSYS, EPERM, ESRCH};
use super::signal as lsig;
use super::sys::{self, ret};
use crate::task;
use crate::user::{self, SyscallRegs};

pub fn syscall(nr: usize, a: [usize; 6], regs: &mut SyscallRegs, user_rip: usize, user_rsp: usize) -> usize {
    match nr {
        17 => ret(sys::getcwd(a[0], a[1])),
        23 => ret(sys::dup(a[0], 0)),
        24 => ret(sys::dup3(a[0], a[1], false)),
        25 => ret(sys::fcntl(a[0], a[1], a[2])),
        29 => ret(sys::ioctl(a[0], a[1], a[2])),
        34 => ret(sys::mkdirat(a[0], a[1])),
        35 => ret(sys::unlinkat(a[0], a[1], a[2])),
        36 => ret(sys::symlinkat(a[0], a[1], a[2])),
        38 | 276 => ret(sys::renameat(a[0], a[1], a[2], a[3])), // renameat, renameat2
        48 | 439 => ret(sys::faccessat(a[0], a[1])),          // faccessat, faccessat2
        49 => ret(sys::chdir(a[0])),
        50 => ret(sys::fchdir(a[0])),
        56 => ret(sys::openat(a[0], a[1], a[2])),
        57 => ret(sys::close(a[0])),
        59 => ret(sys::pipe2(a[0])),
        61 => ret(sys::getdents64(a[0], a[1], a[2])),
        62 => ret(sys::lseek(a[0], a[1], a[2])),
        63 => ret(sys::read(a[0], a[1], a[2])),
        64 => ret(sys::write(a[0], a[1], a[2])),
        65 => ret(sys::rw_vec(a[0], a[1], a[2], false)), // readv
        66 => ret(sys::rw_vec(a[0], a[1], a[2], true)),  // writev
        67 => ret(sys::pread(a[0], a[1], a[2], a[3])),
        73 => ret(sys::ppoll(a[0], a[1], a[2])),
        78 => ret(sys::readlinkat(a[0], a[1], a[2], a[3])),
        79 => ret(sys::fstatat(a[0], a[1], a[2], a[3])), // newfstatat
        80 => ret(sys::fstat(a[0], a[1])),
        93 | 94 => task::user_exit(a[0] as u8), // exit, exit_group
        96 => task::current_id(),               // set_tid_address
        99 => 0,                                // set_robust_list
        101 => ret(sys::nanosleep(a[0], false)),
        113 => ret(sys::clock_gettime(a[1])),
        115 => ret(sys::nanosleep(a[2], a[1] & 1 != 0)), // clock_nanosleep
        124 => {
            task::yield_now();
            0
        }
        129 | 130 => ret(sys::kill(a[0], a[1])), // kill, tkill
        131 => ret(sys::kill(a[1], a[2])),       // tgkill
        132 => ret(lsig::sigaltstack(a[1])),
        133 => lsig::rt_sigsuspend(a[0]),
        134 => ret(lsig::rt_sigaction(a[0], a[1], a[2])),
        135 => ret(lsig::rt_sigprocmask(a[0], a[1], a[2])),
        136 => ret(lsig::rt_sigpending(a[0])),
        137 => ret(lsig::rt_sigtimedwait(a[0], a[1], a[2])),
        139 => lsig::rt_sigreturn(regs),
        144 | 146 => 0, // setgid, setuid
        154 => result(user::sys_setpgid(a[0], a[1]), EPERM),
        155 => result(user::sys_getpgid(a[0]), ESRCH),
        156 => result(user::sys_getsid(a[0]), ESRCH),
        157 => result(user::sys_setsid(), EPERM),
        158 => 0, // getgroups: none
        160 => ret(sys::uname(a[0])),
        163 => ret(sys::prlimit(a[0], a[1])), // getrlimit
        164 => 0,                             // setrlimit
        166 => 0o022,                         // umask
        169 => result(user::sys_gettimeofday(a[0], a[1]), EFAULT),
        172 | 178 => task::current_id(), // getpid, gettid
        173 => task::current_ppid(),
        174..=177 => 0, // getuid, geteuid, getgid, getegid
        214 => user::sys_brk(a[0]),
        215 => result(user::sys_munmap(a[0], a[1]), EINVAL),
        220 => ret(sys::clone(a[0], a[1], user_rip, user_rsp)),
        221 => ret(sys::execve(a[0], a[1], a[2])),
        222 => ret(sys::mmap(a[0], a[1], a[2], a[3], a[4], a[5])),
        226 => result(user::sys_mprotect(a[0], a[1], a[2]), ENOMEM),
        233 => 0, // madvise
        260 => ret(sys::wait4(a[0], a[1], a[2], a[3])),
        261 => ret(sys::prlimit(a[1], a[3])),
        278 => ret(sys::getrandom(a[0], a[1])),
        _ => err(ENOSYS),
    }
}

/// The `asm-generic` `struct stat` (128 bytes).
pub fn stat_bytes(mode: u32, size: u64, ino: u64, nlink: u64, dev: u64) -> [u8; 128] {
    let mut b = [0u8; 128];
    let mut put = |off: usize, v: &[u8]| b[off..off + v.len()].copy_from_slice(v);
    put(0, &dev.to_le_bytes());
    put(8, &ino.to_le_bytes());
    put(16, &mode.to_le_bytes());
    put(20, &(nlink as u32).to_le_bytes());
    // uid, gid, rdev: 0.
    put(48, &size.to_le_bytes());
    put(56, &4096u32.to_le_bytes()); // st_blksize
    put(64, &size.div_ceil(512).to_le_bytes()); // st_blocks
    b
}

/// `siginfo` + `ucontext` up to `uc_mcontext`: the layout both arches use
/// (`uc_mcontext` 16-byte aligned after the 128-byte `uc_sigmask` area).
pub const SIGINFO_BYTES: usize = 128;
pub const UC_SIGMASK: usize = 40;
pub const UC_MCONTEXT: usize = 176;

/// The common head of the ucontext: `uc_stack.ss_flags = SS_DISABLE` and
/// the blocked mask.
pub fn uc_head(uc: &mut [u8], mask: u64) {
    uc[24..28].copy_from_slice(&2u32.to_le_bytes());
    uc[UC_SIGMASK..UC_SIGMASK + 8].copy_from_slice(&mask.to_le_bytes());
}
