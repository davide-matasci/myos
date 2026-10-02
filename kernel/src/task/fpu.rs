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

#[cfg(target_arch = "x86_64")]
mod arch {
/// The FXSAVE image (x87, MXCSR, xmm0-15).
pub const BYTES: usize = 512;

/// # Safety
/// `buf` is 16-byte aligned and `BYTES` long.
pub unsafe fn save(buf: *mut u8) {
    unsafe { core::arch::asm!("fxsave64 [{}]", in(reg) buf, options(nostack, preserves_flags)) };
}

/// # Safety
/// As [`save`]; `buf` holds an image whose MXCSR is valid for this CPU.
pub unsafe fn restore(buf: *const u8) {
    unsafe { core::arch::asm!("fxrstor64 [{}]", in(reg) buf, options(nostack, preserves_flags)) };
}


    /// Whether the outgoing task's registers need saving.
    pub fn live() -> bool {
        true
    }
}

#[cfg(target_arch = "aarch64")]
mod arch {
/// Laid out as the frame's `fpsimd_context`: head (magic, size), FPSR,
/// FPCR, then v0-v31.
pub const BYTES: usize = 528;

/// # Safety
/// `buf` is 16-byte aligned and `BYTES` long.
pub unsafe fn save(buf: *mut u8) {
    unsafe {
        core::arch::asm!(
            ".arch_extension fp",
            ".arch_extension simd",
            "stp q0, q1, [{p}, #16]",
            "stp q2, q3, [{p}, #48]",
            "stp q4, q5, [{p}, #80]",
            "stp q6, q7, [{p}, #112]",
            "stp q8, q9, [{p}, #144]",
            "stp q10, q11, [{p}, #176]",
            "stp q12, q13, [{p}, #208]",
            "stp q14, q15, [{p}, #240]",
            "stp q16, q17, [{p}, #272]",
            "stp q18, q19, [{p}, #304]",
            "stp q20, q21, [{p}, #336]",
            "stp q22, q23, [{p}, #368]",
            "stp q24, q25, [{p}, #400]",
            "stp q26, q27, [{p}, #432]",
            "stp q28, q29, [{p}, #464]",
            "stp q30, q31, [{p}, #496]",
            "mrs {t}, fpsr",
            "str {t:w}, [{p}, #8]",
            "mrs {t}, fpcr",
            "str {t:w}, [{p}, #12]",
            p = in(reg) buf,
            t = out(reg) _,
            options(nostack),
        );
    }
}

/// # Safety
/// As [`save`].
pub unsafe fn restore(buf: *const u8) {
    unsafe {
        core::arch::asm!(
            ".arch_extension fp",
            ".arch_extension simd",
            "ldp q0, q1, [{p}, #16]",
            "ldp q2, q3, [{p}, #48]",
            "ldp q4, q5, [{p}, #80]",
            "ldp q6, q7, [{p}, #112]",
            "ldp q8, q9, [{p}, #144]",
            "ldp q10, q11, [{p}, #176]",
            "ldp q12, q13, [{p}, #208]",
            "ldp q14, q15, [{p}, #240]",
            "ldp q16, q17, [{p}, #272]",
            "ldp q18, q19, [{p}, #304]",
            "ldp q20, q21, [{p}, #336]",
            "ldp q22, q23, [{p}, #368]",
            "ldp q24, q25, [{p}, #400]",
            "ldp q26, q27, [{p}, #432]",
            "ldp q28, q29, [{p}, #464]",
            "ldp q30, q31, [{p}, #496]",
            "ldr {t:w}, [{p}, #8]",
            "msr fpsr, {t}",
            "ldr {t:w}, [{p}, #12]",
            "msr fpcr, {t}",
            p = in(reg) buf,
            t = out(reg) _,
            options(nostack),
        );
    }
}


    pub fn live() -> bool {
        true
    }
}

