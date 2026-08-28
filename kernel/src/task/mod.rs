//! Round-robin kernel threads plus user processes (own CR3/TTBR0).
//! Cooperative (`yield_now`) and preemptive (timer IRQ calls the same
//! `schedule` after EOI).

use alloc::alloc::{alloc, Layout};
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use crate::arch::SerialPort;
use crate::user;

#[cfg(target_arch = "x86_64")]
mod switch_x86;
#[cfg(target_arch = "x86_64")]
use switch_x86::{seed_stack, task_switch};

#[cfg(target_arch = "aarch64")]
mod switch_aarch64;
#[cfg(target_arch = "aarch64")]
use switch_aarch64::{seed_stack, task_switch};

const MAX_TASKS: usize = 8;
const STACK_SIZE: usize = 8 * 1024;
const MAX_FDS: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Unused,
    Ready,
    Running,
    Dead,
}

#[derive(Clone, Copy)]
struct Fd {
    data: &'static [u8],
    pos: usize,
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
    #[cfg(target_arch = "aarch64")]
    pub x: [u64; 31],
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
    fds: [Option<Fd>; MAX_FDS],
    user_base: u64,
    image_span: usize,
    stack_off: u64,
    ppid: usize,
    fork_regs: Option<ForkRegs>,
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
    fds: [None; MAX_FDS],
    user_base: 0,
    image_span: 0,
    stack_off: 0,
    ppid: 0,
    fork_regs: None,
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
    let mut tasks = TASKS.lock();
    tasks[0].state = State::Running;
    tasks[0].sp = 0;
    CURRENT.store(0, Ordering::SeqCst);
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

pub fn current_aspace() -> u64 {
    let id = CURRENT.load(Ordering::SeqCst);
    TASKS.lock()[id].aspace
}

/// Per-task user map: (USER_BASE, IMAGE_SPAN, STACK_OFF).
pub fn current_user_map() -> (u64, usize, u64) {
    let id = CURRENT.load(Ordering::SeqCst);
    let t = TASKS.lock()[id];
    (t.user_base, t.image_span, t.stack_off)
}

pub fn save_user_context(rip: usize, rsp: usize) {
    with_current_mut(|t| {
        t.user_rip = rip;
        t.user_rsp = rsp;
    });
}

fn with_current_mut<R>(f: impl FnOnce(&mut Task) -> R) -> R {
    let mut tasks = TASKS.lock();
    let id = CURRENT.load(Ordering::SeqCst);
    f(&mut tasks[id])
}

pub fn fd_open(data: &'static [u8]) -> Option<usize> {
    with_current_mut(|t| {
        for (i, slot) in t.fds.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(Fd { data, pos: 0 });
                return Some(i);
            }
        }
        None
    })
}

/// Copy from `fd` into the user buffer at `buf`. `range_ok` must accept the
/// actual byte count. Bad fd or a failed range check returns `usize::MAX`.
pub fn fd_read(
    fd: usize,
    buf: usize,
    len: usize,
    range_ok: fn(usize, usize) -> bool,
) -> usize {
    with_current_mut(|t| {
        let Some(Some(f)) = t.fds.get_mut(fd) else {
            return usize::MAX;
        };
        let n = len.min(f.data.len().saturating_sub(f.pos));
        if n != 0 && !range_ok(buf, n) {
            return usize::MAX;
        }
        if n != 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(f.data.as_ptr().add(f.pos), buf as *mut u8, n);
            }
        }
        f.pos += n;
        n
    })
}

pub fn fd_close(fd: usize) -> bool {
    with_current_mut(|t| {
        let Some(slot) = t.fds.get_mut(fd) else {
            return false;
        };
        if slot.is_some() {
            *slot = None;
            true
        } else {
            false
        }
    })
}

/// In-place exec: replace the current task's user image. Does not spawn,
/// does not bump USERS_ALIVE, does not note_exit. Resets fds.
pub fn replace_user(
    aspace: u64,
    user_rip: usize,
    user_rsp: usize,
    user_base: u64,
    image_span: usize,
    stack_off: u64,
) {
    with_current_mut(|t| {
        t.aspace = aspace;
        t.user_rip = user_rip;
        t.user_rsp = user_rsp;
        t.user_base = user_base;
        t.image_span = image_span;
        t.stack_off = stack_off;
        t.fds = [None; MAX_FDS];
        t.fork_regs = None;
    });
    user::switch_aspace(aspace);
    LOADED_ASPACE.store(aspace, Ordering::SeqCst);
}

