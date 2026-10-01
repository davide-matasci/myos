//! x86_64 Linux syscall numbers and the FS base (musl's thread pointer).

use core::sync::atomic::{AtomicU64, Ordering};

use super::abi::{err, ENOSYS};
use super::sys::{self, ret};
use crate::task;
use crate::user;

const IA32_FS_BASE: u32 = 0xC000_0100;

/// The FS base each CPU has loaded, to skip redundant `wrmsr`s on switch.
static LOADED: [AtomicU64; crate::smp::MAX_CPUS] = [const { AtomicU64::new(0) }; crate::smp::MAX_CPUS];

pub fn load_fs_base(v: u64) {
    let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
    if LOADED[cpu].load(Ordering::Relaxed) == v {
        return;
    }
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") IA32_FS_BASE,
            in("eax") v as u32,
            in("edx") (v >> 32) as u32,
            options(nostack, preserves_flags),
        );
    }
    LOADED[cpu].store(v, Ordering::Relaxed);
}

const AT_FDCWD: usize = -100isize as usize;

pub fn dispatch(nr: usize, a: [usize; 6], user_rip: usize, user_rsp: usize) -> usize {
    match nr {
        0 => ret(sys::read(a[0], a[1], a[2])),
        1 => ret(sys::write(a[0], a[1], a[2])),
        2 => ret(sys::openat(AT_FDCWD, a[0], a[1])),
        3 => ret(sys::close(a[0])),
        4 | 6 => ret(sys::fstatat(AT_FDCWD, a[0], a[1], 0)), // stat, lstat
        5 => ret(sys::fstat(a[0], a[1])),
        7 => ret(sys::poll(a[0], a[1], a[2] as i32 as isize)),
        8 => ret(sys::lseek(a[0], a[1], a[2])),
        9 => ret(sys::mmap(a[0], a[1], a[2], a[3], a[4], a[5])),
        10 => sys_result(user::sys_mprotect(a[0], a[1], a[2]), super::abi::ENOMEM),
        11 => sys_result(user::sys_munmap(a[0], a[1]), super::abi::EINVAL),
        12 => user::sys_brk(a[0]),
        13 => ret(sys::rt_sigaction(a[0], a[1], a[2])),
        14 => ret(sys::rt_sigprocmask(a[0], a[1], a[2])),
        16 => ret(sys::ioctl(a[0], a[1], a[2])),
        19 => ret(sys::rw_vec(a[0], a[1], a[2], false)), // readv
        20 => ret(sys::rw_vec(a[0], a[1], a[2], true)),  // writev
        21 => ret(sys::faccessat(AT_FDCWD, a[0])),       // access
        22 => ret(sys::pipe2(a[0])),
        24 => {
            task::yield_now();
            0
        }
        28 => 0, // madvise
        32 => ret(sys::dup(a[0], 0)),
        33 => ret(sys::dup3(a[0], a[1], true)), // dup2
        35 => ret(sys::nanosleep(a[0], false)),
        39 | 186 => task::current_id(), // getpid, gettid
        56 => ret(sys::clone(a[0], a[1], user_rip, user_rsp)),
        57 | 58 => ret(sys::fork(user_rip, user_rsp)), // fork, vfork
        59 => ret(sys::execve(a[0], a[1], a[2])),
        60 | 231 => task::user_exit(a[0] as u8), // exit, exit_group
        61 => ret(sys::wait4(a[0], a[1], a[2], a[3])),
        62 => ret(sys::kill(a[0], a[1])),
        63 => ret(sys::uname(a[0])),
        72 => ret(sys::fcntl(a[0], a[1], a[2])),
        79 => ret(sys::getcwd(a[0], a[1])),
        80 => ret(sys::chdir(a[0])),
        81 => ret(sys::fchdir(a[0])),
        82 => ret(sys::renameat(AT_FDCWD, a[0], AT_FDCWD, a[1])),
        83 => ret(sys::mkdirat(AT_FDCWD, a[0])),
        84 => ret(sys::unlinkat(AT_FDCWD, a[0], 0x200)), // rmdir
        87 => ret(sys::unlinkat(AT_FDCWD, a[0], 0)),     // unlink
        88 => ret(sys::symlinkat(a[0], AT_FDCWD, a[1])),
        89 => ret(sys::readlinkat(AT_FDCWD, a[0], a[1], a[2])),
        95 => 0o022, // umask
        96 => sys_result(user::sys_gettimeofday(a[0], a[1]), super::abi::EFAULT),
        97 => ret(sys::prlimit(a[0], a[1])), // getrlimit
        102 | 104 | 107 | 108 => 0,          // getuid, getgid, geteuid, getegid
        105 | 106 | 160 => 0,                // setuid, setgid, setrlimit
        109 => sys_result(user::sys_setpgid(a[0], a[1]), super::abi::EPERM),
        110 => task::current_ppid(),
        111 => sys_result(user::sys_getpgid(0), super::abi::ESRCH), // getpgrp
        112 => sys_result(user::sys_setsid(), super::abi::EPERM),
        115 => 0, // getgroups: none
        121 => sys_result(user::sys_getpgid(a[0]), super::abi::ESRCH),
        124 => sys_result(user::sys_getsid(a[0]), super::abi::ESRCH),
        131 => 0, // sigaltstack
        158 => arch_prctl(a[0], a[1]),
        200 => ret(sys::kill(a[0], a[1])), // tkill
        201 => ret(sys::time(a[0])),
        217 => ret(sys::getdents64(a[0], a[1], a[2])),
        218 => task::current_id(), // set_tid_address
        228 => ret(sys::clock_gettime(a[1])),
        230 => ret(sys::nanosleep(a[2], a[1] & 1 != 0)), // clock_nanosleep
        234 => ret(sys::kill(a[1], a[2])),               // tgkill
        257 => ret(sys::openat(a[0], a[1], a[2])),
        258 => ret(sys::mkdirat(a[0], a[1])),
        262 => ret(sys::fstatat(a[0], a[1], a[2], a[3])),
        263 => ret(sys::unlinkat(a[0], a[1], a[2])),
        264 | 316 => ret(sys::renameat(a[0], a[1], a[2], a[3])), // renameat, renameat2
        266 => ret(sys::symlinkat(a[0], a[1], a[2])),
        267 => ret(sys::readlinkat(a[0], a[1], a[2], a[3])),
        269 | 439 => ret(sys::faccessat(a[0], a[1])), // faccessat, faccessat2
        273 => 0,                                     // set_robust_list
        292 => ret(sys::dup3(a[0], a[1], false)),
        293 => ret(sys::pipe2(a[0])),
        302 => ret(sys::prlimit(a[1], a[3])),
        318 => ret(sys::getrandom(a[0], a[1])),
        _ => err(ENOSYS),
    }
}

fn sys_result(r: usize, generic: usize) -> usize {
    super::abi::result(r, generic)
}

fn arch_prctl(code: usize, addr: usize) -> usize {
    const ARCH_SET_FS: usize = 0x1002;
    const ARCH_GET_FS: usize = 0x1003;
    match code {
        ARCH_SET_FS => {
            super::set_tp(addr as u64);
            0
        }
        ARCH_GET_FS => {
            let v = super::TP[task::current_id()].load(Ordering::Relaxed);
            if user::buffer_ok(addr, 8) && user::copy_to_user(task::current_aspace(), addr, &v.to_le_bytes()) {
                0
            } else {
                err(super::abi::EFAULT)
            }
        }
        _ => err(super::abi::EINVAL),
    }
}
