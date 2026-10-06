//! A spinlock for task context. The interrupt handler never takes one: it
//! works on the event ring and the completion records (atomics) only, so a
//! task holding a lock on the interrupted CPU cannot deadlock it. Waiters
//! yield the CPU rather than spin, since the holder may be a task that was
//! preempted mid-transfer.

use core::sync::atomic::{AtomicBool, Ordering};

pub struct Spin(AtomicBool);

pub struct Guard<'a>(&'a Spin);

impl Spin {
    pub const fn new() -> Self {
        Spin(AtomicBool::new(false))
    }

    pub fn lock(&self) -> Guard<'_> {
        while self.0.swap(true, Ordering::Acquire) {
            crate::api().task_yield();
        }
        Guard(self)
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.0.store(false, Ordering::Release);
    }
}