pub fn spawn(entry: fn()) {
    spawn_inner(0, Some(entry), 0, 0, 0, 0, 0, 0, [None; MAX_FDS], None);
}

pub fn spawn_user(
    aspace: u64,
    user_rip: usize,
    user_rsp: usize,
    user_base: u64,
    image_span: usize,
    stack_off: u64,
) {
    spawn_inner(
        aspace,
        None,
        user_rip,
        user_rsp,
        user_base,
        image_span,
        stack_off,
        0,
        [None; MAX_FDS],
        None,
    );
}

/// Copy the current user task: new aspace, copied fds, fork resume regs.
/// Child is Ready and will resume userspace with rax/x0 = 0. Returns child slot.
pub fn fork_current(child_regs: ForkRegs) -> Option<usize> {
    let flags = irq_save();
    irq_off();

    let (fds, base, span, off, ppid) = {
        let tasks = TASKS.lock();
        let id = CURRENT.load(Ordering::SeqCst);
        let t = tasks[id];
        if t.user_rip == 0 {
            drop(tasks);
            irq_restore(flags);
            return None;
        }
        (t.fds, t.user_base, t.image_span, t.stack_off, id)
    };

    let Some(aspace) = user::copy_user_aspace(base, span, off) else {
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
    let stack = unsafe { alloc(layout) };
    if stack.is_null() {
        irq_restore(flags);
        return None;
    }
    let sp = unsafe { seed_stack(stack, STACK_SIZE, trampoline as usize) };
    let top = stack as usize + STACK_SIZE;

    let mut tasks = TASKS.lock();
    let Some(slot) = tasks.iter().position(|t| t.state == State::Unused) else {
        drop(tasks);
        irq_restore(flags);
        return None;
    };
    tasks[slot] = Task {
        state: State::Ready,
        stack_base: stack as usize,
        sp,
        entry: None,
        aspace,
        kernel_stack_top: top,
        user_rip: child_regs.rip,
        user_rsp: child_regs.rsp,
        fds,
        user_base: base,
        image_span: span,
        stack_off: off,
        ppid,
        fork_regs: Some(child_regs),
    };
    drop(tasks);
    user::note_fork();
    irq_restore(flags);
    Some(slot)
}

/// Yield until a child has exited, reap it, return its pid.
/// `usize::MAX` if this task has no children.
pub fn wait_child() -> usize {
    let parent = CURRENT.load(Ordering::SeqCst);
    loop {
        let mut any = false;
        let mut reap = None;
        {
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
                tasks[i] = EMPTY;
                return i;
            }
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
    ppid: usize,
    fds: [Option<Fd>; MAX_FDS],
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
    };
    drop(tasks);
    irq_restore(flags);
}

pub fn yield_now() {
    irq_off();
    schedule();
    irq_on();
}

/// Same switch as `yield_now`. No-op until `enable_preempt` so the first-tick
/// IRQ proof does not leave the Limine stack.
pub fn schedule() {
    if !PREEMPT_ON.load(Ordering::SeqCst) {
        return;
    }

    let (old_sp, new_sp, kstack, aspace) = {
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
            return;
        }

        tasks[next].state = State::Running;
        let old_sp = core::ptr::addr_of_mut!(tasks[current].sp);
        let new_sp = tasks[next].sp;
        let kstack = tasks[next].kernel_stack_top;
        let aspace = tasks[next].aspace;
        CURRENT.store(next, Ordering::SeqCst);
        (old_sp, new_sp, kstack, aspace)
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
}

pub fn print(s: &str) {
    let flags = irq_save();
    irq_off();
    {
        let _hold = SERIAL.lock();
        let mut serial = SerialPort::new();
        let _ = serial.write_str(s);
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
                let mut serial = SerialPort::new();
                for &b in bytes {
                    serial.write_byte(b);
                }
            }
            irq_restore(flags);
        }
    }
}

extern "C" fn trampoline() -> ! {
    let (entry, user_rip, user_rsp, fork_regs) = {
        let id = CURRENT.load(Ordering::SeqCst);
        let mut tasks = TASKS.lock();
        let t = &mut tasks[id];
        let fr = t.fork_regs.take();
        (t.entry, t.user_rip, t.user_rsp, fr)
    };
    if let Some(fr) = fork_regs {
        crate::user::enter_fork(fr);
    }
    if user_rip != 0 {
        crate::user::enter(user_rip, user_rsp);
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
}
