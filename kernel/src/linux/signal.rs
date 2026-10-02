//! Linux signal syscalls and handler delivery.
//!
//! Dispositions, masks and pending bits are the native ones
//! (`task::signal_*`, in native numbering); this module translates numbers
//! and structures at the boundary. A caught signal runs through the native
//! decision path (`crate::signal::on_syscall_exit`), which calls [`deliver`]
//! for a Linux task: the arch module writes a Linux `rt_sigframe` (siginfo,
//! ucontext with the interrupted registers, FP state) and `rt_sigreturn`
//! undoes it.

use core::sync::atomic::{AtomicU64, Ordering};

use super::abi::*;
use super::arch;
use super::sys::{get_u64, put, R};
use super::MAX_TASKS;
use crate::task::{self, SigAction};
use crate::user::SyscallRegs;

const SA_RESTORER: usize = 0x0400_0000;
const SS_DISABLE: u32 = 2;

/// Per process (leader slot): the user page holding [`arch::TRAMP_CODE`], mapped on
/// first use for handlers registered without `SA_RESTORER` (riscv64 musl
/// never passes one; real Linux uses its vDSO there). 0 = none yet.
static TRAMP: [AtomicU64; MAX_TASKS] = [const { AtomicU64::new(0) }; MAX_TASKS];
/// Per process: the `sa_restorer` last registered (reported in `oact`).
static RESTORER: [AtomicU64; MAX_TASKS] = [const { AtomicU64::new(0) }; MAX_TASKS];

pub fn on_exec(slot: usize) {
    // exec unmapped the trampoline page with the rest of the image.
    TRAMP[slot].store(0, Ordering::Relaxed);
    RESTORER[slot].store(0, Ordering::Relaxed);
}

pub fn on_fork(parent: usize, child: usize) {
    TRAMP[child].store(TRAMP[parent].load(Ordering::Relaxed), Ordering::Relaxed);
    RESTORER[child].store(RESTORER[parent].load(Ordering::Relaxed), Ordering::Relaxed);
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
    /// Interrupted PC and the syscall result it will see.
    pub pc: usize,
    pub ret: usize,
}

/// `siginfo_t` for a signal sent with `kill` (`SI_USER`).
pub fn siginfo(sig: usize) -> [u8; 128] {
    let mut b = [0u8; 128];
    b[..4].copy_from_slice(&(sig as i32).to_le_bytes());
    b
}

/// Run `act` for native signal `sig` in the current Linux task: called by
/// the native delivery path with the decided return context (`pc`, `ret`)
/// and the native mask the handler's return restores. Returns the value for
/// the result register, or `None` if the frame does not fit (the caller
/// then kills the task with `SIGSEGV`, as Linux does).
pub fn deliver(
    regs: &mut SyscallRegs,
    sig: u32,
    act: &SigAction,
    tramp: usize,
    restore_mask: u32,
    pc: usize,
    ret: usize,
) -> Option<usize> {
    let f = Frame {
        sig: sig_to_linux(sig),
        handler: act.handler,
        restorer: tramp,
        mask: mask_to_linux(restore_mask),
        pc,
        ret,
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
        None => crate::signal::terminate(crate::signal::SIGSEGV),
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
    let page = crate::user::PAGE;
    let va = crate::user::do_mmap(0, page, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
    if va >= crate::signal::SYSERR_EINTR {
        return Err(ENOMEM);
    }
    put(va, arch::TRAMP_CODE)?;
    if crate::user::sys_mprotect(va, page, PROT_READ | PROT_EXEC) != 0 {
        return Err(ENOMEM);
    }
    TRAMP[slot].store(va as u64, Ordering::Relaxed);
    Ok(va)
}

/// `rt_sigaction(sig, act, oact, sigsetsize)` with the arch's
/// `struct sigaction` (`{handler, flags, [restorer,] mask}`).
pub fn rt_sigaction(sig: usize, act: usize, oact: usize) -> R {
    let n = sig_from_linux(sig);
    if n == 0 {
        return Err(EINVAL);
    }
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
    if oact != 0 {
        let (handler, flags, mask) = task::signal_get_action(id, n);
        let caught = handler > crate::signal::HANDLER_IGN;
        let mut w = [0u64; 4];
        w[0] = handler as u64;
        if caught {
            w[1] = flags as u64;
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
        if handler > crate::signal::HANDLER_IGN {
            let restorer = if arch::SIGACTION_HAS_RESTORER && flags & SA_RESTORER != 0 {
                w[2] as usize
            } else {
                0
            };
            RESTORER[id].store(restorer as u64, Ordering::Relaxed);
            tramp = if restorer != 0 { restorer } else { trampoline()? };
        }
        if !task::signal_set_action(id, n, handler, flags as u32, mask, tramp) {
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

/// Always `EINTR` once a handler has run (see `crate::signal::sigsuspend`).
pub fn rt_sigsuspend(set: usize) -> usize {
    match get_u64(set) {
        Ok(m) => crate::signal::sigsuspend(mask_from_linux(m)),
        Err(e) => err(e),
    }
}

/// `rt_sigtimedwait(set, info, timeout)`: take a signal of `set` (normally
/// blocked) instead of delivering it.
pub fn rt_sigtimedwait(set: usize, info: usize, timeout: usize) -> R {
    let id = task::current_id();
    let want = mask_from_linux(get_u64(set)?);
    let deadline = if timeout != 0 {
        let us = get_u64(timeout)?.saturating_mul(1_000_000) + get_u64(timeout + 8)? / 1000;
        Some(super::sys::now_us().saturating_add(us))
    } else {
        None
    };
    loop {
        if let Some(n) = task::signal_take_from(id, want) {
            let sig = sig_to_linux(n);
            if info != 0 {
                put(info, &siginfo(sig))?;
            }
            return Ok(sig);
        }
        if deadline.is_some_and(|d| super::sys::now_us() >= d) {
            return Err(EAGAIN);
        }
        if crate::signal::interrupt_wait() {
            return Err(EINTR);
        }
        task::yield_now();
    }
}

/// No alternate signal stacks: report none and accept (ignore) a new one.
pub fn sigaltstack(oss: usize) -> R {
    if oss != 0 {
        let mut b = [0u8; 24];
        b[8..12].copy_from_slice(&SS_DISABLE.to_le_bytes());
        put(oss, &b)?;
    }
    Ok(0)
}
