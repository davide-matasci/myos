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
