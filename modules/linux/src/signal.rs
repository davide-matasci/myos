//! Linux signal syscalls and handler delivery.
//!
//! Dispositions, masks and pending bits are the native ones
//! (`task::signal_*`, in native numbering); this module translates numbers
//! and structures at the boundary. A caught signal runs through the native
//! decision path (`crate::k::signal::on_syscall_exit`), which calls [`deliver`]
//! for a Linux task: the arch module writes a Linux `rt_sigframe` (siginfo,
//! ucontext with the interrupted registers, FP state) and `rt_sigreturn`
//! undoes it.
//!
//! The Linux signals the core has no number for (`SIGSTKFLT`, `SIGPWR`,
//! the real-time ones from 32) keep their dispositions here ([`EXTRA`]):
//! `sigaction` sets and reports them as Linux does, though nothing sends
//! them. Runtimes walk every signal at startup and stop at the first
//! refusal: Node resets 1 to 31 and aborts on a failed `sigaction`, Go
//! installs its handler on all 64.
//!
//! Lock order: the core calls `on_fork` and `on_spawn` with its task lock
//! held, so nothing may enter the core while holding [`EXTRA`].

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use myos_abi::Lock;

use super::abi::*;
use super::arch;
use super::sys::{get_u64, put, R};
use crate::k::MAX_TASKS;
use crate::k::task;
use crate::k::user::SyscallRegs;

const SA_RESTORER: usize = 0x0400_0000;
/// Linux's `SA_NOCLDWAIT`; the native bit differs (`signal::SA_NOCLDWAIT`).
const LINUX_SA_NOCLDWAIT: u32 = 0x2;
const SS_ONSTACK: u32 = 1;
const SS_DISABLE: u32 = 2;
/// `sigaltstack`'s flag to leave the stack off while a handler runs on it:
/// accepted, the stack staying on (as before Linux 4.7).
const SS_AUTODISARM: u32 = 1 << 31;
/// The smallest alternate stack `sigaltstack` takes (Linux's arm64 one,
/// the largest of the three arches').
const MINSIGSTKSZ: u64 = 5120;
/// A handler that runs on the alternate signal stack.
const SA_ONSTACK: u32 = 0x0800_0000;

/// A thread's alternate signal stack (`sigaltstack`): its lowest address and
/// size, 0 for none.
#[derive(Clone, Copy)]
struct AltStack {
    sp: u64,
    size: u64,
}

impl AltStack {
    const NONE: AltStack = AltStack { sp: 0, size: 0 };

    /// Whether the stack pointer `sp` is on this stack (it grows down, so
    /// its top belongs to it and its bottom does not).
    fn holds(&self, sp: usize) -> bool {
        let sp = sp as u64;
        self.size != 0 && sp > self.sp && sp - self.sp <= self.size
    }

    /// Linux's `stack_t` for it, seen from a thread whose stack pointer is
    /// `sp`: `ss_sp`, `ss_flags` (`SS_DISABLE` for none, `SS_ONSTACK` when
    /// on it) and `ss_size`.
    fn stack_t(&self, sp: usize) -> [u8; 24] {
        let flags = if self.size == 0 {
            SS_DISABLE
        } else if self.holds(sp) {
            SS_ONSTACK
        } else {
            0
        };
        let mut b = [0u8; 24];
        b[..8].copy_from_slice(&self.sp.to_le_bytes());
        b[8..12].copy_from_slice(&flags.to_le_bytes());
        b[16..24].copy_from_slice(&self.size.to_le_bytes());
        b
    }
}

/// Per thread: its alternate signal stack. A new thread starts without
/// one, a forked child has its parent thread's, exec drops it (as Linux).
static ALT: Lock<[AltStack; MAX_TASKS]> = Lock::new([AltStack::NONE; MAX_TASKS]);

/// Per process (leader slot): the user page holding [`arch::TRAMP_CODE`], mapped on
/// first use for handlers registered without `SA_RESTORER` (riscv64 musl
/// never passes one; real Linux uses its vDSO there). 0 = none yet.
static TRAMP: [AtomicU64; MAX_TASKS] = [const { AtomicU64::new(0) }; MAX_TASKS];
/// Per process: the `sa_restorer` last registered (reported in `oact`).
static RESTORER: [AtomicU64; MAX_TASKS] = [const { AtomicU64::new(0) }; MAX_TASKS];

/// The highest Linux signal number (`_NSIG - 1`).
const LINUX_SIGMAX: usize = 64;
/// Per process: the Linux `struct sigaction` words (handler, flags,
/// restorer, mask) of each signal the core has no number for and that is
/// not at its default; a signal absent is `SIG_DFL` with nothing else set.
static EXTRA: Lock<[Vec<(u8, [u64; 4])>; MAX_TASKS]> = Lock::new([const { Vec::new() }; MAX_TASKS]);

