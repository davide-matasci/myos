//! Round-robin kernel threads. Cooperative (`yield_now`) and preemptive
//! (timer IRQ calls the same `schedule` after EOI). Not userspace.

use alloc::alloc::{alloc, Layout};
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::Mutex;

use crate::arch::SerialPort;

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Unused,
    Ready,
    Running,
    Dead,
}

#[derive(Clone, Copy)]
struct Task {
    state: State,
    #[allow(dead_code)]
    stack_base: usize,
    sp: usize,
    entry: Option<fn()>,
}

const EMPTY: Task = Task {
    state: State::Unused,
    stack_base: 0,
    sp: 0,
    entry: None,
};

static TASKS: Mutex<[Task; MAX_TASKS]> = Mutex::new([EMPTY; MAX_TASKS]);
static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PREEMPT_ON: AtomicBool = AtomicBool::new(false);
static SERIAL: Mutex<()> = Mutex::new(());

pub fn init() {
    let mut tasks = TASKS.lock();
    tasks[0].state = State::Running;
    tasks[0].sp = 0;
    CURRENT.store(0, Ordering::SeqCst);
}

pub fn enable_preempt() {
    PREEMPT_ON.store(true, Ordering::SeqCst);
}

pub fn spawn(entry: fn()) {
    let layout = Layout::from_size_align(STACK_SIZE, 16).expect("task stack layout");
    let stack = unsafe { alloc(layout) };
    assert!(!stack.is_null(), "task stack alloc");
    let sp = unsafe { seed_stack(stack, STACK_SIZE, trampoline as usize) };

    let mut tasks = TASKS.lock();
    let slot = tasks
        .iter()
        .position(|t| t.state == State::Unused)
        .expect("no task slot");
    tasks[slot] = Task {
        state: State::Ready,
        stack_base: stack as usize,
        sp,
        entry: Some(entry),
    };
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

    let (old_sp, new_sp) = {
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
        CURRENT.store(next, Ordering::SeqCst);
        (old_sp, new_sp)
    };

    unsafe {
        task_switch(old_sp, new_sp);
    }
}

/// Serial write used by tasks. Holds a lock so bytes from two tasks cannot
/// interleave. IRQs off for the write so a timer tick cannot switch while
/// the UART is mid-character. Panic still fire-and-forgets.
pub fn print(s: &str) {
    irq_off();
    {
        let _hold = SERIAL.lock();
        let mut serial = SerialPort::new();
        let _ = serial.write_str(s);
    }
    irq_on();
}

extern "C" fn trampoline() -> ! {
    // IRQs are still off from yield/timer; take the lock before unmasking.
    let entry = {
        let id = CURRENT.load(Ordering::SeqCst);
        TASKS.lock()[id].entry
    };
    irq_on();
    if let Some(f) = entry {
        f();
    }
    die()
}

fn die() -> ! {
    irq_off();
    {
        let mut tasks = TASKS.lock();
        let id = CURRENT.load(Ordering::SeqCst);
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
