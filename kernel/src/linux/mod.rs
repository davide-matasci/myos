//! Optional Linux syscall compatibility layer.
//!
//! Compiled only with the kernel's `linux-compat` feature (root package:
//! `cargo build --features linux_compat`); a default build has none of this
//! code. It does not change how native myos programs run: a process gets the
//! Linux personality only when the `linux` launcher starts it
//! (`SYS_LINUX_NEXT_EXEC`, then exec) or a process that already has it
//! `execve`s it.
//!
//! For such a process `syscall_dispatch` hands every syscall to [`dispatch`],
//! which decodes Linux numbers and arguments, calls into the native
//! implementation and returns `-errno` on failure. Scope today: static-PIE
//! musl binaries on x86_64, aarch64 and riscv64. See docs/linux-compat.md.
//!
//! State lives here, per task slot, outside `Task`; the core calls the
//! `on_*` hooks at spawn, fork, exec and context switch, and [`deliver`] to
//! run a caught signal's handler.
//!
//! Each arch module (`x86_64`, `aarch64`, `riscv64`) provides the same
//! items: `MACHINE`, `args`, `syscall` (its number table; aarch64 and
//! riscv64 share `generic`), `stat_bytes`, the signal frame (`deliver`,
//! `sigreturn`), the FP/SIMD register file (`FP_BYTES`, `fp_save`,
//! `fp_restore`), the thread pointer (`tls_read`, `tls_write`) and the
//! sigreturn trampoline code (`TRAMP_CODE`).

mod abi;
mod files;
mod signal;
mod sys;

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
mod generic;

#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
use x86_64 as arch;
#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "aarch64")]
use aarch64 as arch;
#[cfg(target_arch = "riscv64")]
mod riscv64;
#[cfg(target_arch = "riscv64")]
use riscv64 as arch;

pub use signal::deliver;
#[cfg(target_arch = "riscv64")]
pub use riscv64::SSTATUS_FS_INITIAL;

use core::cell::UnsafeCell;
use alloc::borrow::Cow;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::task::{self, MAX_TASKS};
use crate::user::{AuxV, SyscallRegs};

/// Per task slot: the task runs with the Linux personality.
static ACTIVE: [AtomicBool; MAX_TASKS] = [const { AtomicBool::new(false) }; MAX_TASKS];
/// Per task slot: the next successful exec starts a Linux image.
static PENDING: [AtomicBool; MAX_TASKS] = [const { AtomicBool::new(false) }; MAX_TASKS];
/// Per task slot: the thread pointer while the task is switched out
/// (`arch_prctl(ARCH_SET_FS)` on x86_64, `tpidr_el0` on aarch64; riscv64
/// keeps `tp` in the trap frame).
static TP: [AtomicU64; MAX_TASKS] = [const { AtomicU64::new(0) }; MAX_TASKS];

/// Per task slot: the FP/SIMD registers while the task is switched out. The
/// kernel itself never touches them (soft-float builds), so a Linux task's
/// registers are saved when it leaves the CPU and restored when it returns.
#[repr(C, align(64))]
struct FpBuf(UnsafeCell<[u8; arch::FP_BYTES]>);
// Each slot is only touched by the CPU switching that task in or out, or by
// fork before the child can run.
unsafe impl Sync for FpBuf {}
static FP: [FpBuf; MAX_TASKS] = [const { FpBuf(UnsafeCell::new([0; arch::FP_BYTES])) }; MAX_TASKS];
/// `FP[slot]` holds a saved state (else the CPU state is left as it is).
static FP_VALID: [AtomicBool; MAX_TASKS] = [const { AtomicBool::new(false) }; MAX_TASKS];

/// Whether the running task has the Linux personality.
pub fn active() -> bool {
    ACTIVE[task::current_id()].load(Ordering::Relaxed)
}

/// A Linux syscall from the running task (see the module docs).
pub fn dispatch(
    nr: usize,
    a0: usize,
    a1: usize,
    a2: usize,
    regs: &mut SyscallRegs,
    user_rip: usize,
    user_rsp: usize,
) -> usize {
    let args = arch::args(regs, a0, a1, a2);
    arch::syscall(nr, args, regs, user_rip, user_rsp)
}

/// Native `SYS_LINUX_NEXT_EXEC`: the next successful exec of the caller
/// starts a Linux image. A failed exec leaves it set; the launcher exits then.
pub fn sys_linux_next_exec() -> usize {
    PENDING[task::current_id()].store(true, Ordering::Relaxed);
    0
}

/// Exec with the Linux personality from a Linux process (`execve`).
fn exec_linux(path: &str, args: &[&[u8]], env: &[&[u8]]) -> usize {
    let slot = task::current_id();
    PENDING[slot].store(true, Ordering::Relaxed);
    let ret = crate::user::exec_path(path, args, env);
    PENDING[slot].store(false, Ordering::Relaxed);
    ret
}

/// Hook: a successful exec replaced the current image.
pub fn on_exec(slot: usize) {
    let linux = PENDING[slot].swap(false, Ordering::Relaxed);
    ACTIVE[slot].store(linux, Ordering::Relaxed);
    TP[slot].store(0, Ordering::Relaxed);
    FP_VALID[slot].store(false, Ordering::Relaxed);
    if linux {
        arch::tls_write(0);
    }
    signal::on_exec(slot);
}

