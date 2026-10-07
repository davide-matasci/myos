//! Futexes for std's locks (`sys::sync`: `Mutex`, `Condvar`, `RwLock`,
//! `Once`, thread parking) on the native `wait_addr` / `wake_addr`, which
//! wait on and wake a 32-bit word of the calling process.

use crate::sync::atomic::Atomic;
use crate::sys::myos::abi;
use crate::time::Duration;

/// An atomic for use as a futex that is at least 32-bits but may be larger
pub type Futex = Atomic<Primitive>;
/// Must be the underlying type of Futex
pub type Primitive = u32;

/// An atomic for use as a futex that is at least 8-bits but may be larger.
pub type SmallFutex = Atomic<SmallPrimitive>;
/// Must be the underlying type of SmallFutex
pub type SmallPrimitive = u32;

/// Wait while `futex` holds `expected`, up to `timeout`. False only when
/// the timeout passed; a wake, a signal or a changed word return true
/// (callers re-check).
pub fn futex_wait(futex: &Atomic<u32>, expected: u32, timeout: Option<Duration>) -> bool {
    // 0 is no limit for the kernel: a zero timeout waits at least 1 ns, and
    // one too long for 64 bits of nanoseconds none.
    let ns = timeout.map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(0).max(1));
    abi::wait_addr(futex.as_ptr(), expected, ns) != abi::WAIT_ADDR_TIMEOUT
}

#[inline]
pub fn futex_wake(futex: &Atomic<u32>) -> bool {
    abi::wake_addr(futex.as_ptr(), 1) > 0
}

#[inline]
pub fn futex_wake_all(futex: &Atomic<u32>) {
    abi::wake_addr(futex.as_ptr(), usize::MAX);
}
