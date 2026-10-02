//! The user thread pointer (the TLS base) across context switches, kept per
//! task like the FP/SIMD registers (see `fpu`): the FS base on x86_64 and
//! `tpidr_el0` on aarch64. riscv64 keeps it in `tp`, a general register the
//! trap frame saves with the others, so there is nothing to switch there.

use core::sync::atomic::{AtomicU64, Ordering};

use super::MAX_TASKS;

#[cfg(target_arch = "x86_64")]
mod arch {
    use core::sync::atomic::{AtomicU64, Ordering};

    const IA32_FS_BASE: u32 = 0xC000_0100;

    /// The FS base each CPU has loaded, to skip redundant `wrmsr`s.
    static LOADED: [AtomicU64; crate::smp::MAX_CPUS] =
        [const { AtomicU64::new(0) }; crate::smp::MAX_CPUS];

    /// User code cannot change the FS base itself (no FSGSBASE), so the
    /// saved value is always the current one.
    pub fn read() -> Option<u64> {
        None
    }

    pub fn write(v: u64) {
        let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
        if LOADED[cpu].load(Ordering::Relaxed) == v {
            return;
        }
        unsafe {
            core::arch::asm!(
                "wrmsr",
                in("ecx") IA32_FS_BASE,
                in("eax") v as u32,
                in("edx") (v >> 32) as u32,
                options(nostack, preserves_flags),
            );
        }
        LOADED[cpu].store(v, Ordering::Relaxed);
    }
}

#[cfg(target_arch = "aarch64")]
mod arch {
    /// User code may write `tpidr_el0` directly.
    pub fn read() -> Option<u64> {
        let v: u64;
        unsafe { core::arch::asm!("mrs {}, tpidr_el0", out(reg) v, options(nomem, nostack)) };
        Some(v)
    }

    pub fn write(v: u64) {
        unsafe { core::arch::asm!("msr tpidr_el0, {}", in(reg) v, options(nostack)) };
    }
}

#[cfg(target_arch = "riscv64")]
mod arch {
    pub fn read() -> Option<u64> {
        None
    }

    pub fn write(_v: u64) {}
}

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