/// `exec` (and a fresh task, `on_spawn`): caught handlers go back to the
/// default, ignored signals stay ignored, as on Linux; no alternate stack.
pub fn on_exec(slot: usize) {
    ALT.lock()[slot] = AltStack::NONE;
    // exec unmapped the trampoline page with the rest of the image.
    TRAMP[slot].store(0, Ordering::Relaxed);
    RESTORER[slot].store(0, Ordering::Relaxed);
    let ign = crate::k::signal::HANDLER_IGN as u64;
    EXTRA.lock()[slot].retain_mut(|(_, w)| {
        *w = [w[0], 0, 0, 0];
        w[0] == ign
    });
}

/// A new thread `slot` of a process: no alternate stack yet.
pub fn on_thread(slot: usize) {
    ALT.lock()[slot] = AltStack::NONE;
}

pub fn on_fork(parent: usize, child: usize) {
    {
        let mut alt = ALT.lock();
        alt[child] = alt[parent];
    }
    TRAMP[child].store(TRAMP[parent].load(Ordering::Relaxed), Ordering::Relaxed);
    RESTORER[child].store(RESTORER[parent].load(Ordering::Relaxed), Ordering::Relaxed);
    let mut extra = EXTRA.lock();
    extra[child] = extra[parent].clone();
}

/// `rt_sigaction` of a Linux signal the core has no number for: the words
/// stored in [`EXTRA`], swapped for the new ones. Such a signal is never
/// delivered, so a handler needs no trampoline.
fn extra_sigaction(id: usize, sig: usize, new: Option<[u64; 4]>, oact: usize, words: usize) -> R {
    let old = {
        let mut extra = EXTRA.lock();
        let list = &mut extra[id];
        let at = list.iter().position(|&(n, _)| usize::from(n) == sig);
        let old = at.map_or([0; 4], |i| list[i].1);
        if let Some(w) = new {
            match (at, w == [0; 4]) {
                (Some(i), true) => {
                    list.swap_remove(i);
                }
                (Some(i), false) => list[i].1 = w,
                (None, false) => list.push((sig as u8, w)),
                (None, true) => {}
            }
        }
        old
    };
    if oact != 0 {
        // The words in the arch's order: the restorer only where it has one.
        let mut b = [0u8; 32];
        let order: &[usize] = if words == 4 { &[0, 1, 2, 3] } else { &[0, 1, 3] };
        for (i, &k) in order.iter().enumerate() {
            b[i * 8..i * 8 + 8].copy_from_slice(&old[k].to_le_bytes());
        }
        put(oact, &b[..words * 8])?;
    }
    Ok(0)
}

/// The interrupted context and handler for one delivery, in Linux terms.
pub struct Frame {
    /// Linux signal number.
    pub sig: usize,
    pub handler: usize,
    /// Where the handler returns to (it calls `rt_sigreturn`).
    pub restorer: usize,
    /// Linux blocked set to restore at `rt_sigreturn`.
    pub mask: u64,
    /// Interrupted PC and the syscall result it will see (for a fault: the
    /// faulting instruction and the result register's value).
    pub pc: usize,
    pub ret: usize,
    /// A fault's `si_code` and `si_addr` (0, 0: a signal sent).
    pub code: u32,
    pub addr: usize,
    /// A signal sent: its sender's pid (`si_pid`; 0: the kernel).
    pub pid: u32,
    /// Where the frame goes below: the interrupted stack pointer, or the top
    /// of the alternate stack for an `SA_ONSTACK` handler.
    pub sp: usize,
    /// The ucontext's `uc_stack`: the alternate stack as `sigaltstack`
    /// reports it to the interrupted code.
    pub stack: [u8; 24],
    /// Arch word from the kernel (x86_64: the user CS; unused elsewhere).
    #[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
    pub arch: u64,
}

/// `siginfo_t` for a signal sent with `kill` (`code` 0, `SI_USER`; its
/// sender's `si_pid` at offset 16, `si_uid` 0 as every uid of the layer) or
/// a fault's (its `si_code`, and `si_addr` at offset 16).
pub fn siginfo(sig: usize, code: u32, addr: usize, pid: u32) -> [u8; 128] {
    let mut b = [0u8; 128];
    b[..4].copy_from_slice(&(sig as i32).to_le_bytes());
    b[8..12].copy_from_slice(&code.to_le_bytes());
    if code == 0 {
        b[16..20].copy_from_slice(&pid.to_le_bytes());
    } else {
        b[16..24].copy_from_slice(&(addr as u64).to_le_bytes());
    }
    b
}

