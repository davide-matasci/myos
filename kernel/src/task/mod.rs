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
const MAX_FDS: usize = 8;

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
    unsafe {
        core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf as *mut u8, n);
    }
    n
}

pub fn fd_read(fd: usize, buf: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    loop {
        let (entry, map) = {
            let flags = irq_save();
            irq_off();
            let id = CURRENT.load(Ordering::SeqCst);
            let t = TASKS.lock()[id];
            irq_restore(flags);
            (
                t.fds.get(fd).copied().unwrap_or(FdEntry::Empty),
                (
                    t.user_base,
                    t.image_span,
                    t.stack_off,
                    t.brk_cur as usize,
                ),
            )
        };
        let (user_base, image_span, stack_off, brk) = map;
        let user_base = user_base as usize;
        let stack_off = stack_off as usize;
        if !user_buf_ok(buf, len.min(128), user_base, image_span, stack_off, brk) {
            return usize::MAX;
        }
        match entry {
            FdEntry::Stdin => return fd_read_stdin(buf, len),
            FdEntry::File { node, pos: _ } => {
                return with_current_mut(|t| {
                    let FdEntry::File { node, pos } = t.fds[fd] else {
                        return usize::MAX;
                    };
                    let mut tmp = [0u8; 128];
                    let want = len.min(tmp.len());
                    let n = crate::fs::read(&node, pos, &mut tmp[..want]);
                    if n != 0 {
                        if !user_buf_ok(
                            buf,
                            n,
                            t.user_base as usize,
                            t.image_span,
                            t.stack_off as usize,
                            t.brk_cur as usize,
                        ) {
                            return usize::MAX;
                        }
                        unsafe {
                            core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf as *mut u8, n);
                        }
                    }
                    if let FdEntry::File { pos: p, .. } = &mut t.fds[fd] {
                        *p += n;
                    }
                    n
                });
            }
            FdEntry::PipeRead(id) => {
                let mut tmp = [0u8; 128];
                let want = len.min(tmp.len());
                let n = pipe::read(id, &mut tmp[..want]);
                if n == usize::MAX {
                    return usize::MAX;
                }
                if n == 0 && pipe::read_would_block(id) {
                    yield_now();
                    continue;
                }
                unsafe {
                    core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf as *mut u8, n);
                }
                return n;
            }
            FdEntry::Empty | FdEntry::Console | FdEntry::PipeWrite(_) => return usize::MAX,
        }
    }
}

pub fn fd_write(fd: usize, buf: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let mut total = 0usize;
    while total < len {
        let chunk = (len - total).min(128);
        let (entry, map) = {
            let flags = irq_save();
            irq_off();
            let id = CURRENT.load(Ordering::SeqCst);
            let t = TASKS.lock()[id];
            irq_restore(flags);
            (
                t.fds.get(fd).copied().unwrap_or(FdEntry::Empty),
                (
                    t.user_base,
                    t.image_span,
                    t.stack_off,
                    t.brk_cur as usize,
                ),
            )
        };
        let (user_base, image_span, stack_off, brk) = map;
        if !user_buf_ok(
            buf + total,
            chunk,
            user_base as usize,
            image_span,
            stack_off as usize,
            brk,
        ) {
            return if total == 0 { usize::MAX } else { total };
        }
        let mut tmp = [0u8; 128];
        unsafe {
            core::ptr::copy_nonoverlapping((buf + total) as *const u8, tmp.as_mut_ptr(), chunk);
        }
        loop {
            match entry {
                FdEntry::Console => {
                    print_bytes(&tmp[..chunk]);
                    total += chunk;
                    break;
                }
                FdEntry::PipeWrite(id) => {
                    let n = pipe::write(id, &tmp[..chunk]);
                    if n == usize::MAX {
                        return if total == 0 { usize::MAX } else { total };
                    }
                    if n == 0 && pipe::write_would_block(id) {
                        yield_now();
                        continue;
                    }
                    total += n;
                    break;
                }
                _ => return if total == 0 { usize::MAX } else { total },
            }
        }
    }
    total
}

pub fn fd_close(fd: usize) -> bool {
    if fd >= MAX_FDS {
        return false;
    }
    with_current_mut(|t| {
        let entry = t.fds[fd];
        if entry == FdEntry::Empty {
            return false;
        }
        fd_drop(entry);
        t.fds[fd] = if fd == 0 {
            FdEntry::Stdin
        } else if fd == 1 || fd == 2 {
            FdEntry::Console
        } else {
            FdEntry::Empty
        };
        true
    })
}

