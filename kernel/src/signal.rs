//! Process signals.
//!
//! Per-task state (pending / ignored / blocked bits, caught handlers) lives in
//! [`crate::task`]; this module holds the policy:
//!
//! - **Numbers** are newlib's (BSD-style: `SIGCHLD = 20`, `SIGUSR1 = 30`),
//!   which libgloss, the ports and rustix_compat are all built against.
//! - **Default actions**: terminate, except the signals POSIX says to ignore
//!   (`SIGCHLD`, `SIGURG`, `SIGWINCH`, `SIGCONT`). There is no job control,
//!   so the stop signals are ignored too.
//! - **Delivery** happens on the way out of a syscall ([`on_syscall_exit`]).
//!   For a caught signal the kernel writes a [`frame`](FRAME_WORDS) below the
//!   user stack pointer and resumes at the libc trampoline registered with
//!   `sigaction`. The trampoline saves every other register (integer and
//!   FP), calls the handler, restores them and calls `SYS_SIGRETURN`, which
//!   puts back the signal mask, PC, SP and the interrupted syscall's result.
//! - **Blocking syscalls** stop waiting when a signal would act
//!   ([`interrupt_wait`]) and return `EINTR`, or restart for `SA_RESTART`.
//!
//! A task looping in user mode without making syscalls is terminated by a
//! signal whose action is to terminate when an interrupt preempts it
//! ([`on_user_preempted`]); a caught signal waits for its next syscall.

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::task::{self, SigNext};
use crate::user::SyscallRegs;

pub const SIGHUP: u32 = 1;
pub const SIGINT: u32 = 2;
pub const SIGKILL: u32 = 9;
pub const SIGSEGV: u32 = 11;
pub const SIGALRM: u32 = 14;
pub const SIGTERM: u32 = 15;
pub const SIGURG: u32 = 16;
pub const SIGSTOP: u32 = 17;
pub const SIGTSTP: u32 = 18;
pub const SIGCONT: u32 = 19;
pub const SIGCHLD: u32 = 20;
pub const SIGTTIN: u32 = 21;
pub const SIGTTOU: u32 = 22;
pub const SIGWINCH: u32 = 28;

/// `SIG_DFL`.
pub const HANDLER_DFL: usize = 0;
/// `SIG_IGN` (refused for `SIGKILL` / `SIGSTOP`).
pub const HANDLER_IGN: usize = 1;

/// `sa_flags` bits (myos `<sys/signal.h>`; Linux values except `SA_SIGINFO`,
/// which is newlib's). Handlers always get `(signo, &siginfo, NULL)`, so
/// `SA_SIGINFO` needs no special casing.
pub const SA_RESTART: u32 = 0x1000_0000;
pub const SA_NODEFER: u32 = 0x4000_0000;
pub const SA_RESETHAND: u32 = 0x8000_0000;
/// On `SIGCHLD`: children leave no zombie (the kernel reaps them, as when
/// `SIGCHLD` is ignored). Not Linux's 2, which is newlib's `SA_SIGINFO`.
pub const SA_NOCLDWAIT: u32 = myos_abi::MYOS_SA_NOCLDWAIT;

/// Returned by a syscall interrupted by a caught signal (libgloss: `EINTR`).
pub const SYSERR_EINTR: usize = usize::MAX - 3;

/// Default action of `sig`: true = terminate, false = ignore.
pub fn default_terminates(sig: u32) -> bool {
    !matches!(
        sig,
        SIGURG | SIGSTOP | SIGTSTP | SIGCONT | SIGCHLD | SIGTTIN | SIGTTOU | SIGWINCH
    )
}

/// Task id currently blocked in [`crate::input::read`], or `usize::MAX` if none.
///
/// Phase-1 foreground for `^C`: prefer this task's process group; if nobody is
/// in a console read, fall back to the last console reader's process group
/// (so `^C` still reaches a foreground child while the shell is blocked in
/// `wait`), then to every live user task with `has_ctty`.
static INPUT_READER: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Process group of the last task that blocked in `input::read`, or `usize::MAX`
/// if none so far. Kept across [`leave_input_read`] so a `^C` typed while a
/// foreground child runs (nobody is reading the console) still interrupts
/// the shell's process group — the shell ignores SIGINT (interactive oksh) and
/// the child dies, exactly like a real tty foreground job.
static LAST_INPUT_PGID: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Mark the current task as blocked in console `input::read` (for `^C` fg).
pub fn enter_input_read() {
    let id = task::current_pid();
    INPUT_READER.store(id, Ordering::SeqCst);
    if let Some(pgid) = task::task_pgid(id) {
        LAST_INPUT_PGID.store(pgid, Ordering::SeqCst);
    }
}

