//! Threads of the calling process and waiting on an address, the building
//! blocks of user-space locks (`SYS_THREAD_*`, `SYS_WAIT_ADDR`,
//! `SYS_WAKE_ADDR`).

use core::sync::atomic::AtomicU32;

use super::sys3;

const SYS_THREAD_SPAWN: usize = 53;
const SYS_THREAD_EXIT: usize = 54;
const SYS_WAIT_ADDR: usize = 55;
const SYS_WAKE_ADDR: usize = 56;
const SYS_GETTID: usize = 57;

/// Start a thread of this process at `entry(arg)` on the stack whose top is
/// `stack_top`, with thread pointer `tls`. `entry` ends with [`exit`].
/// Returns the new thread's id.
pub fn spawn(entry: extern "C" fn(usize) -> !, stack_top: usize, arg: usize, tls: usize) -> Option<usize> {
    let params = [entry as usize, stack_top, arg, tls];
    let r = unsafe { sys3(SYS_THREAD_SPAWN, params.as_ptr() as usize, 0, 0) };
    (r != usize::MAX).then_some(r)
}

/// End the calling thread (the process ends with its last thread).
pub fn exit(code: u8) -> ! {
    unsafe { sys3(SYS_THREAD_EXIT, code as usize, 0, 0) };
    unreachable!("thread_exit returned")
}

/// The calling thread's id.
pub fn gettid() -> usize {
    unsafe { sys3(SYS_GETTID, 0, 0, 0) }
}

/// How [`wait`] ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wait {
    /// Woken by [`wake`], or spuriously: re-check the condition.
    Woken,
    /// `word` did not hold the expected value.
    Changed,
    TimedOut,
    Fault,
}

/// Block while `word` holds `expected`, until [`wake`] on it or
/// `timeout_ns` (0 = none).
pub fn wait(word: &AtomicU32, expected: u32, timeout_ns: u64) -> Wait {
    let r = unsafe { sys3(SYS_WAIT_ADDR, word.as_ptr() as usize, expected as usize, timeout_ns as usize) };
    match r {
        0 => Wait::Woken,
        1 => Wait::Changed,
        2 => Wait::TimedOut,
        _ => Wait::Fault,
    }
}

/// Wake up to `count` threads waiting on `word`. Returns how many.
pub fn wake(word: &AtomicU32, count: usize) -> usize {
    unsafe { sys3(SYS_WAKE_ADDR, word.as_ptr() as usize, count, 0) }
}