/// Run the caught handler of a native signal in the current Linux task:
/// called by the kernel's delivery path with the decided return context
/// (`pc`, `ret`) and the native mask the handler's return restores. Returns
/// the value for the result register, or `None` if the frame does not fit
/// (the kernel then kills the task with `SIGSEGV`, as Linux does).
pub fn deliver(regs: &mut SyscallRegs, d: &myos_abi::SignalDelivery) -> Option<usize> {
    let sp = arch::user_sp(regs);
    let alt = ALT.lock()[task::current_id()];
    // An SA_ONSTACK handler runs on the alternate stack, unless the signal
    // came while on it already (a nested one goes below the first frame).
    let on_alt = d.flags & SA_ONSTACK != 0 && alt.size != 0 && !alt.holds(sp);
    let f = Frame {
        sp: if on_alt { (alt.sp + alt.size) as usize } else { sp },
        stack: alt.stack_t(sp),
        sig: sig_to_linux(d.sig),
        handler: d.handler,
        restorer: d.tramp,
        mask: mask_to_linux(d.restore_mask),
        pc: d.pc,
        ret: d.ret,
        code: d.code,
        addr: d.addr,
        pid: d.pid,
        arch: d.arch,
    };
    arch::deliver(regs, &f)
}

/// `rt_sigreturn`: the handler returned through the restorer. Restores the
/// interrupted registers and mask; returns its result-register value.
pub fn rt_sigreturn(regs: &mut SyscallRegs) -> usize {
    match arch::sigreturn(regs) {
        Some((ret, mask)) => {
            task::signal_set_blocked_mask(task::current_id(), mask_from_linux(mask));
            ret
        }
        // A corrupt frame: what Linux does too.
        None => crate::k::signal::terminate(crate::k::signal::SIGSEGV),
    }
}