/// Clear the console-read marker. [`LAST_INPUT_PGID`] persists so `^C` keeps
/// working while a foreground child runs (see its doc comment).
pub fn leave_input_read() {
    INPUT_READER.store(usize::MAX, Ordering::SeqCst);
}

/// An interrupt that preempted user mode is about to return there: a
/// pending signal whose action is to terminate (`SIGKILL`, `^C`'s `SIGINT`
/// left at its default, ...) ends the task now. A thread spinning in user
/// mode would never reach a syscall exit, where signals otherwise act; a
/// caught signal still waits for it (its handler needs the syscall's frame).
pub fn on_user_preempted() {
    if let Some(sig) = task::signal_terminating(task::current_id()) {
        task::user_exit_signal(sig);
    }
}

/// `SIGCHLD` to `parent` (a child exited).
pub fn raise_sigchld(parent: usize) {
    task::signal_send(parent, SIGCHLD);
}

/// True if the current task has a pending signal that would terminate it or
/// run a handler (so a blocking wait should end).
pub fn current_should_wake() -> bool {
    task::signal_wakeable(task::current_id())
}

/// For a blocking syscall's wait loop: true if a signal should end the wait,
/// in which case the syscall is marked interrupted and its (no-progress)
/// result becomes `EINTR` or a restart in [`on_syscall_exit`].
pub fn interrupt_wait() -> bool {
    let id = task::current_id();
    if task::signal_wakeable(id) {
        task::signal_mark_interrupted(id);
        true
    } else {
        false
    }
}

/// Send `sig` according to POSIX-ish pid rules:
/// - `pid > 0`: that task id
/// - `pid == 0`: current process group
/// - `pid < 0`: process group `-pid`
///
/// Returns `false` if `sig` is invalid or no matching live user task was found.
/// Signal 0 sends nothing: it only asks whether a target exists.
pub fn kill(pid: isize, sig: u32) -> bool {
    if sig > 31 {
        return false;
    }
    if sig == 0 {
        return match pid {
            1.. => task::is_live_user(pid as usize) && may_signal(pid as usize),
            0 => true,
            _ => (0..task::task_slots()).any(|id| task::task_pgid(id) == Some((-pid) as usize) && may_signal(id)),
        };
    }
    if pid > 0 {
        return may_signal(pid as usize) && send_one(pid as usize, sig);
    }
    let pgid = if pid == 0 {
        let Some(pgid) = task::current_pgid() else {
            return false;
        };
        pgid
    } else {
        // pid < 0 → process group -pid
        (-pid) as usize
    };
    // The members the caller may signal (docs/security.md: `proc(user)`).
    let mut any = false;
    for id in 0..task::task_slots() {
        if task::task_pgid(id) == Some(pgid) && may_signal(id) && send_one(id, sig) {
            any = true;
        }
    }
    any
}

/// The caller may signal task `id` (a process, or a thread of one: the
/// process's rules apply): itself, or one whose user's `proc(user)` label
/// its domain has `signal` on. A task that is no process's is nobody's to
/// signal.
fn may_signal(id: usize) -> bool {
    let Some(pid) = task::task_tgid(id) else {
        return false;
    };
    pid == task::current_pid() || task::sec_ctx_of(pid).is_some_and(crate::sec::may_signal)
}

/// Deliver `sig` to every live user task in process group `pgid`.
pub fn kill_pg(pgid: usize, sig: u32) -> bool {
    if sig == 0 || sig > 31 {
        return false;
    }
    let mut any = false;
    for id in 0..task::task_slots() {
        if task::task_pgid(id) == Some(pgid) && send_one(id, sig) {
            any = true;
        }
    }
    any
}

fn send_one(id: usize, sig: u32) -> bool {
    if !task::is_live_user(id) {
        return false;
    }
    task::signal_send(id, sig);
    true
}

