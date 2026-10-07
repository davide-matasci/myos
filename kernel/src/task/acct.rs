//! CPU-time accounting: how long each task ran and each CPU sat halted
//! (`/proc/<pid>/status`, `/proc/cpu`, `docs/proc.md`).
//!
//! A CPU charges the time since its last charge to its current task when
//! that task leaves it (`schedule`) and before it halts (`halt`); the halt
//! itself is charged to nobody, but counted as the CPU's idle time. So a
//! task's time is the time a CPU spent running it: a task halting in
//! `block_until` (it stays the CPU's current task) is charged only up to the
//! halt. Plain atomics per slot and per CPU: the scheduler charges without
//! taking TASKS.

use core::sync::atomic::{AtomicU64, Ordering};

use super::MAX_TASKS;
use crate::smp::MAX_CPUS;

/// Per slot: nanoseconds of CPU time.
static CPU_NS: [AtomicU64; MAX_TASKS] = [const { AtomicU64::new(0) }; MAX_TASKS];
/// Per slot: monotonic nanoseconds when its task started (spawn, fork, a
/// new thread), so a reused slot is told from the task it held before.
static START_NS: [AtomicU64; MAX_TASKS] = [const { AtomicU64::new(0) }; MAX_TASKS];
/// Per CPU: when the time since was last charged.
static SINCE: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// Per CPU: nanoseconds spent halted.
static IDLE_NS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

fn cpu() -> usize {
    crate::smp::cpu_id().min(MAX_CPUS - 1)
}

/// A new task in `slot`: no CPU time yet, started now.
pub(super) fn start(slot: usize) {
    CPU_NS[slot].store(0, Ordering::Relaxed);
    START_NS[slot].store(crate::time::monotonic_ns(), Ordering::Relaxed);
}

/// Per CPU: when its current halt began, 0 while it is not halted.
static HALT_FROM: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Charge this CPU's time since the last charge to `slot`, its current task.
/// Time it spent halted meanwhile is idle time, charged to nobody: a timer
/// interrupt that ends a halt in `block_until` may switch the halting task
/// away (`schedule` charges it here) before the halt returns.
pub(super) fn charge(slot: usize) {
    let c = cpu();
    if end_halt(c) {
        return;
    }
    let now = crate::time::monotonic_ns();
    let since = SINCE[c].swap(now, Ordering::Relaxed);
    if slot < MAX_TASKS && since != 0 {
        CPU_NS[slot].fetch_add(now.saturating_sub(since), Ordering::Relaxed);
    }
}

/// Count CPU `c`'s halt, if it is in one, as idle time up to now: true if
/// it was.
fn end_halt(c: usize) -> bool {
    let from = HALT_FROM[c].swap(0, Ordering::Relaxed);
    if from == 0 {
        return false;
    }
    let now = crate::time::monotonic_ns();
    IDLE_NS[c].fetch_add(now.saturating_sub(from), Ordering::Relaxed);
    SINCE[c].store(now, Ordering::Relaxed);
    true
}

/// Run `wait` (a halt until the next interrupt) as idle time: the current
/// task `slot` is charged up to it, nobody for it.
pub(super) fn idle(slot: usize, wait: impl FnOnce()) {
    charge(slot);
    HALT_FROM[cpu()].store(crate::time::monotonic_ns().max(1), Ordering::Relaxed);
    wait();
    // On the CPU the halt ends on: an interrupt that switched the task away
    // meanwhile has counted it already, on the CPU it began on.
    end_halt(cpu());
}

/// Nanoseconds of CPU time of `slot` so far, its running stretch included
/// only up to its CPU's last charge.
pub(super) fn cpu_ns(slot: usize) -> u64 {
    CPU_NS.get(slot).map_or(0, |n| n.load(Ordering::Relaxed))
}

/// Take `slot`'s CPU time out of it (an exiting thread hands it to its
/// process): what it runs from here on is charged to it again.
pub(super) fn take_cpu_ns(slot: usize) -> u64 {
    CPU_NS.get(slot).map_or(0, |n| n.swap(0, Ordering::Relaxed))
}

pub(super) fn start_ns(slot: usize) -> u64 {
    START_NS.get(slot).map_or(0, |n| n.load(Ordering::Relaxed))
}

/// Nanoseconds CPU `cpu` spent halted.
pub fn idle_ns(cpu: usize) -> u64 {
    IDLE_NS.get(cpu).map_or(0, |n| n.load(Ordering::Relaxed))
}
