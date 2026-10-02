//! User FP/SIMD register file across context switches.
//!
//! The kernel is built soft-float and never touches these registers, so
//! while a task is in the kernel the CPU still holds its user values. A user
//! task's registers are saved when it leaves a CPU and restored when it next
//! runs (on whichever CPU), so tasks preempted in the middle of FP/SIMD code
//! do not see each other's values. The Linux layer also uses [`save`] and
//! [`restore`] for signal frames.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use super::MAX_TASKS;

use crate::arch::fpu as arch;

pub use arch::{restore, save, BYTES};

#[repr(C, align(64))]
struct Buf(UnsafeCell<[u8; BYTES]>);
// Each slot is only touched by the CPU switching that task out or in, or by
// fork before the child can run.
unsafe impl Sync for Buf {}

static SAVED: [Buf; MAX_TASKS] = [const { Buf(UnsafeCell::new([0; BYTES])) }; MAX_TASKS];
/// `SAVED[slot]` holds the task's registers (else they are left as the CPU
/// has them: a fresh image starts from the state its entry path sets up).
static VALID: [AtomicBool; MAX_TASKS] = [const { AtomicBool::new(false) }; MAX_TASKS];

/// `prev` leaves this CPU (irqs off, before the stack switch).
pub(super) fn switch_out(prev: usize) {
    if arch::live() {
        unsafe { save(SAVED[prev].0.get().cast()) };
        VALID[prev].store(true, Ordering::Relaxed);
    }
}

/// `next` is about to run on this CPU (irqs off).
pub(super) fn switch_in(next: usize) {
    if VALID[next].load(Ordering::Relaxed) {
        unsafe { restore(SAVED[next].0.get().cast()) };
    }
}

/// A new image (spawn, exec) starts from its entry path's initial state.
pub(super) fn reset(slot: usize) {
    VALID[slot].store(false, Ordering::Relaxed);
}

/// A forked child starts with the parent's registers (runs on the parent's
/// CPU, in its syscall).
pub(super) fn fork(child: usize) {
    if arch::live() {
        unsafe { save(SAVED[child].0.get().cast()) };
        VALID[child].store(true, Ordering::Relaxed);
    } else {
        VALID[child].store(false, Ordering::Relaxed);
    }
}
