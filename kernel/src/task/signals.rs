//! Signal state: the pending / blocked bits of each thread, the
//! dispositions of each process (`sig_ignored` and the table of caught
//! handlers, in the leader's slot), and the queries the signal code makes on
//! them. Ids are thread ids; a signal sent to a pid goes to the leader
//! thread. Policy (default actions, frame layout, syscalls) lives in
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

/// Caught handlers of one process, plus the libc trampoline that runs them
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
fn with_sig<R>(f: impl FnOnce(&mut TaskTable, &mut [SigTable; MAX_TASKS]) -> R) -> R {
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

/// What `sig` does to thread `id`, from its process's dispositions.
fn disposition(tasks: &TaskTable, tabs: &[SigTable; MAX_TASKS], id: usize, sig: u32) -> Disposition {
    if sig == signal::SIGKILL {
        return Disposition::Terminate;
    }
    let pid = tasks[id].tgid;
    if tasks.sig_ignored(pid) & (1 << sig) != 0 {
        return Disposition::Ignore;
    }
    let tab = &tabs[pid];
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
    let kicks = with_sig(|tasks, tabs| send_locked(tasks, tabs, id, sig));
    kick_cpus_mask(kicks);
}

/// `sig` to `id` for a caller that holds `TASKS` (the timer interrupt's
/// `SIGALRM`): `None` when the handler tables are busy, to retry later;
/// otherwise the CPUs to kick.
pub(super) fn signal_send_locked(tasks: &mut TaskTable, id: usize, sig: u32) -> Option<u64> {
    let mut tabs = SIG_TABLES.try_lock()?;
    Some(send_locked(tasks, &mut tabs, id, sig))
}

/// Whether `t` is a user task a signal can still reach.
pub(super) fn signal_live(t: &Task) -> bool {
    t.user_rip != 0 && matches!(t.state, State::Ready | State::Running | State::Blocked)
}

fn send_locked(tasks: &mut TaskTable, tabs: &mut [SigTable; MAX_TASKS], id: usize, sig: u32) -> u64 {
    let bit = 1u32 << sig;
    let t = &tasks[id];
    if !signal_live(t) {
        return 0;
    }
    let blocked = t.sig_blocked & bit != 0 && bit & UNBLOCKABLE == 0;
    if !blocked && matches!(disposition(tasks, tabs, id, sig), Disposition::Ignore) {
        return 0;
    }
    tasks[id].sig_pending |= bit;
    // A signal that acts ends any blocking wait (EINTR / termination).
    if !blocked {
        wake_task_locked(tasks, id)
    } else {
        0
    }
}

/// True if a pending signal should break `id` out of a blocking wait: one
/// that is unblocked and would terminate the task or run a handler.
pub fn signal_wakeable(id: usize) -> bool {
    if id >= MAX_TASKS {
        return false;
    }
    with_sig(|tasks, tabs| {
        let set = deliverable(&tasks[id]);
        (1..32u32).any(|sig| {
            set & (1 << sig) != 0 && !matches!(disposition(tasks, tabs, id, sig), Disposition::Ignore)
        })
    })
}

/// Whether `SIGKILL` is pending for `id` (it acts even when blocked).
pub fn signal_kill_pending(id: usize) -> bool {
    id < MAX_TASKS && with_sig(|tasks, _| tasks[id].sig_pending & KILL_BIT != 0)
}

/// Consume the lowest-numbered pending, unblocked signal of `id` that has an
/// effect, dropping ignored ones on the way. For a caught signal this also
/// applies the handler's mask (and `SA_RESETHAND`) before returning.
pub fn signal_next(id: usize) -> SigNext {
    if id >= MAX_TASKS {
        return SigNext::None;
    }
    with_sig(|tasks, tabs| {
        let pid = tasks[id].tgid;
        loop {
            let set = deliverable(&tasks[id]);
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
            tasks[id].sig_pending &= !bit;
            match disposition(tasks, tabs, id, sig) {
                Disposition::Ignore => continue,
                Disposition::Terminate => return SigNext::Terminate(sig),
                Disposition::Catch(act) => {
                    let t = &mut tasks[id];
                    let old_blocked = t.sig_blocked;
                    let mut add = act.mask;
                    if act.flags & signal::SA_NODEFER == 0 {
                        add |= bit;
                    }
                    t.sig_blocked = (t.sig_blocked | add) & !UNBLOCKABLE;
                    let tab = &mut tabs[pid];
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
        let pid = tasks[id].tgid;
        let a = tabs[pid].act[sig as usize];
        if tasks.sig_ignored(pid) & (1 << sig) != 0 {
            return (signal::HANDLER_IGN, a.flags, 0);
        }
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
        let pid = tasks[id].tgid;
        let tab = &mut tabs[pid];
        // Dropping a now-ignored pending signal from every thread of the process.
        let discard = |tasks: &mut TaskTable| {
            for t in tasks.iter_mut().filter(|t| t.tgid == pid && t.state != State::Unused) {
                t.sig_pending &= !bit;
            }
        };
        match handler {
            signal::HANDLER_IGN => {
                tasks.proc_mut(pid).sig_ignored |= bit;
                // No handler, but the flags still count (`SA_NOCLDWAIT`).
                tab.act[sig as usize] = SigAction { flags, ..NO_ACTION };
                // POSIX: setting SIG_IGN discards a pending instance, blocked or not.
                discard(tasks);
            }
            signal::HANDLER_DFL => {
                tasks.proc_mut(pid).sig_ignored &= !bit;
                tab.act[sig as usize] = SigAction { flags, ..NO_ACTION };
                if !signal::default_terminates(sig) {
                    discard(tasks);
                }
            }
            _ => {
                tasks.proc_mut(pid).sig_ignored &= !bit;
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

/// Whether process `pid` wants no zombies: it ignores `SIGCHLD` or set
/// `SA_NOCLDWAIT` for it. Caller holds `TASKS` (it comes first in lock order).
pub(super) fn signal_no_zombies(tasks: &TaskTable, pid: usize) -> bool {
    if pid >= MAX_TASKS {
        return false;
    }
    let chld = signal::SIGCHLD as usize;
    let flags = irq_save();
    irq_off();
    let nocldwait = SIG_TABLES.lock()[pid].act[chld].flags & signal::SA_NOCLDWAIT != 0;
    irq_restore(flags);
    nocldwait || tasks.sig_ignored(pid) & (1 << chld) != 0
}

/// A new task in `slot` starts with no handlers.
pub(super) fn signal_table_reset(slot: usize) {
    if slot < MAX_TASKS {
        with_tables(|tabs| tabs[slot] = EMPTY_TABLE);
        signal_thread_reset(slot);
    }
}

/// A new thread in `slot`: no interrupted wait, no `sigsuspend` mask (the
/// handlers are its process's).
pub(super) fn signal_thread_reset(slot: usize) {
    if slot < MAX_TASKS {
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
