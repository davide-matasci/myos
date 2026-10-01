//! Process groups, sessions and the controlling terminal.

use super::*;

pub fn task_pgid(id: usize) -> Option<usize> {
    if id >= MAX_TASKS {
        return None;
    }
    let flags = irq_save();
    irq_off();
    let t = TASKS.lock()[id];
    let out = if t.user_rip != 0 && t.state != State::Unused {
        Some(t.pgid)
    } else {
        None
    };
    irq_restore(flags);
    out
}

pub fn current_pgid() -> Option<usize> {
    task_pgid(current_slot())
}

pub fn task_has_ctty(id: usize) -> bool {
    if id >= MAX_TASKS {
        return false;
    }
    let flags = irq_save();
    irq_off();
    let t = TASKS.lock()[id];
    let out = t.user_rip != 0
        && matches!(t.state, State::Ready | State::Running)
        && !t.exited
        && t.has_ctty;
    irq_restore(flags);
    out
}

pub fn has_ctty() -> bool {
    let flags = irq_save();
    irq_off();
    let id = current_slot();
    let t = TASKS.lock()[id];
    irq_restore(flags);
    t.has_ctty
}

/// Mark the system console as this task's controlling terminal (TIOCSCTTY).
pub fn set_ctty() {
    with_current_mut(|t| {
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
    with_current_mut(|t| {
        let pid = current_slot();
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
    t.state != State::Unused
}

/// `getpgid(pid)`: `pid == 0` means the caller. Returns the process group id,
/// or `None` if `pid` does not name an existing task (ESRCH).
pub fn getpgid(pid: usize) -> Option<usize> {
    let flags = irq_save();
    irq_off();
    let caller = current_slot();
    let target = if pid == 0 { caller } else { pid };
    let out = if target >= MAX_TASKS {
        None
    } else {
        let t = TASKS.lock()[target];
        if task_exists(&t) {
            Some(t.pgid)
        } else {
            None
        }
    };
    irq_restore(flags);
    out
}

/// `getsid(pid)`: `pid == 0` means the caller. Returns the session id, or
/// `None` if `pid` does not name an existing task (ESRCH).
pub fn getsid(pid: usize) -> Option<usize> {
    let flags = irq_save();
    irq_off();
    let caller = current_slot();
    let target = if pid == 0 { caller } else { pid };
    let out = if target >= MAX_TASKS {
        None
    } else {
        let t = TASKS.lock()[target];
        if task_exists(&t) {
            Some(t.sid)
        } else {
            None
        }
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
/// - Caller may change only itself or a direct child (`ppid == caller`).
///   Full POSIX also requires the child not to have `exec`'d yet; we do not
///   track post-exec and allow any same-session direct child.
/// - New `pgid` must be the target's pid (new group) or an existing `pgid` in
///   the same session. Session leaders may not leave their group (EPERM)
///   except a no-op that keeps the current `pgid`.
///
/// Returns `true` on success, `false` on ESRCH/EPERM/EINVAL (all mapped to
/// SYSERR in the syscall layer).
pub fn setpgid(pid: usize, pgid: usize) -> bool {
    let flags = irq_save();
    irq_off();
    let mut tasks = TASKS.lock();
    let caller = current_slot();
    let target = if pid == 0 { caller } else { pid };

    let ok = (|| {
        if target >= MAX_TASKS || !task_exists(&tasks[target]) {
            return false;
        }
        if !task_exists(&tasks[caller]) {
            return false;
        }
        if tasks[target].sid != tasks[caller].sid {
            return false;
        }
        if target != caller && tasks[target].ppid != caller {
            return false;
        }

        let new_pgid = if pgid == 0 { target } else { pgid };
        if new_pgid >= MAX_TASKS {
            return false;
        }
        if new_pgid == tasks[target].pgid {
            return true; // no-op
        }
        // Session leader cannot move to a different process group.
        if tasks[target].sid == target {
            return false;
        }
        let same_sid = tasks[target].sid;
        let allowed = new_pgid == target
            || tasks.iter().any(|t| {
                task_exists(t) && t.sid == same_sid && t.pgid == new_pgid
            });
        if !allowed {
            return false;
        }
        tasks[target].pgid = new_pgid;
        true
    })();

    drop(tasks);
    irq_restore(flags);
    ok
}
