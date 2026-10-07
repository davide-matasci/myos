//! A set-once cell: a controller is written into its slot by `probe`
//! before anything reads it, and read as `&'static Controller` by every
//! context after.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU8, Ordering};

const EMPTY: u8 = 0;
const WRITING: u8 = 1;
const SET: u8 = 2;

pub struct Once<T> {
    state: AtomicU8,
    value: UnsafeCell<Option<T>>,
}

// SAFETY: the value is written once, before `SET` is published, and only
// read after.
unsafe impl<T: Send + Sync> Sync for Once<T> {}

impl<T> Once<T> {
    pub const fn new() -> Self {
        Self { state: AtomicU8::new(EMPTY), value: UnsafeCell::new(None) }
    }

    /// Store `value`: false when the cell already holds one.
    pub fn set(&self, value: T) -> bool {
        if self.state.compare_exchange(EMPTY, WRITING, Ordering::Acquire, Ordering::Relaxed).is_err() {
            return false;
        }
        // SAFETY: `WRITING` is ours alone, and no reader sees the value
        // before `SET`.
        unsafe {
            *self.value.get() = Some(value);
        }
        self.state.store(SET, Ordering::Release);
        true
    }

    pub fn get(&self) -> Option<&T> {
        if self.state.load(Ordering::Acquire) != SET {
            return None;
        }
        // SAFETY: set once, never written again.
        unsafe { (*self.value.get()).as_ref() }
    }
}
