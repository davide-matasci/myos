//! A lock whose waiters sleep: for state that is held across a wait (a USB
//! transfer blocking for its completion) and touched from a thread that
//! must not spin meanwhile (the USB thread's `probe` and `disconnect`).
//!
//! Built on the blocking-wait protocol of the [`KernelApi`] (`wait_seq`,
//! `block_until`, `wake`): a waiter blocks on the lock's own address, the
//! holder wakes that key when it lets go and someone waits. Task context
//! only, as `block_until` is: an interrupt handler never takes one. Before
//! the scheduler preempts, `block_until` returns at once and the waiter
//! spins, which is what boot needs.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::ApiCell;

pub struct SleepLock<T> {
    /// The module's table, for the wait and the wake.
    api: &'static ApiCell,
    held: AtomicBool,
    /// Tasks between announcing themselves and giving up their wait: a
    /// release wakes only when there is one.
    waiters: AtomicUsize,
    value: UnsafeCell<T>,
}

// SAFETY: the value is reached only through a guard, one at a time.
unsafe impl<T: Send> Sync for SleepLock<T> {}
unsafe impl<T: Send> Send for SleepLock<T> {}

/// The value, exclusively, until the guard is dropped.
pub struct SleepGuard<'a, T> {
    lock: &'a SleepLock<T>,
}

impl<T> SleepLock<T> {
    /// A lock on `value` that waits and wakes through the table `api` keeps.
    pub const fn new(api: &'static ApiCell, value: T) -> Self {
        Self {
            api,
            held: AtomicBool::new(false),
            waiters: AtomicUsize::new(0),
            value: UnsafeCell::new(value),
        }
    }

    /// What a waiter blocks on: the lock's address, unique for a `static`.
    fn key(&self) -> usize {
        self as *const Self as usize
    }

    fn try_take(&self) -> bool {
        self.held.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok()
    }

    /// The lock, sleeping until the holder lets go.
    pub fn lock(&self) -> SleepGuard<'_, T> {
        loop {
            if self.try_take() {
                return SleepGuard { lock: self };
            }
            // Announce the wait, then try once more: a release between the
            // first try and the announcement wakes nobody, and this try
            // sees it (both sides are sequentially consistent). A release
            // after the announcement bumps the sequence read here, or
            // wakes the key, so `block_until` returns.
            self.waiters.fetch_add(1, Ordering::SeqCst);
            let api = self.api.get();
            let seq = api.wait_seq();
            if self.try_take() {
                self.waiters.fetch_sub(1, Ordering::SeqCst);
                return SleepGuard { lock: self };
            }
            api.block_until(self.key(), seq, 0);
            self.waiters.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// The lock if nobody holds it, without waiting.
    pub fn try_lock(&self) -> Option<SleepGuard<'_, T>> {
        self.try_take().then_some(SleepGuard { lock: self })
    }
}

impl<T> Deref for SleepGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the guard holds the lock.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for SleepGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the guard holds the lock, and `&mut self` is the only
        // way to the value through it.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SleepGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.held.store(false, Ordering::SeqCst);
        if self.lock.waiters.load(Ordering::SeqCst) != 0 {
            self.lock.api.get().wake(self.lock.key());
        }
    }
}
