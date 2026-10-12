//! Sets of task slots, a bit per slot: the run queues ([`AtomicTaskSet`],
//! read without `TASKS` by the idle loop) and the slots other processes
//! still go by as a group or session id ([`TaskBits`]).

use core::sync::atomic::{AtomicU64, Ordering};

use super::MAX_TASKS;

const WORDS: usize = MAX_TASKS.div_ceil(64);

/// A set of task slots.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TaskBits([u64; WORDS]);

impl TaskBits {
    pub const EMPTY: Self = Self([0; WORDS]);

    pub fn insert(&mut self, slot: usize) {
        self.0[slot / 64] |= 1 << (slot % 64);
    }

    pub fn contains(&self, slot: usize) -> bool {
        slot < MAX_TASKS && self.0[slot / 64] & 1 << (slot % 64) != 0
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|&w| w == 0)
    }

    pub fn union(mut self, other: Self) -> Self {
        for (w, o) in self.0.iter_mut().zip(other.0) {
            *w |= o;
        }
        self
    }

    /// The slots, lowest first.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(|(i, &w)| {
            let mut w = w;
            core::iter::from_fn(move || {
                (w != 0).then(|| {
                    let b = w.trailing_zeros() as usize;
                    w &= w - 1;
                    i * 64 + b
                })
            })
        })
    }

    /// Round-robin: the first slot above `current`, or else the lowest.
    pub fn next_after(&self, current: usize) -> Option<usize> {
        self.iter().find(|&s| s > current).or_else(|| self.iter().next())
    }
}

/// A set of task slots changed a bit at a time without a lock.
pub struct AtomicTaskSet([AtomicU64; WORDS]);

impl AtomicTaskSet {
    pub const fn new() -> Self {
        Self([const { AtomicU64::new(0) }; WORDS])
    }

    pub fn insert(&self, slot: usize) {
        self.0[slot / 64].fetch_or(1 << (slot % 64), Ordering::SeqCst);
    }

    pub fn remove(&self, slot: usize) {
        self.0[slot / 64].fetch_and(!(1 << (slot % 64)), Ordering::SeqCst);
    }

    /// The set as it is now (word by word: a bit that changes meanwhile
    /// may or may not be in it, as with one word read a moment earlier).
    pub fn load(&self) -> TaskBits {
        TaskBits(core::array::from_fn(|i| self.0[i].load(Ordering::SeqCst)))
    }
}
