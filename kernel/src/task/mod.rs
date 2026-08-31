//! Round-robin kernel threads plus user processes (own CR3/TTBR0).
//! Cooperative (`yield_now`) and preemptive (timer IRQ calls the same
//! `schedule` after EOI).

use alloc::alloc::{alloc, Layout};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use crate::console;
use crate::pipe;
use crate::user;

#[cfg(target_arch = "x86_64")]
mod switch_x86;
#[cfg(target_arch = "x86_64")]
use switch_x86::{seed_stack, task_switch};

#[cfg(target_arch = "aarch64")]
mod switch_aarch64;
#[cfg(target_arch = "aarch64")]
use switch_aarch64::{seed_stack, task_switch};

#[cfg(target_arch = "riscv64")]
mod switch_riscv64;
#[cfg(target_arch = "riscv64")]
use switch_riscv64::{seed_stack, task_switch};

const MAX_TASKS: usize = 8;
/// Exec from a syscall runs `load_user_elf` on the task stack (exception frame +
/// `[MAX_INIT_PAGES]`/`[USER_STACK_PAGES]` frame arrays). 8 KiB overflowed after
/// widening the user stack to 64 KiB (AArch64 CI hung in `dealloc`).
const STACK_SIZE: usize = 16 * 1024;
/// oksh `FDBASE` is 10 (`fcntl(F_DUPFD)` for tty/script fds). 8 was enough for
/// the tiny Rust shell; 16 leaves room for stdio + FDBASE + a pipe.
const MAX_FDS: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
enum FdEntry {
    Empty,
    Stdin,
    Console,
    File {
        node: crate::fs::Vnode,
        pos: usize,
    },
    PipeRead(usize),
    PipeWrite(usize),
}

fn default_user_fds() -> [FdEntry; MAX_FDS] {
    let mut fds = [FdEntry::Empty; MAX_FDS];
    fds[0] = FdEntry::Stdin;
    fds[1] = FdEntry::Console;
    fds[2] = FdEntry::Console;
    fds
}

fn fd_clone(entry: FdEntry) -> FdEntry {
    match entry {
        FdEntry::PipeRead(id) => {
            pipe::add_reader(id);
            FdEntry::PipeRead(id)
        }
        FdEntry::PipeWrite(id) => {
            pipe::add_writer(id);
            FdEntry::PipeWrite(id)
        }
        other => other,
    }
}

fn fd_drop(entry: FdEntry) {
    match entry {
        FdEntry::PipeRead(id) => pipe::drop_reader(id),
        FdEntry::PipeWrite(id) => pipe::drop_writer(id),
        _ => {}
    }
}

fn user_buf_ok(
    buf: usize,
    len: usize,
    user_base: usize,
    image_span: usize,
    stack_off: usize,
    brk: usize,
) -> bool {
    if len == 0 {
        return true;
    }
    let end = match buf.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    let stack = stack_off;
    let stack_bytes = crate::user::USER_STACK_PAGES * crate::user::PAGE;
    let in_code = buf >= user_base && end <= user_base + image_span;
    let in_stack = buf >= user_base + stack && end <= user_base + stack + stack_bytes;
    let heap_base = user_base + stack + stack_bytes;
    let in_heap = brk > heap_base && buf >= heap_base && end <= brk;
    in_code || in_stack || in_heap
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Unused,
    Ready,
    Running,
    Dead,
}

/// User register snapshot so a forked child can resume after the syscall
/// with the same callee-saved state the parent had (rax/x0 forced to 0).
#[derive(Clone, Copy)]
pub struct ForkRegs {
    pub rip: usize,
    pub rsp: usize,
    #[cfg(target_arch = "x86_64")]
    pub rbx: u64,
    #[cfg(target_arch = "x86_64")]
    pub rbp: u64,
    #[cfg(target_arch = "x86_64")]
    pub r12: u64,
    #[cfg(target_arch = "x86_64")]
    pub r13: u64,
    #[cfg(target_arch = "x86_64")]
    pub r14: u64,
    #[cfg(target_arch = "x86_64")]
    pub r15: u64,
    /// Full `lower_sync` frame (x0..x30, elr, spsr, sp_el0). Index 31 unused.
    #[cfg(target_arch = "aarch64")]
    pub frame: [u64; 36],
    /// Trap frame (x0..x31, sepc, sstatus, user sp). Index 35 unused.
    #[cfg(target_arch = "riscv64")]
    pub frame: [u64; 36],
}

