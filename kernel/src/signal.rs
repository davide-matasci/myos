//! Process signals (phase-1).
//!
//! Privileged delivery only: pending/ignored bitmasks live on each task, and
//! cross-process `kill` must go through the kernel. Disposition policy and
//! constants live here so libgloss/libc stay thin wrappers.
//!
//! Signal numbers **must match** newlib `<signal.h>` / Linux (and rustix_compat):
//! `SIGINT=2`, `SIGKILL=9`, `SIGTERM=15`.

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::task;

/// Must match newlib `<signal.h>` / Linux.
pub const SIGINT: u32 = 2;
/// Must match newlib `<signal.h>` / Linux.
pub const SIGKILL: u32 = 9;
/// Must match newlib `<signal.h>` / Linux.
pub const SIGTERM: u32 = 15;

/// `SIG_DFL` — default action (terminate for the signals we deliver).
pub const HANDLER_DFL: usize = 0;
/// `SIG_IGN` — ignore (except `SIGKILL`, which cannot be ignored).
pub const HANDLER_IGN: usize = 1;

/// Task id currently blocked in [`crate::input::read`], or `usize::MAX` if none.
///
/// Phase-1 foreground for `^C`: prefer this task's process group; if nobody is
/// in a console read, fall back to every live user task with `has_ctty`.
static INPUT_READER: AtomicUsize = AtomicUsize::new(usize::MAX);

#[inline]
fn sig_bit(sig: u32) -> Option<u32> {
    if sig == 0 || sig > 31 {
        return None;
    }
    Some(1u32 << sig)
}

/// Mark the current task as blocked in console `input::read` (for `^C` fg).
pub fn enter_input_read() {
    INPUT_READER.store(task::current_id(), Ordering::SeqCst);
}

/// Clear the console-read marker.
pub fn leave_input_read() {
    INPUT_READER.store(usize::MAX, Ordering::SeqCst);
}

/// True if the current task has a pending signal that should break a wait
/// (ignored signals are not pending; `SIGKILL` is never ignored).
pub fn current_should_wake() -> bool {
    task::signal_pending_actionable(task::current_id())
}

/// Send `sig` according to POSIX-ish pid rules:
/// - `pid > 0`: that task id
/// - `pid == 0`: current process group
/// - `pid < 0`: process group `-pid`
///
/// Returns `false` if `sig` is invalid or no matching live user task was found.
pub fn kill(pid: isize, sig: u32) -> bool {
    let Some(bit) = sig_bit(sig) else {
        return false;
    };
    if pid > 0 {
        return send_one(pid as usize, sig, bit);
    }
    if pid == 0 {
        let Some(pgid) = task::current_pgid() else {
            return false;
        };
        return kill_pg(pgid, sig);
    }
    // pid < 0 → process group -pid
    let pgid = (-pid) as usize;
    kill_pg(pgid, sig)
}

/// Deliver `sig` to every live user task in process group `pgid`.
pub fn kill_pg(pgid: usize, sig: u32) -> bool {
    let Some(bit) = sig_bit(sig) else {
        return false;
    };
    let mut any = false;
    for id in 0..task::task_slots() {
        if task::task_pgid(id) == Some(pgid) && send_one(id, sig, bit) {
            any = true;
        }
    }
    any
}

fn send_one(id: usize, sig: u32, bit: u32) -> bool {
    if !task::is_live_user(id) {
        return false;
    }
    // Discard if ignored — except SIGKILL, which cannot be ignored.
    if sig != SIGKILL && task::signal_is_ignored(id, bit) {
        return true;
    }
    task::signal_set_pending(id, bit);
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
    for id in 0..task::task_slots() {
        if task::task_has_ctty(id) {
            let _ = send_one(id, SIGINT, 1u32 << SIGINT);
        }
    }
}

/// If the current task has a default-fatal pending signal, exit with
/// `128 + sig` (shell convention). Call on paths back toward userspace and
/// when breaking out of `input::read`.
pub fn deliver_due() {
    let id = task::current_id();
    if let Some(sig) = task::signal_take_fatal(id) {
        task::user_exit(128u8.wrapping_add(sig as u8));
    }
}

/// `sigaction(sig, act, oact)` — phase-1: `SIG_DFL` / `SIG_IGN` only.
///
/// User pointers refer to a minimal `{handler: usize, flags: usize, mask: usize}`
/// (libgloss packs newlib `struct sigaction` into this). Handler `0` = DFL,
/// `1` = IGN; any other handler is rejected (`false` → ENOSYS in userspace).
/// `SIGKILL` cannot be ignored (DFL only).
pub fn sigaction(sig: u32, act: Option<usize>, oact: Option<usize>) -> bool {
    let Some(bit) = sig_bit(sig) else {
        return false;
    };
    let id = task::current_id();
    if !task::is_live_user(id) {
        return false;
    }

    if let Some(out) = oact {
        let handler = if task::signal_is_ignored(id, bit) {
            HANDLER_IGN
        } else {
            HANDLER_DFL
        };
        let words = [handler, 0usize, 0usize];
        let bytes = unsafe {
            core::slice::from_raw_parts(
                words.as_ptr() as *const u8,
                core::mem::size_of_val(&words),
            )
        };
        if !crate::user::copy_to_user(task::current_aspace(), out, bytes) {
            return false;
        }
    }

    if let Some(inp) = act {
        let mut words = [0usize; 3];
        let bytes = unsafe {
            core::slice::from_raw_parts_mut(
                words.as_mut_ptr() as *mut u8,
                core::mem::size_of_val(&words),
            )
        };
        if !crate::user::copy_from_user(task::current_aspace(), inp, bytes) {
            return false;
        }
        let handler = words[0];
        match handler {
            HANDLER_DFL => {
                task::signal_set_ignored(id, bit, false);
                // Drop a pending bit when resetting to default? Keep pending;
                // next deliver_due will terminate — fine for v1.
            }
            HANDLER_IGN => {
                if sig == SIGKILL {
                    return false;
                }
                task::signal_set_ignored(id, bit, true);
                task::signal_clear_pending(id, bit);
            }
            _ => {
                // Custom handlers deferred (no userspace trampoline yet).
                return false;
            }
        }
    }
    true
}
