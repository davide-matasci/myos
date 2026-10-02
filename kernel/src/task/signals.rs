//! Per-task signal state: the pending / ignored / blocked bits on [`Task`],
//! the table of caught handlers, and the queries the signal code makes on
//! them. Policy (default actions, frame layout, syscalls) lives in
//! [`crate::signal`].

use super::*;
use crate::signal;

/// A caught signal's registration: `sigaction` with a function handler.
#[derive(Clone, Copy)]
pub struct SigAction {
    /// User address of the handler (never `SIG_DFL`/`SIG_IGN`).
    pub handler: usize,
    /// Extra signals blocked while the handler runs (`sa_mask`).
    pub mask: u32,
    /// `SA_*` flags.
    pub flags: u32,
}

const NO_ACTION: SigAction = SigAction {
    handler: 0,
    mask: 0,
    flags: 0,
};

/// Caught handlers of one task, plus the libc trampoline that runs them
/// (registered with each `sigaction`; 0 = none, so nothing can be caught).
#[derive(Clone, Copy)]
struct SigTable {
    act: [SigAction; 32],
    tramp: usize,
}

const EMPTY_TABLE: SigTable = SigTable {
    act: [NO_ACTION; 32],
    tramp: 0,
};

/// Kept apart from [`Task`] so the frequent whole-`Task` copies stay small.
static SIG_TABLES: Mutex<[SigTable; MAX_TASKS]> = Mutex::new([EMPTY_TABLE; MAX_TASKS]);

/// Set by a blocking syscall that gave up because a signal arrived; the
/// syscall exit path turns it into `EINTR` or a restart.
static INTERRUPTED: [AtomicBool; MAX_TASKS] = [const { AtomicBool::new(false) }; MAX_TASKS];

/// Mask in force before `sigsuspend` swapped in its own (`u64::MAX` = none):
/// the handler that ends the suspension restores this one, not the
/// temporary mask.
static SUSPEND_MASK: [AtomicU64; MAX_TASKS] = [const { AtomicU64::new(u64::MAX) }; MAX_TASKS];

const KILL_BIT: u32 = 1 << signal::SIGKILL;
/// Neither blockable nor catchable.
const UNBLOCKABLE: u32 = KILL_BIT | (1 << signal::SIGSTOP);

/// What a pending signal does to its task right now.
#[derive(Clone, Copy)]
pub enum Disposition {
    Ignore,
    Terminate,
    Catch(SigAction),
}

/// Next step for a task's pending, unblocked signals (see [`signal_next`]).
pub enum SigNext {
    None,
    Terminate(u32),
    /// Run `act` for `sig`; `old_blocked` is restored by `sigreturn`.
    Deliver {
        sig: u32,
        act: SigAction,
        tramp: usize,
        old_blocked: u32,
    },
}

/// Run `f` on the task and handler tables with IRQs off (lock order:
/// `TASKS`, then `SIG_TABLES`).
fn with_sig<R>(f: impl FnOnce(&mut [Task; MAX_TASKS], &mut [SigTable; MAX_TASKS]) -> R) -> R {
    let flags = irq_save();
    irq_off();
    let mut tasks = TASKS.lock();
    let mut tabs = SIG_TABLES.lock();
    let r = f(&mut tasks, &mut tabs);
    drop(tabs);
    drop(tasks);
    irq_restore(flags);
    r
}

fn disposition(t: &Task, tab: &SigTable, sig: u32) -> Disposition {
    if sig == signal::SIGKILL {
        return Disposition::Terminate;
    }
    if t.sig_ignored & (1 << sig) != 0 {
        return Disposition::Ignore;
    }
    let act = tab.act[sig as usize];
    if act.handler != 0 && tab.tramp != 0 {
        return Disposition::Catch(act);
    }
    if signal::default_terminates(sig) {
        Disposition::Terminate
    } else {
        Disposition::Ignore
    }
}

/// Pending bits that may act now: unblocked, plus `SIGKILL` always.
fn deliverable(t: &Task) -> u32 {
    t.sig_pending & (!t.sig_blocked | KILL_BIT)
}

