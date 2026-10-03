//! x86_64: syscall numbers, `struct stat`, the signal frame, FXSAVE state
//! and the FS base (musl's thread pointer).


use super::abi::{err, result, EFAULT, EINVAL, ENOMEM, ENOSYS, EPERM, ESRCH};
use super::signal::{self as lsig, siginfo, Frame};
use super::net;
use super::sys::{self, ret};
use super::thread;
use crate::k::task;
use crate::k::user::{self, SyscallRegs};

pub const MACHINE: &[u8] = b"x86_64";

/// `struct sigaction` carries `sa_restorer` (musl always sets it).
pub const SIGACTION_HAS_RESTORER: bool = true;

/// `mov eax, 15 (rt_sigreturn); syscall`.
pub const TRAMP_CODE: &[u8] = &[0xb8, 0x0f, 0x00, 0x00, 0x00, 0x0f, 0x05];

// `SyscallRegs` block (see `syscall_entry`): user rsp, rip, r8, r9, rflags,
// r10, rdx, rsi, rdi.
const R_RSP: usize = 0;
const R_RIP: usize = 1;
const R_R8: usize = 2;
const R_R9: usize = 3;
const R_RFLAGS: usize = 4;
const R_R10: usize = 5;
const R_RDX: usize = 6;
const R_RSI: usize = 7;
const R_RDI: usize = 8;

/// Syscall arguments: rdi, rsi, rdx, r10, r8, r9.
pub fn args(regs: &SyscallRegs, a0: usize, a1: usize, a2: usize) -> [usize; 6] {
    [a0, a1, a2, regs.word(R_R10) as usize, regs.word(R_R8) as usize, regs.word(R_R9) as usize]
}

const AT_FDCWD: usize = -100isize as usize;

pub fn syscall(nr: usize, a: [usize; 6], regs: &mut SyscallRegs) -> usize {
    match nr {
        0 => ret(sys::read(a[0], a[1], a[2])),
        1 => ret(sys::write(a[0], a[1], a[2])),
        2 => ret(sys::openat(AT_FDCWD, a[0], a[1])),
        3 => ret(sys::close(a[0])),
        4 => ret(sys::fstatat(AT_FDCWD, a[0], a[1], 0)), // stat
        6 => ret(sys::fstatat(AT_FDCWD, a[0], a[1], sys::AT_SYMLINK_NOFOLLOW)), // lstat
        5 => ret(sys::fstat(a[0], a[1])),
        7 => ret(sys::poll(a[0], a[1], a[2] as i32 as isize)),
        8 => ret(sys::lseek(a[0], a[1], a[2])),
        9 => ret(sys::mmap(a[0], a[1], a[2], a[3], a[4], a[5])),
        10 => result(user::sys_mprotect(a[0], a[1], a[2]), ENOMEM),
        11 => result(user::sys_munmap(a[0], a[1]), EINVAL),
        12 => user::sys_brk(a[0]),
        13 => ret(lsig::rt_sigaction(a[0], a[1], a[2])),
        14 => ret(lsig::rt_sigprocmask(a[0], a[1], a[2])),
        15 => lsig::rt_sigreturn(regs),
        16 => ret(sys::ioctl(a[0], a[1], a[2])),
        17 => ret(sys::pread(a[0], a[1], a[2], a[3])),
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
        39 => task::current_pid(),
        41 => ret(net::socket(a[0], a[1])),
        42 => ret(net::connect(a[0], a[1], a[2])),
        43 | 50 | 288 => ret(net::no_listen(a[0])), // accept, listen, accept4
        44 => ret(net::sendto(a[0], a[1], a[2], a[4], a[5])),
        45 => ret(net::recvfrom(a[0], a[1], a[2], a[3], a[4], a[5])),
        46 => ret(net::sendmsg(a[0], a[1])),
        47 => ret(net::recvmsg(a[0], a[1], a[2])),
        48 => ret(net::shutdown(a[0], a[1])),
        49 | 54 => ret(net::ignored(a[0])), // bind, setsockopt
        51 => ret(net::getsockname(a[0], a[1], a[2])),
        52 => ret(net::getpeername(a[0], a[1], a[2])),
        55 => ret(net::getsockopt(a[0], a[1], a[2], a[3], a[4])),
        186 => task::current_tid(),
        56 => ret(thread::clone(regs, a[0], a[1], a[2], a[4], a[3])),
        57 | 58 => ret(sys::fork(regs)), // fork, vfork
        59 => ret(sys::execve(a[0], a[1], a[2])),
        60 => thread::exit(a[0]),
        231 => task::user_exit(a[0] as u8), // exit_group
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
        96 => result(user::sys_gettimeofday(a[0], a[1]), EFAULT),
        97 => ret(sys::prlimit(a[0], a[1])), // getrlimit
        102 | 104 | 107 | 108 => 0,          // getuid, getgid, geteuid, getegid
        105 | 106 | 160 => 0,                // setuid, setgid, setrlimit
        109 => result(user::sys_setpgid(a[0], a[1]), EPERM),
        110 => task::current_ppid(),
        111 => result(user::sys_getpgid(0), ESRCH), // getpgrp
        112 => result(user::sys_setsid(), EPERM),
        115 => 0, // getgroups: none
        121 => result(user::sys_getpgid(a[0]), ESRCH),
        124 => result(user::sys_getsid(a[0]), ESRCH),
        127 => ret(lsig::rt_sigpending(a[0])),
        128 => ret(lsig::rt_sigtimedwait(a[0], a[1], a[2])),
        130 => lsig::rt_sigsuspend(a[0]),
        131 => ret(lsig::sigaltstack(a[1])),
        158 => arch_prctl(a[0], a[1]),
        200 => ret(sys::kill(a[0], a[1])), // tkill
        201 => ret(sys::time(a[0])),
        202 => ret(thread::futex(a[0], a[1], a[2], a[3], a[5])),
        217 => ret(sys::getdents64(a[0], a[1], a[2])),
        218 => thread::set_tid_address(a[0]),
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
        271 => ret(sys::ppoll(a[0], a[1], a[2])),
        273 => 0, // set_robust_list
        292 => ret(sys::dup3(a[0], a[1], false)),
        293 => ret(sys::pipe2(a[0])),
        302 => ret(sys::prlimit(a[1], a[3])),
        318 => ret(sys::getrandom(a[0], a[1])),
        _ => err(ENOSYS),
    }
}

