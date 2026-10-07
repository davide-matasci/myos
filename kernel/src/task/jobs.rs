//! Process groups, sessions and the controlling terminal. All of it is
//! process state: ids here are pids (leader slots), and a thread's id
//! resolves to its process.

use super::*;

pub fn task_pgid(id: usize) -> Option<usize> {
    if id >= MAX_TASKS {
        return None;
    }
    let flags = irq_save();
    irq_off();
    let out = {
        let tasks = TASKS.lock();
        let t = &tasks[id];
        if t.user_rip != 0 && t.state != State::Unused && t.tgid == id {
            Some(tasks.proc(id).pgid)
        } else {
            None
        }
    };
    irq_restore(flags);
    out
}

pub fn current_pgid() -> Option<usize> {
    task_pgid(current_pid())
}

/// The parent of process `id`, `None` for init, an orphan or a free slot.
pub fn parent_pid(id: usize) -> Option<usize> {
    if id >= MAX_TASKS {
        return None;
    }
    let flags = irq_save();
    irq_off();
    let out = {
        let tasks = TASKS.lock();
        let t = &tasks[id];
        if t.user_rip != 0 && t.state != State::Unused && t.ppid != NO_PARENT && t.ppid < MAX_TASKS {
            Some(t.ppid)
        } else {
            None
        }
    };
    irq_restore(flags);
    out
}

pub fn task_has_ctty(id: usize) -> bool {
    if id >= MAX_TASKS {
        return false;
    }
    let flags = irq_save();
    irq_off();
    let out = {
        let tasks = TASKS.lock();
        let t = &tasks[id];
        t.user_rip != 0
            && t.tgid == id
            && matches!(t.state, State::Ready | State::Running | State::Blocked)
            && !t.exited
            && tasks.proc(id).has_ctty
    };
    irq_restore(flags);
    out
}

pub fn has_ctty() -> bool {
    with_process_mut(|t| t.has_ctty)
}

/// Mark the system console as this task's controlling terminal (TIOCSCTTY).
pub fn set_ctty() {
    with_process_mut(|t| {
        t.has_ctty = true;
    });
}

/// Create a new session: caller becomes session leader (`sid = pid` / task slot),
/// joins a new process group (`pgid = pid`), and loses any controlling terminal
/// (`has_ctty = false`).
///
/// Phase-1 vs full POSIX:
/// - Fails when the caller is already a session leader (`sid == pid`). Full
///   POSIX also rejects process-group leaders that are not session leaders; we
///   approximate by requiring `sid != pid` only (a group leader that is not a
///   session leader can still call `setsid` and becomes both).
/// - Returns the new session id (task slot) on success, or `None` (EPERM).
pub fn setsid() -> Option<usize> {
    let pid = current_pid();
    with_process_mut(|t| {
        if t.sid == pid {
            return None;
        }
        t.sid = pid;
        t.pgid = pid;
        t.has_ctty = false;
        Some(pid)
    })
}

fn task_exists(t: &Task) -> bool {
    !matches!(t.state, State::Unused | State::Claimed)
}

/// `getpgid(pid)`: `pid == 0` means the caller. Returns the process group id,
/// or `None` if `pid` does not name an existing task (ESRCH).
pub fn getpgid(pid: usize) -> Option<usize> {
    let flags = irq_save();
    irq_off();
    let out = {
        let tasks = TASKS.lock();
        // `process_of` resolves to a leader slot, but a pid that names a
        // kernel thread or idle task resolves to one with no process block;
        // `proc_opt` returns None (ESRCH) there instead of panicking.
        process_of(&tasks, if pid == 0 { current_slot() } else { pid })
            .and_then(|p| tasks.proc_opt(p))
            .map(|p| p.pgid)
    };
    irq_restore(flags);
    out
}

/// The process `id` (a pid or a thread's tid) belongs to, if it exists.
fn process_of(tasks: &TaskTable, id: usize) -> Option<usize> {
    (id < MAX_TASKS && task_exists(&tasks[id])).then(|| tasks[id].tgid)
}

/// `getsid(pid)`: `pid == 0` means the caller. Returns the session id, or
/// `None` if `pid` does not name an existing task (ESRCH).
pub fn getsid(pid: usize) -> Option<usize> {
    let flags = irq_save();
    irq_off();
    let out = {
        let tasks = TASKS.lock();
        process_of(&tasks, if pid == 0 { current_slot() } else { pid })
            .and_then(|p| tasks.proc_opt(p))
            .map(|p| p.sid)
    };
    irq_restore(flags);
    out
}

/// `setpgid(pid, pgid)` — phase-1 process groups for shells later.
///
/// Semantics (approximate POSIX):
/// - `pid == 0` → caller; `pgid == 0` → use the *target* process id as the new
///   group id (create a group led by that process).
/// - Target must exist and share the caller's session.
/// - Caller may change only itself or a direct child (`ppid == caller`)
///   that has not exec'd since the fork ([`SetpgidError::Execd`]).
/// - New `pgid` must be the target's pid (new group) or an existing `pgid` in
///   the same session. Session leaders may not leave their group (EPERM)
///   except a no-op that keeps the current `pgid`.
///
/// Fails with [`SetpgidError::Execd`] (EACCES) or else
/// [`SetpgidError::Refused`] (ESRCH/EPERM/EINVAL, the generic SYSERR).
pub fn setpgid(pid: usize, pgid: usize) -> Result<(), SetpgidError> {
    let flags = irq_save();
    irq_off();
    let mut tasks = TASKS.lock();
    let caller = tasks[current_slot()].tgid;
    let target = if pid == 0 { caller } else { pid };

    let mut execd = false;
    let ok = (|| {
        if target >= MAX_TASKS || !task_exists(&tasks[target]) || tasks[target].tgid != target {
            return false;
        }
        if !task_exists(&tasks[caller]) {
            return false;
        }
        // Both must lead a real process: a pid naming a kernel thread or idle
        // task resolves to a leader slot with no process block, and the
        // `.proc()` below would panic on it.
        if tasks.proc_opt(target).is_none() || tasks.proc_opt(caller).is_none() {
            return false;
        }
        if tasks.proc(target).sid != tasks.proc(caller).sid {
            return false;
        }
        if target != caller && tasks[target].ppid != caller {
            return false;
        }
        if target != caller && tasks.proc(target).execd {
            execd = true;
            return false;
        }

        let new_pgid = if pgid == 0 { target } else { pgid };
        if new_pgid >= MAX_TASKS {
            return false;
        }
        if new_pgid == tasks.proc(target).pgid {
            return true; // no-op
        }
        // Session leader cannot move to a different process group.
        if tasks.proc(target).sid == target {
            return false;
        }
        let same_sid = tasks.proc(target).sid;
        let allowed = new_pgid == target
            || (0..MAX_TASKS).any(|i| {
                task_exists(&tasks[i])
                    && tasks[i].tgid == i
                    && tasks
                        .proc_opt(i)
                        .is_some_and(|p| p.sid == same_sid && p.pgid == new_pgid)
            });
        if !allowed {
            return false;
        }
        tasks.proc_mut(target).pgid = new_pgid;
        true
    })();

    drop(tasks);
    irq_restore(flags);
    match (ok, execd) {
        (true, _) => Ok(()),
        (false, true) => Err(SetpgidError::Execd),
        (false, false) => Err(SetpgidError::Refused),
    }
}

/// Why `setpgid` failed.
pub enum SetpgidError {
    /// The target is a child that has exec'd (POSIX `EACCES`).
    Execd,
    /// Anything else (no such process, another session, not a child, no
    /// such group, a session leader).
    Refused,
}