/// Hook: `child` was forked from `parent` (TASKS held, irqs off). Runs on
/// the parent's CPU, so its live thread pointer and FP registers are the
/// parent's.
pub fn on_fork(parent: usize, child: usize) {
    let linux = ACTIVE[parent].load(Ordering::Relaxed);
    ACTIVE[child].store(linux, Ordering::Relaxed);
    PENDING[child].store(false, Ordering::Relaxed);
    let tp = arch::tls_read().unwrap_or(TP[parent].load(Ordering::Relaxed));
    TP[child].store(tp, Ordering::Relaxed);
    FP_VALID[child].store(linux, Ordering::Relaxed);
    if linux {
        unsafe { arch::fp_save(FP[child].0.get().cast()) };
    }
    files::on_fork(parent, child);
    signal::on_fork(parent, child);
}

/// Hook: `slot` was (re)used for a freshly spawned task.
pub fn on_spawn(slot: usize) {
    ACTIVE[slot].store(false, Ordering::Relaxed);
    PENDING[slot].store(false, Ordering::Relaxed);
    TP[slot].store(0, Ordering::Relaxed);
    FP_VALID[slot].store(false, Ordering::Relaxed);
    files::on_spawn(slot);
    signal::on_exec(slot);
}

/// Hook: this CPU switches from task `prev` to task `next` (irqs off).
pub fn on_switch(prev: usize, next: usize) {
    if ACTIVE[prev].load(Ordering::Relaxed) {
        if let Some(tp) = arch::tls_read() {
            TP[prev].store(tp, Ordering::Relaxed);
        }
        unsafe { arch::fp_save(FP[prev].0.get().cast()) };
        FP_VALID[prev].store(true, Ordering::Relaxed);
    }
    if ACTIVE[next].load(Ordering::Relaxed) {
        arch::tls_write(TP[next].load(Ordering::Relaxed));
        if FP_VALID[next].load(Ordering::Relaxed) {
            unsafe { arch::fp_restore(FP[next].0.get().cast()) };
        }
    }
}

/// The running task's thread pointer changed (`arch_prctl`).
#[cfg(target_arch = "x86_64")]
fn set_tp(v: u64) {
    TP[task::current_id()].store(v, Ordering::Relaxed);
    arch::tls_write(v);
}

/// The auxiliary vector for an image about to start with the Linux
/// personality (empty for native images): musl's startup (static-PIE, or the
/// dynamic linker for a dynamic program) finds the program headers
/// (PT_DYNAMIC, PT_TLS) through `AT_PHDR`, and the dynamic linker its own
/// load address through `AT_BASE`.
pub fn exec_auxv(elf: &[u8], base: u64, entry: usize, interp_base: Option<usize>) -> AuxV {
    let mut aux = AuxV::new();
    if !PENDING[task::current_id()].load(Ordering::Relaxed) {
        return aux;
    }
    let u16_at = |o: usize| elf.get(o..o + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as u64);
    let u32_at = |o: usize| elf.get(o..o + 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()) as u64);
    let u64_at = |o: usize| elf.get(o..o + 8).map_or(0, |b| u64::from_le_bytes(b.try_into().unwrap()));
    let phoff = u64_at(0x20);
    let phent = u16_at(0x36);
    let phnum = u16_at(0x38);
    let Ok(span) = crate::modules::elf::image_span(elf) else {
        return aux;
    };
    let bias = base.wrapping_sub(span.min_vaddr);
    // Where the program headers sit in memory: PT_PHDR, else the PT_LOAD
    // whose file range covers them.
    let mut phdr = 0u64;
    for i in 0..phnum {
        let p = (phoff + i * phent) as usize;
        let (ty, off, va, filesz) = (u32_at(p), u64_at(p + 8), u64_at(p + 16), u64_at(p + 32));
        if ty == 6 {
            phdr = va;
            break;
        }
        if ty == 1 && off <= phoff && phoff < off + filesz {
            phdr = va + (phoff - off);
        }
    }
    const AT_PHDR: usize = 3;
    const AT_PHENT: usize = 4;
    const AT_PHNUM: usize = 5;
    const AT_PAGESZ: usize = 6;
    const AT_ENTRY: usize = 9;
    const AT_CLKTCK: usize = 17;
    aux.push(AT_PHDR, (bias + phdr) as usize);
    aux.push(AT_PHENT, phent as usize);
    aux.push(AT_PHNUM, phnum as usize);
    aux.push(AT_PAGESZ, crate::user::PAGE);
    aux.push(AT_ENTRY, entry);
    aux.push(AT_CLKTCK, 100);
    if let Some(b) = interp_base {
        const AT_BASE: usize = 7;
        aux.push(AT_BASE, b);
    }
    aux
}

/// For a Linux exec of a dynamically linked program: the bytes of its
/// interpreter (`PT_INTERP`, e.g. `/lib/ld-musl-x86_64.so.1`), which the
/// core maps next to it and starts instead. `Ok(None)` for a static image
/// or a native exec; `Err` if the interpreter cannot be read.
pub fn exec_interp(elf: &[u8]) -> Result<Option<Cow<'static, [u8]>>, ()> {
    if !PENDING[task::current_id()].load(Ordering::Relaxed) {
        return Ok(None);
    }
    let Some(path) = crate::modules::elf::interp_path(elf) else {
        return Ok(None);
    };
    let path = core::str::from_utf8(path).map_err(|_| ())?;
    let real = crate::user::resolve_copied_path(path).ok_or(())?;
    if let Some(b) = crate::fs::lookup(&real) {
        return Ok(Some(Cow::Borrowed(b)));
    }
    const INTERP_MAX: usize = 16 << 20;
    crate::fs::read_all(&real, INTERP_MAX).map(|v| Some(Cow::Owned(v))).ok_or(())
}
