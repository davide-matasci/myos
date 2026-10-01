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
//! musl binaries on x86_64 (other arches do not build with the feature).
//! See docs/linux-compat.md.
//!
//! State lives here, per task slot, outside `Task`; the core calls the
//! `on_*` hooks at spawn, fork, exec and context switch.

#[cfg(not(target_arch = "x86_64"))]
compile_error!("the linux-compat feature supports x86_64 only so far");

mod abi;
mod files;
mod sys;
mod x86_64;

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::task::{self, MAX_TASKS};
use crate::user::{AuxV, SyscallRegs};

/// Per task slot: the task runs with the Linux personality.
static ACTIVE: [AtomicBool; MAX_TASKS] = [const { AtomicBool::new(false) }; MAX_TASKS];
/// Per task slot: the next successful exec starts a Linux image.
static PENDING: [AtomicBool; MAX_TASKS] = [const { AtomicBool::new(false) }; MAX_TASKS];
/// Per task slot: the thread pointer (`arch_prctl(ARCH_SET_FS)` on x86_64).
static TP: [AtomicU64; MAX_TASKS] = [const { AtomicU64::new(0) }; MAX_TASKS];

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
    regs: &SyscallRegs,
    user_rip: usize,
    user_rsp: usize,
) -> usize {
    let [a3, a4, a5] = regs.args_3_to_5();
    x86_64::dispatch(nr, [a0, a1, a2, a3, a4, a5], user_rip, user_rsp)
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
    x86_64::load_fs_base(0);
}

/// Hook: `child` was forked from `parent` (TASKS held, irqs off).
pub fn on_fork(parent: usize, child: usize) {
    ACTIVE[child].store(ACTIVE[parent].load(Ordering::Relaxed), Ordering::Relaxed);
    PENDING[child].store(false, Ordering::Relaxed);
    TP[child].store(TP[parent].load(Ordering::Relaxed), Ordering::Relaxed);
    files::on_fork(parent, child);
}

/// Hook: `slot` was (re)used for a freshly spawned task.
pub fn on_spawn(slot: usize) {
    ACTIVE[slot].store(false, Ordering::Relaxed);
    PENDING[slot].store(false, Ordering::Relaxed);
    TP[slot].store(0, Ordering::Relaxed);
    files::on_spawn(slot);
}

/// Hook: this CPU is about to run `slot`.
pub fn on_switch(slot: usize) {
    x86_64::load_fs_base(TP[slot].load(Ordering::Relaxed));
}

fn set_tp(v: u64) {
    TP[task::current_id()].store(v, Ordering::Relaxed);
    x86_64::load_fs_base(v);
}

/// The auxiliary vector for an image about to start with the Linux
/// personality (empty for native images): musl's static-PIE startup finds
/// its program headers (PT_DYNAMIC, PT_TLS) through `AT_PHDR`.
pub fn exec_auxv(elf: &[u8], base: u64, entry: usize) -> AuxV {
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
    aux
}