/// `^C` (byte `0x03`) from the console: `SIGINT` to the phase-1 foreground group.
///
/// Foreground choice (documented): pgid of the task blocked in `input::read`
/// when one exists; otherwise every live user task with a controlling tty.
pub fn handle_ctrl_c() {
    let reader = INPUT_READER.load(Ordering::SeqCst);
    if reader != usize::MAX {
        if let Some(pgid) = task::task_pgid(reader) {
            let _ = kill_pg(pgid, SIGINT);
            return;
        }
    }
    let last = LAST_INPUT_PGID.load(Ordering::SeqCst);
    if last != usize::MAX {
        // Foreground child phase: nobody is blocked on the console (the shell
        // sits in `wait`, the child in a socket/pipe poll). Interrupt the
        // shell's group — the shell ignores SIGINT, the child dies.
        let _ = kill_pg(last, SIGINT);
        return;
    }
    for id in 0..task::task_slots() {
        if task::task_has_ctty(id) {
            let _ = send_one(id, SIGINT);
        }
    }
}

// --- Delivery -------------------------------------------------------------

/// Signal frame the kernel writes below the user stack pointer, as `u64`
/// words. The libc trampoline starts with `sp` pointing at word 0 and must
/// call `SYS_SIGRETURN` with `sp` back at word 0.
const FRAME_WORDS: usize = 16;
const F_MAGIC: usize = 0;
/// Signal number (first handler argument).
const F_SIGNO: usize = 1;
/// Handler address.
const F_HANDLER: usize = 2;
/// Blocked mask to restore.
const F_MASK: usize = 3;
/// Interrupted context: where the syscall returns to, its stack pointer,
/// its result, and the syscall-number register (`x8` / `a7`).
const F_PC: usize = 4;
const F_SP: usize = 5;
const F_RET: usize = 6;
const F_NR_REG: usize = 7;
/// `siginfo_t` (newlib: `int si_signo; int si_code; union sigval si_value`),
/// passed as the handler's second argument; words 10..13.
const F_SIGINFO: usize = 10;
const FRAME_MAGIC: u64 = u64::from_le_bytes(*b"MYOSSIG1");
/// Skipped below the interrupted stack pointer: the x86-64 SysV red zone,
/// where a leaf syscall wrapper may keep locals. Harmless elsewhere.
const RED_ZONE: usize = 128;
/// newlib `si_code` for a signal sent by `kill`/`raise`.
const SI_USER: u64 = 1;

/// Act on the current task's pending signals as its syscall `nr` (first
/// argument `a0`) returns `ret` to user mode. Returns the value for the
/// result register; `regs` may be redirected to a handler. Does not return
/// if a signal terminates the task.
pub fn on_syscall_exit(regs: &mut SyscallRegs, nr: usize, a0: usize, ret: usize) -> usize {
    let id = task::current_id();
    let interrupted = task::signal_take_interrupted(id);
    // Re-executing the syscall: back to its instruction, with the register
    // that carried the number / first argument restored.
    let restart_pc = regs.pc().wrapping_sub(SyscallRegs::INSN_LEN);
    let restart_ret = SyscallRegs::restart_value(nr, a0);
    let suspend_mask = task::signal_take_suspend_mask(id);
    match task::signal_next(id) {
        SigNext::None => {
            if let Some(m) = suspend_mask {
                task::signal_set_blocked_mask(id, m);
            }
            if interrupted {
                // Whatever woke the wait no longer acts (e.g. it got ignored):
                // restart transparently, as if never interrupted.
                regs.set_pc(restart_pc);
                return restart_ret;
            }
            ret
        }
        SigNext::Terminate(sig) => terminate(sig),
        SigNext::Deliver {
            sig,
            act,
            tramp,
            old_blocked,
        } => {
            let (pc, ret) = if !interrupted {
                (regs.pc(), ret)
            } else if act.flags & SA_RESTART != 0 {
                (restart_pc, restart_ret)
            } else {
                (regs.pc(), SYSERR_EINTR)
            };
            // After sigsuspend, the handler returns to the caller's mask.
            let restore_mask = suspend_mask.unwrap_or(old_blocked);
            // A foreign-personality task gets its own frame (Linux `rt_sigframe`).
            match crate::personality::deliver(regs, sig, &act, tramp, restore_mask, pc, ret) {
                crate::personality::Deliver::Native => {}
                crate::personality::Deliver::Done(v) => return v,
                crate::personality::Deliver::Failed => terminate(SIGSEGV),
            }
            let sp = regs.sp();
            let frame_va = (sp.wrapping_sub(RED_ZONE + FRAME_WORDS * 8)) & !15;
            let mut frame = [0u64; FRAME_WORDS];
            frame[F_MAGIC] = FRAME_MAGIC;
            frame[F_SIGNO] = sig as u64;
            frame[F_HANDLER] = act.handler as u64;
            frame[F_MASK] = restore_mask as u64;
            frame[F_PC] = pc as u64;
            frame[F_SP] = sp as u64;
            frame[F_RET] = ret as u64;
            frame[F_NR_REG] = regs.nr_reg() as u64;
            frame[F_SIGINFO] = sig as u64 | (SI_USER << 32);
            if !write_frame(frame_va, &frame) {
                // No room on the user stack: what Linux does too.
                terminate(SIGSEGV);
            }
            // The trampoline is returned to directly (x86 `sysretq` loads it
            // into RIP): a non-canonical address there would #GP in ring 0 on
            // real hardware. It must be a mapped user address.
            if !crate::user::buffer_ok(tramp, 1) {
                terminate(SIGSEGV);
            }
            regs.set_pc(tramp);
            regs.set_sp(frame_va);
            ret
        }
    }
}