/// In-place exec: replace the current task's user image. Does not spawn,
/// does not bump USERS_ALIVE, does not note_exit. Keeps the fd table so
/// shell redirects and pipes survive exec.
pub fn replace_user(
    aspace: u64,
    user_rip: usize,
    user_rsp: usize,
    user_base: u64,
    image_span: usize,
    stack_off: u64,
    user_argc: usize,
    user_argv: usize,
) {
    with_current_mut(|t| {
        t.aspace = aspace;
        t.user_rip = user_rip;
        t.user_rsp = user_rsp;
        t.user_base = user_base;
        t.image_span = image_span;
        t.stack_off = stack_off;
        t.user_argc = user_argc;
        t.user_argv = user_argv;
        t.fork_regs = None;
        t.brk_cur = heap_base_for(user_base, stack_off);
    });
    user::switch_aspace(aspace);
    LOADED_ASPACE.store(aspace, Ordering::SeqCst);
}

pub fn spawn(entry: fn()) {
    spawn_inner(
        0,
        Some(entry),
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        [FdEntry::Empty; MAX_FDS],
        None,
    );
}

pub fn spawn_user(
    aspace: u64,
    user_rip: usize,
    user_rsp: usize,
    user_base: u64,
    image_span: usize,
    stack_off: u64,
    user_argc: usize,
    user_argv: usize,
) {
    spawn_inner(
        aspace,
        None,
        user_rip,
        user_rsp,
        user_base,
        image_span,
        stack_off,
        user_argc,
        user_argv,
        0,
        default_user_fds(),
        None,
    );
}

/// Copy the current user task: new aspace, copied fds, fork resume regs.
/// Child is Ready and will resume userspace with rax/x0 = 0. Returns child slot.
pub fn fork_current(child_regs: ForkRegs) -> Option<usize> {
    let flags = irq_save();
    irq_off();

    let (fds, base, span, off, ppid, uargc, uargv, brk) = {
        let tasks = TASKS.lock();
        let id = CURRENT.load(Ordering::SeqCst);
        let t = tasks[id];
        if t.user_rip == 0 {
            drop(tasks);
            irq_restore(flags);
            return None;
        }
        (
            t.fds,
            t.user_base,
            t.image_span,
            t.stack_off,
            id,
            t.user_argc,
            t.user_argv,
            t.brk_cur,
        )
    };

    let Some(aspace) = user::copy_user_aspace(base, span, off, brk) else {
        irq_restore(flags);
        return None;
    };

    let layout = match Layout::from_size_align(STACK_SIZE, 16) {
        Ok(l) => l,
        Err(_) => {
            irq_restore(flags);
            return None;
        }
    };

    let (slot, reuse_stack) = {
        let tasks = TASKS.lock();
        // Reuse kernel stacks left behind by reaped fork children (stack_base kept
        // in EMPTY slots) or dead kernel threads — avoids kernel-heap alloc on CI.
        let slot = tasks
            .iter()
            .position(|t| {
                (t.state == State::Unused || t.state == State::Dead)
                    && t.user_rip == 0
                    && t.aspace == 0
                    && t.stack_base != 0
            })
            .or_else(|| tasks.iter().position(|t| t.state == State::Unused));
        let Some(slot) = slot else {
            drop(tasks);
            irq_restore(flags);
            return None;
        };
        let reuse = tasks[slot].stack_base != 0;
        let stack_base = tasks[slot].stack_base;
        drop(tasks);
        (slot, (reuse, stack_base))
    };

    let mut child_fds = default_user_fds();
    for i in 0..MAX_FDS {
        child_fds[i] = fd_clone(fds[i]);
    }

    let (stack_base, sp, top) = if reuse_stack.0 {
        let sb = reuse_stack.1;
        let sp = unsafe { seed_stack(sb as *mut u8, STACK_SIZE, trampoline as *const () as usize) };
        (sb, sp, sb + STACK_SIZE)
    } else {
        let stack = unsafe { alloc(layout) };
        if stack.is_null() {
            irq_restore(flags);
            return None;
        }
        let sb = stack as usize;
        let sp = unsafe { seed_stack(stack, STACK_SIZE, trampoline as *const () as usize) };
        (sb, sp, sb + STACK_SIZE)
    };

    let mut tasks = TASKS.lock();
    tasks[slot] = Task {
        state: State::Ready,
        stack_base,
        sp,
        entry: None,
        aspace,
        kernel_stack_top: top,
        user_rip: child_regs.rip,
        user_rsp: child_regs.rsp,
        fds: child_fds,
        user_base: base,
        image_span: span,
        stack_off: off,
        ppid,
        fork_regs: Some(child_regs),
        user_argc: uargc,
        user_argv: uargv,
        brk_cur: brk,
        exec_name: [0; 32],
        exec_name_len: 0,
        exit_code: 0,
    };
    drop(tasks);
    user::note_fork();
    irq_restore(flags);
    Some(slot)
}

