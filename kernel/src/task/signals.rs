//! Per-task signal masks: pending, ignored and blocked bits.

use super::*;

pub fn signal_is_ignored(id: usize, bit: u32) -> bool {
    if id >= MAX_TASKS {
        return false;
    }
    let flags = irq_save();
    irq_off();
    let t = TASKS.lock()[id];
    let out = (t.sig_ignored & bit) != 0;
    irq_restore(flags);
    out
}

/// Mark `bit` pending on `id` if it is a live user task (Ready/Running).
pub fn signal_set_pending(id: usize, bit: u32) {
    if id >= MAX_TASKS {
        return;
    }
    let flags = irq_save();
    irq_off();
    let mut tasks = TASKS.lock();
    let stored = tasks[id].user_rip != 0
        && matches!(tasks[id].state, State::Ready | State::Running);
    if stored {
        tasks[id].sig_pending |= bit;
    }
    drop(tasks);
    irq_restore(flags);
}

pub fn signal_clear_pending(id: usize, bit: u32) {
    if id >= MAX_TASKS {
        return;
    }
    let flags = irq_save();
    irq_off();
    TASKS.lock()[id].sig_pending &= !bit;
    irq_restore(flags);
}

/// Consume `bit` from `id`'s pending set: true if it was pending (now cleared).
/// Used by libgloss to pick up `SIGCHLD` (no userspace handler trampolines yet).
pub fn signal_take_pending(id: usize, bit: u32) -> bool {
    if id >= MAX_TASKS {
        return false;
    }
    let flags = irq_save();
    irq_off();
    let mut tasks = TASKS.lock();
    let had = tasks[id].sig_pending & bit != 0;
    tasks[id].sig_pending &= !bit;
    drop(tasks);
    irq_restore(flags);
    had
}

pub fn signal_set_ignored(id: usize, bit: u32, ign: bool) {
    if id >= MAX_TASKS {
        return;
    }
    let flags = irq_save();
    irq_off();
    let mut tasks = TASKS.lock();
    if ign {
        tasks[id].sig_ignored |= bit;
    } else {
        tasks[id].sig_ignored &= !bit;
    }
    drop(tasks);
    irq_restore(flags);
}

pub fn signal_blocked(id: usize) -> u32 {
    if id >= MAX_TASKS {
        return 0;
    }
    let flags = irq_save();
    irq_off();
    let out = TASKS.lock()[id].sig_blocked;
    irq_restore(flags);
    out
}

pub fn signal_block(id: usize, bits: u32) {
    if id >= MAX_TASKS {
        return;
    }
    let flags = irq_save();
    irq_off();
    TASKS.lock()[id].sig_blocked |= bits;
    irq_restore(flags);
}

pub fn signal_set_blocked_mask(id: usize, mask: u32) {
    if id >= MAX_TASKS {
        return;
    }
    let flags = irq_save();
    irq_off();
    TASKS.lock()[id].sig_blocked = mask;
    irq_restore(flags);
}

/// Effective delivery mask: pending minus blocked, with `SIGKILL` never blockable.
pub fn signal_effective(id: usize) -> u32 {
    if id >= MAX_TASKS {
        return 0;
    }
    let flags = irq_save();
    irq_off();
    let t = TASKS.lock()[id];
    let kill_bit = 1u32 << 9;
    let out = (t.sig_pending & !t.sig_blocked) | (t.sig_pending & kill_bit);
    irq_restore(flags);
    out
}

/// True if `id` has a pending default-fatal signal (`SIGINT`/`SIGKILL`/`SIGTERM`).
///
/// Must stay aligned with [`signal_take_fatal`]: waking `input::read` on a
/// non-fatal pending bit (e.g. `SIGHUP` at `SIG_DFL`) would return 0 to
/// userspace without `deliver_due` exiting the task — getty treats that as
/// EOF, exits, and init respawns another `login: ` on the same line.
pub fn signal_pending_actionable(id: usize) -> bool {
    if id >= MAX_TASKS {
        return false;
    }
    let flags = irq_save();
    irq_off();
    let t = TASKS.lock()[id];
    // SIGKILL cannot be ignored even if the bit is set in ignored.
    let kill_bit = 1u32 << crate::signal::SIGKILL;
    let pending = t.sig_pending;
    let effective = (pending & !t.sig_ignored) | (pending & kill_bit);
    let out = crate::signal::DEFAULT_FATAL
        .iter().any(|sig| effective & (1u32 << sig) != 0);
    irq_restore(flags);
    out
}

/// Take the lowest-numbered default-fatal pending signal (SIGINT/KILL/TERM),
/// clearing its pending bit. Returns `None` if none.
pub fn signal_take_fatal(id: usize) -> Option<u32> {
    if id >= MAX_TASKS {
        return None;
    }
    let flags = irq_save();
    irq_off();
    let mut tasks = TASKS.lock();
    let t = &mut tasks[id];
    let kill_bit = 1u32 << crate::signal::SIGKILL;
    let effective = ((t.sig_pending & !t.sig_ignored) | (t.sig_pending & kill_bit))
        & !t.sig_blocked
        | (t.sig_pending & kill_bit);
    let mut found = None;
    for sig in crate::signal::DEFAULT_FATAL {
        let bit = 1u32 << sig;
        if effective & bit != 0 {
            t.sig_pending &= !bit;
            found = Some(sig);
            break;
        }
    }
    drop(tasks);
    irq_restore(flags);
    found
}