#[derive(Clone, Copy)]
struct Task {
    state: State,
    #[allow(dead_code)]
    stack_base: usize,
    sp: usize,
    entry: Option<fn()>,
    aspace: u64,
    kernel_stack_top: usize,
    user_rip: usize,
    user_rsp: usize,
    fds: [FdEntry; MAX_FDS],
    user_base: u64,
    image_span: usize,
    stack_off: u64,
    ppid: usize,
    fork_regs: Option<ForkRegs>,
    user_argc: usize,
    user_argv: usize,
    /// Current program break (end of heap). 0 for kernel threads.
    brk_cur: u64,
    /// Basename from the last successful exec (multicall argv[0] fallback).
    exec_name: [u8; 32],
    exec_name_len: u8,
    exit_code: u8,
}

const EMPTY: Task = Task {
    state: State::Unused,
    stack_base: 0,
    sp: 0,
    entry: None,
    aspace: 0,
    kernel_stack_top: 0,
    user_rip: 0,
    user_rsp: 0,
    fds: [FdEntry::Empty; MAX_FDS],
    user_base: 0,
    image_span: 0,
    stack_off: 0,
    ppid: 0,
    fork_regs: None,
    user_argc: 0,
    user_argv: 0,
    brk_cur: 0,
    exec_name: [0; 32],
    exec_name_len: 0,
    exit_code: 0,
};

static TASKS: Mutex<[Task; MAX_TASKS]> = Mutex::new([EMPTY; MAX_TASKS]);
static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PREEMPT_ON: AtomicBool = AtomicBool::new(false);
static SERIAL: Mutex<()> = Mutex::new(());
static KERNEL_ASPACE: AtomicU64 = AtomicU64::new(0);
static LOADED_ASPACE: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    let a = user::read_aspace();
    KERNEL_ASPACE.store(a, Ordering::SeqCst);
    LOADED_ASPACE.store(a, Ordering::SeqCst);
    let flags = irq_save();
    irq_off();
    let mut tasks = TASKS.lock();
    tasks[0].state = State::Running;
    tasks[0].sp = 0;
    CURRENT.store(0, Ordering::SeqCst);
    drop(tasks);
    irq_restore(flags);
}

pub fn enable_preempt() {
    PREEMPT_ON.store(true, Ordering::SeqCst);
}

pub fn kernel_aspace() -> u64 {
    KERNEL_ASPACE.load(Ordering::SeqCst)
}

#[allow(dead_code)]
pub fn current_id() -> usize {
    CURRENT.load(Ordering::SeqCst)
}

/// When the running task is a user process, its saved PC and stack pointer.
pub fn current_user_pc_sp() -> Option<(usize, usize)> {
    let flags = irq_save();
    irq_off();
    let id = CURRENT.load(Ordering::SeqCst);
    let t = TASKS.lock()[id];
    let out = if t.user_rip != 0 {
        Some((t.user_rip, t.user_rsp))
    } else {
        None
    };
    irq_restore(flags);
    out
}

pub fn current_aspace() -> u64 {
    let flags = irq_save();
    irq_off();
    let id = CURRENT.load(Ordering::SeqCst);
    let a = TASKS.lock()[id].aspace;
    irq_restore(flags);
    a
}

/// Per-task user map: (USER_BASE, IMAGE_SPAN, STACK_OFF).
pub fn current_user_map() -> (u64, usize, u64) {
    let flags = irq_save();
    irq_off();
    let id = CURRENT.load(Ordering::SeqCst);
    let t = TASKS.lock()[id];
    let out = (t.user_base, t.image_span, t.stack_off);
    irq_restore(flags);
    out
}

