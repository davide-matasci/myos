//! Linux threads on the core's (`task::thread`): `clone` with
//! `CLONE_THREAD`, `futex` on `task::wait_addr` / `wake_addr`, thread ids,
//! and the `CLONE_CHILD_CLEARTID` handshake `pthread_join` relies on.

use core::sync::atomic::{AtomicUsize, Ordering};

use super::abi::*;
use super::sys::{self, put, R};
use crate::k::task::{self, AddrWait};
use crate::k::MAX_TASKS;
use crate::k::user::SyscallRegs;

const CLONE_VM: usize = 0x100;
const CLONE_FS: usize = 0x200;
const CLONE_FILES: usize = 0x400;
const CLONE_SIGHAND: usize = 0x800;
const CLONE_THREAD: usize = 0x1_0000;
const CLONE_SETTLS: usize = 0x8_0000;
const CLONE_PARENT_SETTID: usize = 0x10_0000;
const CLONE_CHILD_CLEARTID: usize = 0x20_0000;
const CLONE_CHILD_SETTID: usize = 0x100_0000;

/// Per thread slot: where the thread's end stores 0 and wakes a waiter
/// (`set_tid_address`, `CLONE_CHILD_CLEARTID`); 0 = nowhere.
static CLEAR_TID: [AtomicUsize; MAX_TASKS] = [const { AtomicUsize::new(0) }; MAX_TASKS];

/// Hook: a new thread or process starts in `slot`.
pub fn on_new_task(slot: usize) {
    CLEAR_TID[slot].store(0, Ordering::Relaxed);
}

/// `clone(flags, stack, ptid, tls, ctid)` (the arch passes them in this
/// order). Two forms: a thread (`CLONE_THREAD`, sharing what the core's
/// threads share), and the fork-equivalent one (exit signal in the low
/// byte, nothing shared, no new stack).
pub fn clone(regs: &SyscallRegs, flags: usize, stack: usize, ptid: usize, tls: usize, ctid: usize) -> R {
    if flags & CLONE_THREAD == 0 {
        if flags & !0xff != 0 || stack != 0 {
            return Err(ENOSYS);
        }
        return sys::fork(regs);
    }
    const SHARED: usize = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND;
    if flags & SHARED != SHARED || stack == 0 {
        return Err(EINVAL);
    }
    // The child resumes like a forked one (result 0), on its own stack.
    let tls = (flags & CLONE_SETTLS != 0).then_some(tls as u64);
    let tid = task::spawn_thread_from(regs, stack, tls).ok_or(EAGAIN)?;
    // The new thread cannot run before this syscall returns (it shares this
    // CPU, and the syscall runs with interrupts off), so it finds these set.
    // Parent and child share their memory: both ids are stored from here.
    for (flag, at) in [(CLONE_PARENT_SETTID, ptid), (CLONE_CHILD_SETTID, ctid)] {
        if flags & flag != 0 {
            put(at, &(tid as u32).to_le_bytes())?;
        }
    }
    if flags & CLONE_CHILD_CLEARTID != 0 {
        CLEAR_TID[tid].store(ctid, Ordering::Relaxed);
    }
    Ok(tid)
}

/// `set_tid_address(addr)`: the calling thread's end clears and wakes `addr`.
pub fn set_tid_address(addr: usize) -> usize {
    let tid = task::current_tid();
    CLEAR_TID[tid].store(addr, Ordering::Relaxed);
    tid
}

/// `exit`: end the calling thread (`exit_group` ends the process).
pub fn exit(code: usize) -> ! {
    let addr = CLEAR_TID[task::current_tid()].swap(0, Ordering::Relaxed);
    if addr != 0 && put(addr, &0u32.to_le_bytes()).is_ok() {
        task::wake_addr(addr, 1);
    }
    task::thread_exit(code as u8);
}

/// `futex(uaddr, op, val, timeout, uaddr2, val3)`: the wait and wake
/// operations (bitsets are treated as "all"), and requeue as a wake of
/// every waiter (they re-check their condition anyway).
pub fn futex(uaddr: usize, op: usize, val: usize, timeout: usize, val3: usize) -> R {
    const WAIT: usize = 0;
    const WAKE: usize = 1;
    const REQUEUE: usize = 3;
    const CMP_REQUEUE: usize = 4;
    const WAIT_BITSET: usize = 9;
    const WAKE_BITSET: usize = 10;
    // Without FUTEX_PRIVATE_FLAG and FUTEX_CLOCK_REALTIME: every futex is
    // private to its process, and every Linux clock reads the same one.
    match op & 0x7f {
        WAIT | WAIT_BITSET => {
            // FUTEX_WAIT's timeout is relative, FUTEX_WAIT_BITSET's absolute.
            let deadline = match timeout {
                0 => 0,
                ts => sys::monotonic_deadline(sys::timespec_deadline(ts, op & 0x7f == WAIT_BITSET)?),
            };
            match task::wait_addr(uaddr, val as u32, deadline) {
                AddrWait::Woken => Ok(0),
                AddrWait::Changed => Err(EAGAIN),
                AddrWait::TimedOut => Err(ETIMEDOUT),
                AddrWait::Interrupted => Err(EINTR),
                AddrWait::Fault => Err(EFAULT),
            }
        }
        WAKE | WAKE_BITSET => Ok(task::wake_addr(uaddr, val)),
        REQUEUE | CMP_REQUEUE => {
            if op & 0x7f == CMP_REQUEUE {
                let mut b = [0u8; 4];
                sys::get_bytes(uaddr, &mut b)?;
                if u32::from_le_bytes(b) != val3 as u32 {
                    return Err(EAGAIN);
                }
            }
            Ok(task::wake_addr(uaddr, usize::MAX))
        }
        _ => Err(ENOSYS),
    }
}