fn write_frame(va: usize, frame: &[u64; FRAME_WORDS]) -> bool {
    let mut bytes = [0u8; FRAME_WORDS * 8];
    for (i, w) in frame.iter().enumerate() {
        bytes[i * 8..i * 8 + 8].copy_from_slice(&w.to_le_bytes());
    }
    crate::user::buffer_ok(va, bytes.len())
        && crate::user::copy_to_user(task::current_aspace(), va, &bytes)
}

pub(crate) fn terminate(sig: u32) -> ! {
    // `user_exit_signal` never returns to the trap epilogue that clears
    // `SYSCALL_FRAME`; drop it here so a later fork/exec cannot read a
    // dangling frame on the dying task's kernel stack.
    crate::user::set_syscall_frame(core::ptr::null_mut());
    task::user_exit_signal(sig);
}

/// `SYS_SIGRETURN`: the trampoline is done with the frame at the user stack
/// pointer. Restore the mask and the interrupted context; returns the
/// interrupted syscall's result.
pub fn sigreturn(regs: &mut SyscallRegs) -> usize {
    let va = regs.sp();
    let mut bytes = [0u8; FRAME_WORDS * 8];
    if !crate::user::buffer_ok(va, bytes.len())
        || !crate::user::copy_from_user(task::current_aspace(), va, &mut bytes)
    {
        terminate(SIGSEGV);
    }
    let word = |i: usize| u64::from_le_bytes(bytes[i * 8..i * 8 + 8].try_into().unwrap());
    if word(F_MAGIC) != FRAME_MAGIC {
        terminate(SIGSEGV);
    }
    task::signal_set_blocked_mask(task::current_id(), word(F_MASK) as u32);
    // The restored PC is returned to directly (x86 `sysretq` loads it into
    // RIP): a forged non-canonical value would #GP in ring 0 on real
    // hardware. Require a mapped user address; kill the task otherwise.
    let pc = word(F_PC) as usize;
    if !crate::user::buffer_ok(pc, 1) {
        terminate(SIGSEGV);
    }
    regs.set_pc(pc);
    regs.set_sp(word(F_SP) as usize);
    regs.set_nr_reg(word(F_NR_REG) as usize);
    word(F_RET) as usize
}

// --- sigpending / sigsuspend / sigwait -------------------------------------------

/// `sigpending`: the pending set (signals that arrived while blocked).
pub fn sigpending() -> usize {
    task::signal_pending(task::current_id()) as usize
}

/// `sigsuspend(mask)`: wait with `mask` as the blocked set until a signal
/// terminates the task or runs a handler; the handler runs on the way out
/// and then the previous mask is back. Always "fails" with `EINTR`.
pub fn sigsuspend(mask: u32) -> usize {
    let id = task::current_id();
    task::signal_save_suspend_mask(id, task::signal_blocked(id));
    task::signal_set_blocked_mask(id, mask);
    loop {
        let seq = task::wait_seq();
        if task::signal_wakeable(id) {
            break;
        }
        // Woken by `signal_send` (`wake_task`), not by any key.
        task::block_until(task::KEY_SIGNAL, seq, 0);
    }
    SYSERR_EINTR
}

