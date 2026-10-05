//! The `asm-generic` syscall numbers and `struct stat` that aarch64 and
//! riscv64 share. No legacy calls (`open`, `fork`, `poll`, `dup2`, ...):
//! libc uses the `*at` / `clone` / `ppoll` / `dup3` forms.

use super::abi::{err, result, EFAULT, EINVAL, ENOMEM, ENOSYS, EPERM, ESRCH};
use super::signal as lsig;
use super::net;
use super::sys::{self, ret};
use super::thread;
use crate::k::task;
use crate::k::user::{self, SyscallRegs};

pub fn syscall(nr: usize, a: [usize; 6], regs: &mut SyscallRegs) -> usize {
    match nr {
        17 => ret(sys::getcwd(a[0], a[1])),
        19 => ret(sys::eventfd2(a[0], a[1])),
        23 => ret(sys::dup(a[0], 0)),
        24 => ret(sys::dup3(a[0], a[1], false, a[2])),
        25 => ret(sys::fcntl(a[0], a[1], a[2])),
        29 => ret(sys::ioctl(a[0], a[1], a[2])),
        32 => ret(sys::flock(a[0])),
        34 => ret(sys::mkdirat(a[0], a[1])),
        35 => ret(sys::unlinkat(a[0], a[1], a[2])),
        36 => ret(sys::symlinkat(a[0], a[1], a[2])),
        38 | 276 => ret(sys::renameat(a[0], a[1], a[2], a[3])), // renameat, renameat2
        45 => ret(sys::truncate(a[0], a[1])),
        46 => ret(sys::ftruncate(a[0], a[1])),
        48 | 439 => ret(sys::faccessat(a[0], a[1])),          // faccessat, faccessat2
        49 => ret(sys::chdir(a[0])),
        50 => ret(sys::fchdir(a[0])),
        52 | 55 => ret(sys::fd_noop(a[0])),         // fchmod, fchown
        53 | 54 => ret(sys::path_noop(a[0], a[1])), // fchmodat, fchownat
        56 => ret(sys::openat(a[0], a[1], a[2])),
        57 => ret(sys::close(a[0])),
        59 => ret(sys::pipe2(a[0], a[1])),
        61 => ret(sys::getdents64(a[0], a[1], a[2])),
        62 => ret(sys::lseek(a[0], a[1], a[2])),
        63 => ret(sys::read(a[0], a[1], a[2])),
        64 => ret(sys::write(a[0], a[1], a[2])),
        65 => ret(sys::rw_vec(a[0], a[1], a[2], false)), // readv
        66 => ret(sys::rw_vec(a[0], a[1], a[2], true)),  // writev
        67 => ret(sys::pread(a[0], a[1], a[2], a[3])),
        68 => ret(sys::pwrite(a[0], a[1], a[2], a[3])),
        70 | 287 => ret(sys::pwritev(a[0], a[1], a[2], a[3])), // pwritev, pwritev2
        73 => ret(sys::ppoll(a[0], a[1], a[2])),
        78 => ret(sys::readlinkat(a[0], a[1], a[2], a[3])),
        79 => ret(sys::fstatat(a[0], a[1], a[2], a[3])), // newfstatat
        88 => ret(sys::utimensat(a[0], a[1], a[2], a[3])),
        80 => ret(sys::fstat(a[0], a[1])),
        82 | 83 => ret(sys::fd_noop(a[0])), // fsync, fdatasync
        93 => thread::exit(a[0]),
        94 => task::user_exit(a[0] as u8), // exit_group
        96 => thread::set_tid_address(a[0]),
        98 => ret(thread::futex(a[0], a[1], a[2], a[3], a[5])),
        99 => 0, // set_robust_list
        101 => ret(sys::nanosleep(a[0], false)),
        113 => ret(sys::clock_gettime(a[1])),
        115 => ret(sys::nanosleep(a[2], a[1] & 1 != 0)), // clock_nanosleep
        122 => 0, // sched_setaffinity: the core places tasks
        123 => ret(sys::sched_getaffinity(a[1], a[2])),
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
        161 => ret(sys::sethostname(a[0], a[1])),
        163 => ret(sys::prlimit(a[0], a[1])), // getrlimit
        164 => 0,                             // setrlimit
        166 => 0o022,                         // umask
        169 => result(user::sys_gettimeofday(a[0], a[1]), EFAULT),
        172 => task::current_pid(),
        178 => task::current_tid(),
        173 => task::current_ppid(),
        174..=177 => 0, // getuid, geteuid, getgid, getegid
        198 => ret(net::socket(a[0], a[1])),
        199 => ret(net::socketpair(a[0], a[1], a[3])),
        200 | 208 => ret(net::ignored(a[0])),             // bind, setsockopt
        201 | 202 | 242 => ret(net::no_listen(a[0])),     // listen, accept, accept4
        203 => ret(net::connect(a[0], a[1], a[2])),
        204 => ret(net::getsockname(a[0], a[1], a[2])),
        205 => ret(net::getpeername(a[0], a[1], a[2])),
        206 => ret(net::sendto(a[0], a[1], a[2], a[4], a[5])),
        207 => ret(net::recvfrom(a[0], a[1], a[2], a[3], a[4], a[5])),
        209 => ret(net::getsockopt(a[0], a[1], a[2], a[3], a[4])),
        210 => ret(net::shutdown(a[0], a[1])),
        211 => ret(net::sendmsg(a[0], a[1])),
        212 => ret(net::recvmsg(a[0], a[1], a[2])),
        214 => user::sys_brk(a[0]),
        215 => result(user::sys_munmap(a[0], a[1]), EINVAL),
        220 => ret(thread::clone(regs, a[0], a[1], a[2], a[3], a[4])),
        221 => ret(sys::execve(a[0], a[1], a[2])),
        222 => ret(sys::mmap(a[0], a[1], a[2], a[3], a[4], a[5])),
        226 => result(user::sys_mprotect(a[0], a[1], a[2]), ENOMEM),
        233 => ret(sys::madvise(a[0], a[1], a[2])),
        260 => ret(sys::wait4(a[0], a[1], a[2], a[3])),
        261 => ret(sys::prlimit(a[1], a[3])),
        278 => ret(sys::getrandom(a[0], a[1])),
        _ => err(ENOSYS),
    }
}

/// The `asm-generic` `struct stat` (128 bytes); `ctime` is the
/// modification time.
pub fn stat_bytes(mode: u32, size: u64, ino: u64, nlink: u64, dev: u64, atime: u64, mtime: u64) -> [u8; 128] {
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
    put(72, &atime.to_le_bytes()); // st_atime
    put(88, &mtime.to_le_bytes()); // st_mtime
    put(104, &mtime.to_le_bytes()); // st_ctime: the modification time
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