/// Post `sig` to `id`. A signal whose disposition is to ignore it is dropped
/// at once unless it is blocked: the disposition may change before it is
/// unblocked, so a blocked signal always stays pending (Linux semantics).
/// No-op for a slot that is not a live user task.
pub fn signal_send(id: usize, sig: u32) {
    if id >= MAX_TASKS || sig == 0 || sig > 31 {
        return;
    }
    let bit = 1u32 << sig;
    let kicks = with_sig(|tasks, tabs| {
        let t = &mut tasks[id];
        let live = t.user_rip != 0
            && matches!(t.state, State::Ready | State::Running | State::Blocked);
        if !live {
            return 0;
        }
        let blocked = t.sig_blocked & bit != 0 && bit & UNBLOCKABLE == 0;
        if !blocked && matches!(disposition(t, &tabs[id], sig), Disposition::Ignore) {
            return 0;
        }
        t.sig_pending |= bit;
        // A signal that acts ends any blocking wait (EINTR / termination).
        if !blocked {
            wake_task_locked(tasks, id)
        } else {
            0
        }
    });
    kick_cpus_mask(kicks);
}

/// True if a pending signal should break `id` out of a blocking wait: one
/// that is unblocked and would terminate the task or run a handler.
pub fn signal_wakeable(id: usize) -> bool {
    if id >= MAX_TASKS {
        return false;
    }
    with_sig(|tasks, tabs| {
        let t = &tasks[id];
        let set = deliverable(t);
        (1..32u32).any(|sig| {
            set & (1 << sig) != 0 && !matches!(disposition(t, &tabs[id], sig), Disposition::Ignore)
        })
    })
}

/// Consume the lowest-numbered pending, unblocked signal of `id` that has an
/// effect, dropping ignored ones on the way. For a caught signal this also
/// applies the handler's mask (and `SA_RESETHAND`) before returning.
pub fn signal_next(id: usize) -> SigNext {
    if id >= MAX_TASKS {
        return SigNext::None;
    }
    with_sig(|tasks, tabs| {
        let t = &mut tasks[id];
        let tab = &mut tabs[id];
        loop {
            let set = deliverable(t);
            if set == 0 {
                return SigNext::None;
            }
            // SIGKILL first, then the lowest number.
            let sig = if set & KILL_BIT != 0 {
                signal::SIGKILL
            } else {
                set.trailing_zeros()
            };
            let bit = 1u32 << sig;
            t.sig_pending &= !bit;
            match disposition(t, tab, sig) {
                Disposition::Ignore => continue,
                Disposition::Terminate => return SigNext::Terminate(sig),
                Disposition::Catch(act) => {
                    let old_blocked = t.sig_blocked;
                    let mut add = act.mask;
                    if act.flags & signal::SA_NODEFER == 0 {
                        add |= bit;
                    }
                    t.sig_blocked = (t.sig_blocked | add) & !UNBLOCKABLE;
                    if act.flags & signal::SA_RESETHAND != 0 {
                        tab.act[sig as usize] = NO_ACTION;
                    }
                    return SigNext::Deliver {
                        sig,
                        act,
                        tramp: tab.tramp,
                        old_blocked,
                    };
                }
            }
        }
    })
}

/// `(handler, flags, mask)` as `sigaction` reports it: `SIG_IGN`, `SIG_DFL`
/// (0) or the caught handler's address.
pub fn signal_get_action(id: usize, sig: u32) -> (usize, u32, u32) {
    if id >= MAX_TASKS || sig == 0 || sig > 31 {
        return (signal::HANDLER_DFL, 0, 0);
    }
    with_sig(|tasks, tabs| {
        if tasks[id].sig_ignored & (1 << sig) != 0 {
            return (signal::HANDLER_IGN, 0, 0);
        }
        let a = tabs[id].act[sig as usize];
        (a.handler, a.flags, a.mask)
    })
}

/// Install a disposition for `sig`: `SIG_DFL`, `SIG_IGN`, or a handler that
/// runs through `tramp`. Returns false for a signal that cannot be caught or
/// ignored (`SIGKILL`, `SIGSTOP`) or a handler without a trampoline.
pub fn signal_set_action(
    id: usize,
    sig: u32,
    handler: usize,
    flags: u32,
    mask: u32,
    tramp: usize,
) -> bool {
    if id >= MAX_TASKS || sig == 0 || sig > 31 {
        return false;
    }
    let bit = 1u32 << sig;
    if bit & UNBLOCKABLE != 0 && handler != signal::HANDLER_DFL {
        return false;
    }
    if handler > signal::HANDLER_IGN && tramp == 0 {
        return false;
    }
    with_sig(|tasks, tabs| {
        let t = &mut tasks[id];
        let tab = &mut tabs[id];
        match handler {
            signal::HANDLER_IGN => {
                t.sig_ignored |= bit;
                tab.act[sig as usize] = NO_ACTION;
                // POSIX: setting SIG_IGN discards a pending instance, blocked or not.
                t.sig_pending &= !bit;
            }
            signal::HANDLER_DFL => {
                t.sig_ignored &= !bit;
                tab.act[sig as usize] = NO_ACTION;
                if !signal::default_terminates(sig) {
                    t.sig_pending &= !bit;
                }
            }
            _ => {
                t.sig_ignored &= !bit;
                tab.act[sig as usize] = SigAction {
                    handler,
                    mask: mask & !UNBLOCKABLE,
                    flags,
                };
                tab.tramp = tramp;
            }
        }
        true
    })
}