/// `sigwait(set)`: wait for a signal in `set` (normally blocked) and consume
/// it instead of delivering it. Returns its number, or `EINTR` if another
/// signal acts first (libgloss retries).
pub fn sigwait(set: u32) -> usize {
    let id = task::current_id();
    loop {
        let seq = task::wait_seq();
        if let Some(sig) = task::signal_take_from(id, set) {
            return sig as usize;
        }
        if task::signal_wakeable(id) {
            return SYSERR_EINTR;
        }
        task::block_until(task::KEY_SIGNAL, seq, 0);
    }
}

// --- sigaction / sigprocmask ------------------------------------------------

/// `sigprocmask(how, set, oset)` — per-task blocked-signal mask.
/// `SIG_BLOCK=1` ors, `SIG_SETMASK=0` replaces, `SIG_UNBLOCK=2` clears.
/// `SIGKILL`/`SIGSTOP` can never be blocked. libgloss packs `sigset_t` as one
/// `u32`. A pending signal unblocked here is delivered before the call
/// returns (at syscall exit), as POSIX requires.
pub fn sigprocmask(how: usize, set: Option<usize>, oset: Option<usize>) -> bool {
    let id = task::current_id();
    if !task::is_live_user(id) {
        return false;
    }
    let cur = task::signal_blocked(id);
    if let Some(out) = oset {
        if !crate::user::buffer_ok(out, 4) || !crate::user::copy_to_user(task::current_aspace(), out, &cur.to_le_bytes()) {
            return false;
        }
    }
    if let Some(inp) = set {
        let mut buf = [0u8; 4];
        if !crate::user::buffer_ok(inp, 4) || !crate::user::copy_from_user(task::current_aspace(), inp, &mut buf) {
            return false;
        }
        let mask = u32::from_le_bytes(buf);
        let new = match how {
            0 => mask,        // SIG_SETMASK
            1 => cur | mask,  // SIG_BLOCK
            2 => cur & !mask, // SIG_UNBLOCK
            _ => return false,
        };
        task::signal_set_blocked_mask(id, new);
    }
    true
}

/// `sigaction(sig, act, oact)` on libgloss's packed struct of `usize` words
/// `{handler, flags, mask}` and, with `with_tramp` (`SYS_SIGACTION2`), a 4th
/// word: the trampoline (libgloss's `__myos_sigtramp`) that runs a function
/// handler. Without it (`SYS_SIGACTION`, binaries from before kernel-delivered
/// handlers) only `SIG_DFL` (0) / `SIG_IGN` (1) are accepted. `oact` gets the
/// first three words.
pub fn sigaction(sig: u32, act: Option<usize>, oact: Option<usize>, with_tramp: bool) -> bool {
    if sig == 0 || sig > 31 {
        return false;
    }
    let id = task::current_id();
    if !task::is_live_user(id) {
        return false;
    }
    if let Some(out) = oact {
        let (handler, flags, mask) = task::signal_get_action(id, sig);
        let words = [handler, flags as usize, mask as usize];
        let mut bytes = [0u8; 3 * 8];
        for (i, w) in words.iter().enumerate() {
            bytes[i * 8..i * 8 + 8].copy_from_slice(&(*w as u64).to_le_bytes());
        }
        if !crate::user::buffer_ok(out, bytes.len()) || !crate::user::copy_to_user(task::current_aspace(), out, &bytes) {
            return false;
        }
    }
    if let Some(inp) = act {
        let mut bytes = [0u8; 4 * 8];
        let len = if with_tramp { 4 * 8 } else { 3 * 8 };
        if !crate::user::buffer_ok(inp, len) || !crate::user::copy_from_user(task::current_aspace(), inp, &mut bytes[..len]) {
            return false;
        }
        let word = |i: usize| u64::from_le_bytes(bytes[i * 8..i * 8 + 8].try_into().unwrap()) as usize;
        let (handler, flags, mask, tramp) = (word(0), word(1), word(2), word(3));
        if !task::signal_set_action(id, sig, handler, flags as u32, mask as u32, tramp) {
            return false;
        }
    }
    true
}