/// The x86_64 `struct stat` (144 bytes).
pub fn stat_bytes(mode: u32, size: u64, ino: u64, nlink: u64, dev: u64) -> [u8; 144] {
    let mut b = [0u8; 144];
    let mut put = |off: usize, v: &[u8]| b[off..off + v.len()].copy_from_slice(v);
    put(0, &dev.to_le_bytes());
    put(8, &ino.to_le_bytes());
    put(16, &nlink.to_le_bytes());
    put(24, &mode.to_le_bytes());
    // uid, gid: 0 (everything is root).
    put(48, &size.to_le_bytes());
    put(56, &4096u64.to_le_bytes()); // st_blksize
    put(64, &size.div_ceil(512).to_le_bytes()); // st_blocks
    b
}

// ---- thread pointer ---------------------------------------------------------

/// `arch_prctl`: the FS base is the core's per-task thread pointer.
fn arch_prctl(code: usize, addr: usize) -> usize {
    const ARCH_SET_FS: usize = 0x1002;
    const ARCH_GET_FS: usize = 0x1003;
    match code {
        ARCH_SET_FS => {
            task::tp::set(addr as u64);
            0
        }
        ARCH_GET_FS => {
            let v = task::tp::get();
            ret(sys::put(addr, &v.to_le_bytes()).map(|()| 0))
        }
        _ => err(EINVAL),
    }
}

// ---- FP / SSE -------------------------------------------------------------

pub use crate::k::task::fpu::{restore as fp_restore, save as fp_save, BYTES as FP_BYTES};

#[repr(C, align(16))]
struct Fx([u8; FP_BYTES]);

// ---- signal frame ---------------------------------------------------------

/// `struct ucontext` up to and including `uc_sigmask`.
const UC_BYTES: usize = 304;
const UC_MCONTEXT: usize = 40;
const UC_SIGMASK: usize = 296;
/// `uc_mcontext` (`struct sigcontext`) word indexes.
const MC_R8: usize = 0;
const MC_R9: usize = 1;
const MC_R10: usize = 2;
const MC_R11: usize = 3;
const MC_RDI: usize = 8;
const MC_RSI: usize = 9;
const MC_RDX: usize = 12;
const MC_RAX: usize = 13;
const MC_RCX: usize = 14;
const MC_RSP: usize = 15;
const MC_RIP: usize = 16;
const MC_EFLAGS: usize = 17;
const MC_CSGSFS: usize = 18;
const MC_OLDMASK: usize = 21;
const MC_FPSTATE: usize = 23;
/// Below the interrupted stack pointer: the SysV red zone.
const RED_ZONE: usize = 128;
/// User-changeable RFLAGS bits (CF PF AF ZF SF TF DF OF AC); IF stays set.
const FLAGS_USER: u64 = 0x40dd5;
const FLAGS_IF: u64 = 0x202;
/// Canonical user addresses end here (`sysret` to anything above faults in
/// the kernel).
const USER_TOP: u64 = 0x0000_8000_0000_0000;

