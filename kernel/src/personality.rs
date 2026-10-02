//! Foreign syscall personalities: a module (the Linux layer, `modules/linux`)
//! registers a [`PersonalityOps`] table; tasks that exec'd with it make its
//! syscalls instead of the native ones and get its signal frames.
//!
//! The kernel keeps only which task slots carry the personality and which
//! have one pending for their next exec (`SYS_LINUX_NEXT_EXEC`, or an exec
//! from a task that already has it); everything else is the module's. A
//! process gets the personality only through that path: native programs are
//! never affected, and without a registered personality the request fails.

use core::sync::atomic::{AtomicBool, Ordering};
use myos_abi::{PERSONALITY_FPU_ON, PersonalityOps, SignalDelivery};
use spin::Once;

use crate::task::{self, MAX_TASKS, SigAction};
use crate::user::SyscallRegs;

static OPS: Once<PersonalityOps> = Once::new();
/// Per task slot: the task runs with the foreign personality.
static ACTIVE: [AtomicBool; MAX_TASKS] = [const { AtomicBool::new(false) }; MAX_TASKS];
/// Per task slot: the next successful exec starts an image with it.
static PENDING: [AtomicBool; MAX_TASKS] = [const { AtomicBool::new(false) }; MAX_TASKS];

/// A module installs the personality (one per boot).
pub fn register(ops: PersonalityOps) -> bool {
    if OPS.get().is_some() {
        return false;
    }
    OPS.call_once(|| ops);
    true
}

/// The running task has the foreign personality.
pub fn active() -> bool {
    ACTIVE[task::current_id()].load(Ordering::Relaxed)
}

/// The running task's image runs with the FPU on because of its personality
/// (riscv64: `sstatus.FS`).
#[allow(dead_code)]
pub fn fpu_on() -> bool {
    active() && OPS.get().is_some_and(|o| o.flags & PERSONALITY_FPU_ON != 0)
}

/// The running task's next exec starts an image with the personality.
pub fn pending() -> bool {
    PENDING[task::current_id()].load(Ordering::Relaxed)
}

/// `SYS_LINUX_NEXT_EXEC`: the caller's next successful exec starts an image
/// with the personality. Fails when none is registered (the launcher then
/// exits with an error instead of running a Linux binary natively).
pub fn request_next_exec() -> bool {
    if OPS.get().is_none() {
        return false;
    }
    PENDING[task::current_id()].store(true, Ordering::Relaxed);
    true
}

/// Exec from a task that already has the personality (`execve`): the new
/// image keeps it. A failed exec clears the request.
pub fn exec_with(path: &str, args: &[&[u8]], env: &[&[u8]]) -> usize {
    let slot = task::current_id();
    PENDING[slot].store(true, Ordering::Relaxed);
    let ret = crate::user::exec_path(path, args, env);
    PENDING[slot].store(false, Ordering::Relaxed);
    ret
}

/// A syscall of the running task, if it has the personality.
pub fn dispatch(nr: usize, a0: usize, a1: usize, a2: usize, regs: &mut SyscallRegs) -> Option<usize> {
    if !active() {
        return None;
    }
    let ops = OPS.get()?;
    Some(unsafe { (ops.syscall)(nr, a0, a1, a2, regs.as_ptr()) })
}

/// Hook: a successful exec replaced `slot`'s image.
pub fn on_exec(slot: usize) {
    let foreign = PENDING[slot].swap(false, Ordering::Relaxed);
    ACTIVE[slot].store(foreign, Ordering::Relaxed);
    if let Some(ops) = OPS.get() {
        unsafe { (ops.on_exec)(slot) };
    }
}

/// Hook: `child` was forked from `parent` (TASKS held, irqs off).
pub fn on_fork(parent: usize, child: usize) {
    ACTIVE[child].store(ACTIVE[parent].load(Ordering::Relaxed), Ordering::Relaxed);
    PENDING[child].store(false, Ordering::Relaxed);
    if let Some(ops) = OPS.get() {
        unsafe { (ops.on_fork)(parent, child) };
    }
}

/// Hook: thread `creator` started thread `slot` (TASKS held, irqs off).
pub fn on_thread(creator: usize, slot: usize) {
    ACTIVE[slot].store(ACTIVE[creator].load(Ordering::Relaxed), Ordering::Relaxed);
    PENDING[slot].store(false, Ordering::Relaxed);
    if let Some(ops) = OPS.get() {
        unsafe { (ops.on_thread)(creator, slot) };
    }
}

/// Hook: `slot` was (re)used for a freshly spawned task.
pub fn on_spawn(slot: usize) {
    ACTIVE[slot].store(false, Ordering::Relaxed);
    PENDING[slot].store(false, Ordering::Relaxed);
    if let Some(ops) = OPS.get() {
        unsafe { (ops.on_spawn)(slot) };
    }
}

/// How a caught signal is delivered to the running task.
pub enum Deliver {
    /// Not a foreign task: use the native frame.
    Native,
    /// The personality built its frame; the result-register value.
    Done(usize),
    /// The frame did not fit: kill the task (SIGSEGV).
    Failed,
}

pub fn deliver(
    regs: &mut SyscallRegs,
    sig: u32,
    act: &SigAction,
    tramp: usize,
    restore_mask: u32,
    pc: usize,
    ret: usize,
) -> Deliver {
    if !active() {
        return Deliver::Native;
    }
    let Some(ops) = OPS.get() else {
        return Deliver::Native;
    };
    let d = SignalDelivery {
        sig,
        flags: act.flags,
        handler: act.handler,
        tramp,
        restore_mask,
        pc,
        ret,
        arch: crate::arch::signal_arch_word(),
    };
    let mut out = 0usize;
    if unsafe { (ops.deliver)(regs.as_ptr(), &d, &mut out) } < 0 {
        Deliver::Failed
    } else {
        Deliver::Done(out)
    }
}