pub fn signal_blocked(id: usize) -> u32 {
    if id >= MAX_TASKS {
        return 0;
    }
    with_sig(|tasks, _| tasks[id].sig_blocked)
}

/// Replace the blocked mask (`SIGKILL`/`SIGSTOP` are never blocked).
pub fn signal_set_blocked_mask(id: usize, mask: u32) {
    if id >= MAX_TASKS {
        return;
    }
    with_sig(|tasks, _| tasks[id].sig_blocked = mask & !UNBLOCKABLE);
}

/// Pending signals of `id` (`sigpending`).
pub fn signal_pending(id: usize) -> u32 {
    if id >= MAX_TASKS {
        return 0;
    }
    with_sig(|tasks, _| tasks[id].sig_pending)
}

/// Consume the lowest pending signal of `id` that is in `set` (`sigwait`).
pub fn signal_take_from(id: usize, set: u32) -> Option<u32> {
    if id >= MAX_TASKS {
        return None;
    }
    with_sig(|tasks, _| {
        let hit = tasks[id].sig_pending & set & !1;
        if hit == 0 {
            return None;
        }
        let sig = hit.trailing_zeros();
        tasks[id].sig_pending &= !(1 << sig);
        Some(sig)
    })
}

/// `sigsuspend`: remember the mask to restore once a handler has run.
pub fn signal_save_suspend_mask(id: usize, mask: u32) {
    if id < MAX_TASKS {
        SUSPEND_MASK[id].store(mask as u64, Ordering::SeqCst);
    }
}

/// Take the mask saved by [`signal_save_suspend_mask`], if any.
pub fn signal_take_suspend_mask(id: usize) -> Option<u32> {
    if id >= MAX_TASKS {
        return None;
    }
    let m = SUSPEND_MASK[id].swap(u64::MAX, Ordering::SeqCst);
    (m != u64::MAX).then_some(m as u32)
}

/// Note that `id`'s blocking syscall stopped waiting because of a signal.
pub fn signal_mark_interrupted(id: usize) {
    if id < MAX_TASKS {
        INTERRUPTED[id].store(true, Ordering::SeqCst);
    }
}

/// Take (and clear) the flag set by [`signal_mark_interrupted`].
pub fn signal_take_interrupted(id: usize) -> bool {
    id < MAX_TASKS && INTERRUPTED[id].swap(false, Ordering::SeqCst)
}

/// Take-and-clear a pending bit: the legacy `SIGCHLD_TAKE` syscall that
/// libgloss used before handlers were delivered by the kernel.
pub fn signal_take_pending(id: usize, bit: u32) -> bool {
    if id >= MAX_TASKS {
        return false;
    }
    with_sig(|tasks, _| {
        let had = tasks[id].sig_pending & bit != 0;
        tasks[id].sig_pending &= !bit;
        had
    })
}

/// Run `f` on the handler table alone, IRQs off. Callers must not hold
/// `SIG_TABLES`; holding `TASKS` is fine (it comes first in lock order).
fn with_tables(f: impl FnOnce(&mut [SigTable; MAX_TASKS])) {
    let flags = irq_save();
    irq_off();
    f(&mut SIG_TABLES.lock());
    irq_restore(flags);
}

/// A new task in `slot` starts with no handlers.
pub(super) fn signal_table_reset(slot: usize) {
    if slot < MAX_TASKS {
        with_tables(|tabs| tabs[slot] = EMPTY_TABLE);
        INTERRUPTED[slot].store(false, Ordering::SeqCst);
        SUSPEND_MASK[slot].store(u64::MAX, Ordering::SeqCst);
    }
}

/// fork: the child inherits the parent's handlers and trampoline.
pub(super) fn signal_table_fork(parent: usize, child: usize) {
    if parent < MAX_TASKS && child < MAX_TASKS {
        with_tables(|tabs| tabs[child] = tabs[parent]);
        INTERRUPTED[child].store(false, Ordering::SeqCst);
        SUSPEND_MASK[child].store(u64::MAX, Ordering::SeqCst);
    }
}

/// exec: caught signals revert to `SIG_DFL` (the handlers are gone with the
/// old image); ignored ones and the blocked mask are kept by the caller.
pub(super) fn signal_table_exec(slot: usize) {
    signal_table_reset(slot);
}