fn put_u64(b: &mut [u8], off: usize, v: u64) {
    b[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

fn get_u64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}

/// Linux `rt_sigframe`: `[restorer][ucontext][siginfo]`, the FXSAVE image
/// 64-byte aligned above it, `rsp ≡ 8 (mod 16)` at handler entry.
pub fn deliver(regs: &mut SyscallRegs, f: &Frame) -> Option<usize> {
    let sp = regs.word(R_RSP) as usize;
    let fp_va = sp.checked_sub(RED_ZONE + FP_BYTES)? & !63;
    let info_va = fp_va.checked_sub(128)? & !15;
    let uc_va = info_va - UC_BYTES;
    let frame_va = uc_va - 8;

    let mut fx = Fx([0; FP_BYTES]);
    unsafe { fp_save(fx.0.as_mut_ptr()) };

    let mut uc = [0u8; UC_BYTES];
    uc[24..28].copy_from_slice(&2u32.to_le_bytes()); // uc_stack.ss_flags = SS_DISABLE
    let mut mc = [0u64; 32];
    mc[MC_R8] = regs.word(R_R8);
    mc[MC_R9] = regs.word(R_R9);
    mc[MC_R10] = regs.word(R_R10);
    mc[MC_R11] = regs.word(R_RFLAGS);
    mc[MC_RDI] = regs.word(R_RDI);
    mc[MC_RSI] = regs.word(R_RSI);
    mc[MC_RDX] = regs.word(R_RDX);
    mc[MC_RAX] = f.ret as u64;
    mc[MC_RCX] = f.pc as u64;
    mc[MC_RSP] = sp as u64;
    mc[MC_RIP] = f.pc as u64;
    mc[MC_EFLAGS] = regs.word(R_RFLAGS);
    mc[MC_CSGSFS] = f.arch;
    mc[MC_OLDMASK] = f.mask;
    mc[MC_FPSTATE] = fp_va as u64;
    // rbx, rbp, r12-r15 stay 0: the handler preserves them (SysV ABI), and
    // rt_sigreturn keeps the live ones.
    for (i, w) in mc.iter().enumerate() {
        put_u64(&mut uc, UC_MCONTEXT + i * 8, *w);
    }
    put_u64(&mut uc, UC_SIGMASK, f.mask);

    sys::put(fp_va, &fx.0).ok()?;
    sys::put(info_va, &siginfo(f.sig)).ok()?;
    sys::put(uc_va, &uc).ok()?;
    sys::put(frame_va, &(f.restorer as u64).to_le_bytes()).ok()?;

    regs.set_word(R_RDI, f.sig as u64);
    regs.set_word(R_RSI, info_va as u64);
    regs.set_word(R_RDX, uc_va as u64);
    regs.set_word(R_RSP, frame_va as u64);
    regs.set_word(R_RIP, f.handler as u64);
    // The ABI wants DF clear at function entry.
    regs.set_word(R_RFLAGS, regs.word(R_RFLAGS) & !0x400);
    Some(0)
}

/// Undo [`deliver`]: the restorer runs with `rsp` at the ucontext (the
/// handler's `ret` popped the restorer address). Returns `(rax, mask)`.
pub fn sigreturn(regs: &mut SyscallRegs) -> Option<(usize, u64)> {
    let uc_va = regs.word(R_RSP) as usize;
    let mut uc = [0u8; UC_BYTES];
    sys::get_bytes(uc_va, &mut uc).ok()?;
    let mc = |i: usize| get_u64(&uc, UC_MCONTEXT + i * 8);
    let (rip, rsp) = (mc(MC_RIP), mc(MC_RSP));
    if rip >= USER_TOP || rsp >= USER_TOP {
        return None;
    }
    let fp_va = mc(MC_FPSTATE) as usize;
    if fp_va != 0 {
        let mut fx = Fx([0; FP_BYTES]);
        sys::get_bytes(fp_va, &mut fx.0).ok()?;
        // MXCSR bits this CPU rejects would #GP in fxrstor: keep the valid
        // ones (MXCSR_MASK from a live FXSAVE; 0 means the default 0xffbf).
        let mut live = Fx([0; FP_BYTES]);
        unsafe { fp_save(live.0.as_mut_ptr()) };
        let mut valid = u32::from_le_bytes(live.0[28..32].try_into().unwrap());
        if valid == 0 {
            valid = 0xffbf;
        }
        let mxcsr = u32::from_le_bytes(fx.0[24..28].try_into().unwrap()) & valid;
        fx.0[24..28].copy_from_slice(&mxcsr.to_le_bytes());
        unsafe { fp_restore(fx.0.as_ptr()) };
    }
    regs.set_word(R_R8, mc(MC_R8));
    regs.set_word(R_R9, mc(MC_R9));
    regs.set_word(R_R10, mc(MC_R10));
    regs.set_word(R_RDI, mc(MC_RDI));
    regs.set_word(R_RSI, mc(MC_RSI));
    regs.set_word(R_RDX, mc(MC_RDX));
    regs.set_word(R_RSP, rsp);
    regs.set_word(R_RIP, rip);
    regs.set_word(R_RFLAGS, (mc(MC_EFLAGS) & FLAGS_USER) | FLAGS_IF);
    Some((mc(MC_RAX) as usize, get_u64(&uc, UC_SIGMASK)))
}