/// Yield until a child has exited, reap it, return its pid.
/// `usize::MAX` if this task has no children. If `status_out` is non-null,
/// stores the low 8 bits of the child's exit code.
pub fn wait_child(status_out: *mut u8) -> usize {
    let parent = CURRENT.load(Ordering::SeqCst);
    loop {
        let mut any = false;
        let mut reap = None;
        {
            let flags = irq_save();
            irq_off();
            let mut tasks = TASKS.lock();
            for i in 0..MAX_TASKS {
                if i == parent
                    || tasks[i].ppid != parent
                    || tasks[i].user_rip == 0
                    || tasks[i].state == State::Unused
                {
                    continue;
                }
                if tasks[i].state == State::Dead {
                    reap = Some(i);
                    break;
                }
                any = true;
            }
            if let Some(i) = reap {
                let stack_base = tasks[i].stack_base;
                let code = tasks[i].exit_code;
                tasks[i] = EMPTY;
                if stack_base != 0 {
                    tasks[i].stack_base = stack_base;
                }
                drop(tasks);
                irq_restore(flags);
                if !status_out.is_null() {
                    unsafe {
                        *status_out = code;
                    }
                }
                return i;
            }
            drop(tasks);
            irq_restore(flags);
        }
        if !any {
            return usize::MAX;
        }
        yield_now();
    }
}

fn spawn_inner(
    aspace: u64,
    entry: Option<fn()>,
    user_rip: usize,
    user_rsp: usize,
    user_base: u64,
    image_span: usize,
    stack_off: u64,
    user_argc: usize,
    user_argv: usize,
    ppid: usize,
    fds: [FdEntry; MAX_FDS],
    fork_regs: Option<ForkRegs>,
) {
    let flags = irq_save();
    irq_off();
    let layout = Layout::from_size_align(STACK_SIZE, 16).expect("task stack layout");
    let stack = unsafe { alloc(layout) };
    assert!(!stack.is_null(), "task stack alloc");
    let sp = unsafe { seed_stack(stack, STACK_SIZE, trampoline as usize) };
    let top = stack as usize + STACK_SIZE;

    let mut tasks = TASKS.lock();
    let slot = tasks
        .iter()
        .position(|t| t.state == State::Unused)
        .expect("no task slot");
    let brk_cur = if user_rip != 0 {
        heap_base_for(user_base, stack_off)
    } else {
        0
    };
    tasks[slot] = Task {
        state: State::Ready,
        stack_base: stack as usize,
        sp,
        entry,
        aspace,
        kernel_stack_top: top,
        user_rip,
        user_rsp,
        fds,
        user_base,
        image_span,
        stack_off,
        ppid,
        fork_regs,
        user_argc,
        user_argv,
        brk_cur,
        exec_name: [0; 32],
        exec_name_len: 0,
        exit_code: 0,
    };
    drop(tasks);
    irq_restore(flags);
}

pub fn user_exit(code: u8) -> ! {
    with_current_mut(|t| t.exit_code = code);
    die();
}

pub fn yield_now() {
    // Must restore the caller's IF. Unconditional sti broke syscalls
    // (syscall_entry runs with cli): wait_child then locked TASKS with
    // IF=1 and the timer nested into schedule → same-CPU spin deadlock.
    let flags = irq_save();
    irq_off();
    schedule();
    irq_restore(flags);
}