pub fn current_brk() -> u64 {
    let flags = irq_save();
    irq_off();
    let id = CURRENT.load(Ordering::SeqCst);
    let b = TASKS.lock()[id].brk_cur;
    irq_restore(flags);
    b
}

pub fn set_brk(brk: u64) {
    with_current_mut(|t| t.brk_cur = brk);
}

pub fn set_exec_name(name: &[u8]) {
    with_current_mut(|t| {
        let n = name.len().min(t.exec_name.len());
        t.exec_name[..n].copy_from_slice(&name[..n]);
        t.exec_name_len = n as u8;
    });
}

pub fn exec_name(out: &mut [u8]) -> usize {
    let flags = irq_save();
    irq_off();
    let id = CURRENT.load(Ordering::SeqCst);
    let t = TASKS.lock()[id];
    let n = t.exec_name_len as usize;
    let n = n.min(out.len()).min(t.exec_name.len());
    out[..n].copy_from_slice(&t.exec_name[..n]);
    irq_restore(flags);
    n
}

fn heap_base_for(base: u64, stack_off: u64) -> u64 {
    base + stack_off + (crate::user::USER_STACK_PAGES * crate::user::PAGE) as u64
}

pub fn save_user_context(rip: usize, rsp: usize) {
    with_current_mut(|t| {
        t.user_rip = rip;
        t.user_rsp = rsp;
    });
}

fn with_current_mut<R>(f: impl FnOnce(&mut Task) -> R) -> R {
    // Timer schedule also takes TASKS; nesting that while IF=1 deadlocks the
    // same CPU (seen as a hang after fork child's open/read).
    let flags = irq_save();
    irq_off();
    let mut tasks = TASKS.lock();
    let id = CURRENT.load(Ordering::SeqCst);
    let out = f(&mut tasks[id]);
    drop(tasks);
    irq_restore(flags);
    out
}

pub fn fd_open(node: crate::fs::Vnode) -> Option<usize> {
    with_current_mut(|t| {
        for i in 0..MAX_FDS {
            if t.fds[i] == FdEntry::Empty {
                t.fds[i] = FdEntry::File { node, pos: 0 };
                return Some(i);
            }
        }
        None
    })
}

pub fn pipe_open() -> Option<(usize, usize)> {
    let id = pipe::alloc()?;
    let out = with_current_mut(|t| {
        let mut read_fd = None;
        let mut write_fd = None;
        for i in 0..MAX_FDS {
            if t.fds[i] == FdEntry::Empty {
                if read_fd.is_none() {
                    read_fd = Some(i);
                } else {
                    write_fd = Some(i);
                    break;
                }
            }
        }
        let (Some(r), Some(w)) = (read_fd, write_fd) else {
            return None;
        };
        pipe::add_reader(id);
        pipe::add_writer(id);
        t.fds[r] = FdEntry::PipeRead(id);
        t.fds[w] = FdEntry::PipeWrite(id);
        Some((r, w))
    });
    if out.is_none() {
        pipe::free(id);
    }
    out
}

pub fn fd_dup2(oldfd: usize, newfd: usize) -> bool {
    if oldfd >= MAX_FDS || newfd >= MAX_FDS {
        return false;
    }
    with_current_mut(|t| {
        let old = t.fds[oldfd];
        if old == FdEntry::Empty {
            return false;
        }
        if oldfd == newfd {
            return true;
        }
        fd_drop(t.fds[newfd]);
        t.fds[newfd] = fd_clone(old);
        true
    })
}

/// Read from fd 0 (keyboard + serial stdin). `buf` must lie in the user map.
pub fn fd_read_stdin(buf: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    if !user::buffer_ok(buf, len) {
        return usize::MAX;
    }
    let mut tmp = [0u8; 128];
    let want = len.min(tmp.len());
    // Do not call input::read (may yield) while TASKS is locked — deadlock.
    let n = crate::input::read(&mut tmp[..want]);
    let aspace = current_aspace();
    if !user::copy_to_user(aspace, buf, &tmp[..n]) {
        return usize::MAX;
    }
    n
}