#[cfg(target_arch = "riscv64")]
mod arch {
    /// `sstatus.FS = Initial`: FP instructions allowed.
    pub const SSTATUS_FS_INITIAL: u64 = 1 << 13;

/// f0-f31 then fcsr: the `__riscv_d_ext_state` layout.
pub const BYTES: usize = 264;

/// # Safety
/// `buf` is 8-byte aligned and `BYTES` long.
pub unsafe fn save(buf: *mut u8) {
    unsafe {
        core::arch::asm!(
            ".option push",
            ".option arch, +d",
            // The kernel runs with whatever FS the user had: make sure FP
            // instructions are allowed here.
            "csrs sstatus, {fs}",
            "fsd f0, 0({p})",
            "fsd f1, 8({p})",
            "fsd f2, 16({p})",
            "fsd f3, 24({p})",
            "fsd f4, 32({p})",
            "fsd f5, 40({p})",
            "fsd f6, 48({p})",
            "fsd f7, 56({p})",
            "fsd f8, 64({p})",
            "fsd f9, 72({p})",
            "fsd f10, 80({p})",
            "fsd f11, 88({p})",
            "fsd f12, 96({p})",
            "fsd f13, 104({p})",
            "fsd f14, 112({p})",
            "fsd f15, 120({p})",
            "fsd f16, 128({p})",
            "fsd f17, 136({p})",
            "fsd f18, 144({p})",
            "fsd f19, 152({p})",
            "fsd f20, 160({p})",
            "fsd f21, 168({p})",
            "fsd f22, 176({p})",
            "fsd f23, 184({p})",
            "fsd f24, 192({p})",
            "fsd f25, 200({p})",
            "fsd f26, 208({p})",
            "fsd f27, 216({p})",
            "fsd f28, 224({p})",
            "fsd f29, 232({p})",
            "fsd f30, 240({p})",
            "fsd f31, 248({p})",
            "frcsr {t}",
            "sw {t}, 256({p})",
            ".option pop",
            p = in(reg) buf,
            fs = in(reg) SSTATUS_FS_INITIAL,
            t = out(reg) _,
            options(nostack),
        );
    }
}

/// # Safety
/// As [`save`].
pub unsafe fn restore(buf: *const u8) {
    unsafe {
        core::arch::asm!(
            ".option push",
            ".option arch, +d",
            "csrs sstatus, {fs}",
            "fld f0, 0({p})",
            "fld f1, 8({p})",
            "fld f2, 16({p})",
            "fld f3, 24({p})",
            "fld f4, 32({p})",
            "fld f5, 40({p})",
            "fld f6, 48({p})",
            "fld f7, 56({p})",
            "fld f8, 64({p})",
            "fld f9, 72({p})",
            "fld f10, 80({p})",
            "fld f11, 88({p})",
            "fld f12, 96({p})",
            "fld f13, 104({p})",
            "fld f14, 112({p})",
            "fld f15, 120({p})",
            "fld f16, 128({p})",
            "fld f17, 136({p})",
            "fld f18, 144({p})",
            "fld f19, 152({p})",
            "fld f20, 160({p})",
            "fld f21, 168({p})",
            "fld f22, 176({p})",
            "fld f23, 184({p})",
            "fld f24, 192({p})",
            "fld f25, 200({p})",
            "fld f26, 208({p})",
            "fld f27, 216({p})",
            "fld f28, 224({p})",
            "fld f29, 232({p})",
            "fld f30, 240({p})",
            "fld f31, 248({p})",
            "lw {t}, 256({p})",
            "fscsr {t}",
            ".option pop",
            p = in(reg) buf,
            fs = in(reg) SSTATUS_FS_INITIAL,
            t = out(reg) _,
            options(nostack),
        );
    }
}


    /// Native riscv64 programs are soft-float and run with `sstatus.FS`
    /// off; only tasks that have the FPU on (Linux tasks) need saving.
    pub fn live() -> bool {
        let s: u64;
        unsafe { core::arch::asm!("csrr {}, sstatus", out(reg) s, options(nomem, nostack)) };
        s & (3 << 13) != 0
    }
}

pub use arch::{restore, save, BYTES};
#[cfg(all(target_arch = "riscv64", feature = "linux-compat"))]
pub use arch::SSTATUS_FS_INITIAL;

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
