//! The user thread pointer (the TLS base) across context switches, kept per
//! task like the FP/SIMD registers (see `fpu`): the FS base on x86_64 and
//! `tpidr_el0` on aarch64. riscv64 keeps it in `tp`, a general register the
//! trap frame saves with the others, so there is nothing to switch there.

use core::sync::atomic::{AtomicU64, Ordering};

use super::MAX_TASKS;

use crate::arch::tp as arch;

static SAVED: [AtomicU64; MAX_TASKS] = [const { AtomicU64::new(0) }; MAX_TASKS];

/// `prev` leaves this CPU (irqs off, before the stack switch).
pub(super) fn switch_out(prev: usize) {
    if let Some(v) = arch::read() {
        SAVED[prev].store(v, Ordering::Relaxed);
    }
}

/// `next` is about to run on this CPU (irqs off).
pub(super) fn switch_in(next: usize) {
    arch::write(SAVED[next].load(Ordering::Relaxed));
}

/// A task in `slot` starts with no thread pointer (spawn; for a new thread
/// its creator sets one with [`init`]).
pub(super) fn reset(slot: usize) {
    SAVED[slot].store(0, Ordering::Relaxed);
}

/// The new thread in `slot` starts with thread pointer `v`.
pub(super) fn init(slot: usize, v: u64) {
    SAVED[slot].store(v, Ordering::Relaxed);
}

/// A forked child starts with its parent thread's thread pointer (runs on
/// the parent's CPU, in its syscall).
pub(super) fn fork(parent: usize, child: usize) {
    let v = arch::read().unwrap_or(SAVED[parent].load(Ordering::Relaxed));
    SAVED[child].store(v, Ordering::Relaxed);
}

/// The running task's thread pointer (x86_64 / aarch64; riscv64 programs
/// read `tp` themselves).
pub fn get() -> u64 {
    let slot = super::current_id();
    arch::read().unwrap_or(SAVED[slot].load(Ordering::Relaxed))
}

/// Set the running task's thread pointer (exec clears it; the Linux
/// layer's `arch_prctl(ARCH_SET_FS)`).
pub fn set(v: u64) {
    SAVED[super::current_id()].store(v, Ordering::Relaxed);
    arch::write(v);
}