/// Same switch as `yield_now`. No-op until `enable_preempt` so the first-tick
/// IRQ proof does not leave the Limine stack.
pub fn schedule() {
    if !PREEMPT_ON.load(Ordering::SeqCst) {
        return;
    }

    // Hold IF off across TASKS + switch. Caller may already have IF clear
    // (timer, yield); save/restore so we never leave IF on while locked.
    let flags = irq_save();
    irq_off();

    let switch = {
        let mut tasks = TASKS.lock();
        let current = CURRENT.load(Ordering::SeqCst);
        match tasks[current].state {
            State::Running => tasks[current].state = State::Ready,
            State::Dead | State::Ready | State::Unused => {}
        }

        let mut next = current;
        for off in 1..MAX_TASKS {
            let i = (current + off) % MAX_TASKS;
            if tasks[i].state == State::Ready {
                next = i;
                break;
            }
        }

        if next == current {
            if tasks[current].state == State::Ready {
                tasks[current].state = State::Running;
            }
            None
        } else {
            tasks[next].state = State::Running;
            let old_sp = core::ptr::addr_of_mut!(tasks[current].sp);
            let new_sp = tasks[next].sp;
            let kstack = tasks[next].kernel_stack_top;
            let aspace = tasks[next].aspace;
            CURRENT.store(next, Ordering::SeqCst);
            Some((old_sp, new_sp, kstack, aspace))
        }
    };

    let Some((old_sp, new_sp, kstack, aspace)) = switch else {
        irq_restore(flags);
        return;
    };

    if kstack != 0 {
        user::set_kernel_rsp0(kstack);
        #[cfg(target_arch = "x86_64")]
        crate::arch::gdt::set_rsp0(kstack as u64);
    }

    let want = if aspace == 0 {
        KERNEL_ASPACE.load(Ordering::SeqCst)
    } else {
        aspace
    };
    if want != LOADED_ASPACE.load(Ordering::SeqCst) {
        user::switch_aspace(want);
        LOADED_ASPACE.store(want, Ordering::SeqCst);
    }

    unsafe {
        task_switch(old_sp, new_sp);
    }
    irq_restore(flags);
}

pub fn print(s: &str) {
    let flags = irq_save();
    irq_off();
    {
        let _hold = SERIAL.lock();
        console::write_str(s);
    }
    irq_restore(flags);
}

pub fn print_bytes(bytes: &[u8]) {
    match core::str::from_utf8(bytes) {
        Ok(s) => print(s),
        Err(_) => {
            let flags = irq_save();
            irq_off();
            {
                let _hold = SERIAL.lock();
                for &b in bytes {
                    console::write_byte(b);
                }
            }
            irq_restore(flags);
        }
    }
}

extern "C" fn trampoline() -> ! {
    let (entry, user_rip, user_rsp, user_argc, user_argv, fork_regs) = {
        let flags = irq_save();
        irq_off();
        let id = CURRENT.load(Ordering::SeqCst);
        let mut tasks = TASKS.lock();
        let t = &mut tasks[id];
        let fr = t.fork_regs.take();
        let out = (
            t.entry,
            t.user_rip,
            t.user_rsp,
            t.user_argc,
            t.user_argv,
            fr,
        );
        drop(tasks);
        irq_restore(flags);
        out
    };
    if let Some(fr) = fork_regs {
        crate::user::enter_fork(fr);
    }
    if user_rip != 0 {
        crate::user::enter(user_rip, user_rsp, user_argc, user_argv);
    }
    irq_on();
    if let Some(f) = entry {
        f();
    }
    die()
}

pub fn die() -> ! {
    irq_off();
    {
        let mut tasks = TASKS.lock();
        let id = CURRENT.load(Ordering::SeqCst);
        if tasks[id].user_rip != 0 {
            user::note_exit();
            for entry in tasks[id].fds {
                fd_drop(entry);
            }
            tasks[id].fds = [FdEntry::Empty; MAX_FDS];
        }
        tasks[id].state = State::Dead;
        tasks[id].entry = None;
    }
    schedule();
    loop {
        irq_on();
        wait();
        irq_off();
        schedule();
    }
}

fn irq_save() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let r: u64;
        core::arch::asm!("pushfq; pop {r}", r = out(reg) r);
        r
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let r: u64;
        core::arch::asm!(
            "mrs {r}, daif",
            r = out(reg) r,
            options(nomem, nostack, preserves_flags)
        );
        r
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        let r: u64;
        core::arch::asm!(
            "csrr {r}, sstatus",
            r = out(reg) r,
            options(nomem, nostack, preserves_flags)
        );
        r
    }
}

fn irq_restore(flags: u64) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        if flags & (1 << 9) != 0 {
            core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
        } else {
            core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
        }
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr daif, {r}", r = in(reg) flags, options(nomem, nostack));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        if flags & (1 << 1) != 0 {
            core::arch::asm!("csrs sstatus, {}", in(reg) 1 << 1, options(nomem, nostack));
        } else {
            core::arch::asm!("csrc sstatus, {}", in(reg) 1 << 1, options(nomem, nostack));
        }
    }
}

fn irq_off() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr daifset, #3", options(nomem, nostack));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("csrc sstatus, {}", in(reg) 1 << 1, options(nomem, nostack));
    }
}

fn irq_on() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr daifclr, #3", options(nomem, nostack));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("csrs sstatus, {}", in(reg) 1 << 1, options(nomem, nostack));
    }
}

fn wait() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("wfi", options(nomem, nostack, preserves_flags));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("wfi", options(nomem, nostack, preserves_flags));
    }
}