/// The trampoline page for handlers without `SA_RESTORER`.
fn trampoline() -> Result<usize, usize> {
    let slot = task::current_pid();
    let have = TRAMP[slot].load(Ordering::Relaxed) as usize;
    if have != 0 {
        return Ok(have);
    }
    const PROT_READ: usize = 1;
    const PROT_WRITE: usize = 2;
    const PROT_EXEC: usize = 4;
    const MAP_PRIVATE: usize = 2;
    const MAP_ANON: usize = 0x20;
    let page = crate::k::user::PAGE;
    let va = crate::k::user::do_mmap(0, page, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
    if va >= crate::k::signal::SYSERR_LOWEST {
        return Err(ENOMEM);
    }
    put(va, arch::TRAMP_CODE)?;
    if crate::k::user::sys_mprotect(va, page, PROT_READ | PROT_EXEC) != 0 {
        return Err(ENOMEM);
    }
    TRAMP[slot].store(va as u64, Ordering::Relaxed);
    Ok(va)
}

/// Linux `sa_flags` to the native ones: only `SA_NOCLDWAIT` moves.
fn flags_from_linux(flags: u32) -> u32 {
    let native = crate::k::signal::SA_NOCLDWAIT;
    let nocldwait = if flags & LINUX_SA_NOCLDWAIT != 0 { native } else { 0 };
    (flags & !(LINUX_SA_NOCLDWAIT | native)) | nocldwait
}

fn flags_to_linux(flags: u32) -> u32 {
    let native = crate::k::signal::SA_NOCLDWAIT;
    let nocldwait = if flags & native != 0 { LINUX_SA_NOCLDWAIT } else { 0 };
    (flags & !native) | nocldwait
}

/// `rt_sigaction(sig, act, oact, sigsetsize)` with the arch's
/// `struct sigaction` (`{handler, flags, [restorer,] mask}`).
pub fn rt_sigaction(sig: usize, act: usize, oact: usize) -> R {
    if sig == 0 || sig > LINUX_SIGMAX {
        return Err(EINVAL);
    }
    let n = sig_from_linux(sig);
    let id = task::current_pid();
    let words = if arch::SIGACTION_HAS_RESTORER { 4 } else { 3 };
    let mask_word = words - 1;
    // Read the new action before writing the old one (they may alias).
    let new = if act != 0 {
        let mut w = [0u64; 4];
        for (i, v) in w.iter_mut().enumerate().take(words) {
            *v = get_u64(act + i * 8)?;
        }
        Some(w)
    } else {
        None
    };
    if n == 0 {
        // Stored as handler, flags, restorer, mask whatever the arch's order.
        let new = new.map(|w| {
            let restorer = if words == 4 { w[2] } else { 0 };
            [w[0], w[1], restorer, w[mask_word]]
        });
        return extra_sigaction(id, sig, new, oact, words);
    }
    if oact != 0 {
        let (handler, flags, mask) = task::signal_get_action(id, n);
        let caught = handler > crate::k::signal::HANDLER_IGN;
        let mut w = [0u64; 4];
        w[0] = handler as u64;
        if caught {
            w[1] = flags_to_linux(flags) as u64;
            if arch::SIGACTION_HAS_RESTORER {
                w[2] = RESTORER[id].load(Ordering::Relaxed);
            }
            w[mask_word] = mask_to_linux(mask);
        }
        let mut b = [0u8; 32];
        for (i, v) in w.iter().enumerate() {
            b[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
        }
        put(oact, &b[..words * 8])?;
    }
    if let Some(w) = new {
        let (handler, flags) = (w[0] as usize, w[1] as usize);
        let mask = mask_from_linux(w[mask_word]);
        let mut tramp = 0;
        if handler > crate::k::signal::HANDLER_IGN {
            let restorer = if arch::SIGACTION_HAS_RESTORER && flags & SA_RESTORER != 0 {
                w[2] as usize
            } else {
                0
            };
            RESTORER[id].store(restorer as u64, Ordering::Relaxed);
            tramp = if restorer != 0 { restorer } else { trampoline()? };
        }
        if !task::signal_set_action(id, n, handler, flags_from_linux(flags as u32), mask, tramp) {
            return Err(EINVAL);
        }
    }
    Ok(0)
}

pub fn rt_sigprocmask(how: usize, set: usize, oset: usize) -> R {
    let id = task::current_id();
    let cur = task::signal_blocked(id);
    let new = if set != 0 {
        let s = mask_from_linux(get_u64(set)?);
        Some(match how {
            0 => cur | s,
            1 => cur & !s,
            2 => s,
            _ => return Err(EINVAL),
        })
    } else {
        None
    };
    if oset != 0 {
        put(oset, &mask_to_linux(cur).to_le_bytes())?;
    }
    if let Some(m) = new {
        task::signal_set_blocked_mask(id, m);
    }
    Ok(0)
}

pub fn rt_sigpending(set: usize) -> R {
    let p = task::signal_pending(task::current_id());
    put(set, &mask_to_linux(p).to_le_bytes())?;
    Ok(0)
}

/// Always `EINTR` once a handler has run (see `crate::k::signal::sigsuspend`).
pub fn rt_sigsuspend(set: usize) -> usize {
    match get_u64(set) {
        Ok(m) => crate::k::signal::sigsuspend(mask_from_linux(m)),
        Err(e) => err(e),
    }
}

/// `rt_sigtimedwait(set, info, timeout)`: take a signal of `set` (normally
/// blocked) instead of delivering it.
pub fn rt_sigtimedwait(set: usize, info: usize, timeout: usize) -> R {
    let id = task::current_id();
    let want = mask_from_linux(get_u64(set)?);
    let deadline = if timeout != 0 {
        Some(super::sys::timespec_deadline(timeout, None)?)
    } else {
        None
    };
    loop {
        if let Some((n, pid)) = task::signal_take_from(id, want) {
            let sig = sig_to_linux(n);
            if info != 0 {
                put(info, &siginfo(sig, 0, 0, pid))?;
            }
            return Ok(sig);
        }
        if deadline.is_some_and(|d| crate::k::time::monotonic_ns() >= d) {
            return Err(EAGAIN);
        }
        if crate::k::signal::interrupt_wait() {
            return Err(EINTR);
        }
        task::yield_now();
    }
}

/// `sigaltstack(ss, oss)` of the calling thread, whose stack pointer is
/// `sp`: the stack it had into `oss`, then `ss` its new one (or none, with
/// `SS_DISABLE`). Go's runtime gives every thread one and checks, in each
/// handler, that it is still there.
pub fn sigaltstack(ss: usize, oss: usize, sp: usize) -> R {
    let id = task::current_id();
    let old = ALT.lock()[id];
    let new = if ss != 0 {
        let base = get_u64(ss)?;
        let flags = get_u64(ss + 8)? as u32;
        let size = get_u64(ss + 16)?;
        // Not while a handler runs on it.
        if old.holds(sp) {
            return Err(EPERM);
        }
        Some(match flags & !SS_AUTODISARM {
            SS_DISABLE => AltStack::NONE,
            0 | SS_ONSTACK if size < MINSIGSTKSZ => return Err(ENOMEM),
            0 | SS_ONSTACK => AltStack { sp: base, size },
            _ => return Err(EINVAL),
        })
    } else {
        None
    };
    if oss != 0 {
        put(oss, &old.stack_t(sp))?;
    }
    if let Some(new) = new {
        ALT.lock()[id] = new;
    }
    Ok(0)
}
