//! Threads on myos: `sleep` only. A process is one task, so spawning,
//! naming and parallelism come from `unsupported`; sleeping blocks the task
//! in the kernel (`SYS_NANOSLEEP`) until the duration passed, resuming the
//! wait after a signal.

use crate::sys::myos::abi;
use crate::time::{Duration, SystemTime};

pub fn sleep(dur: Duration) {
    let start = SystemTime::now();
    let mut left = dur;
    loop {
        if abi::nanosleep(left) {
            return;
        }
        // Interrupted: sleep what is left, by the wall clock.
        let slept = SystemTime::now().duration_since(start).unwrap_or(Duration::ZERO);
        match dur.checked_sub(slept) {
            Some(l) if !l.is_zero() => left = l,
            _ => return,
        }
    }
}
