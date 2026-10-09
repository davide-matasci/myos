//! Frames shared between address spaces by a fork, copy-on-write: the
//! child maps the parent's pages instead of copying them, both read-only
//! (`LEAF_COW` in the leaf entry), and the first store to such a page by
//! either side gives that side its own copy (`aspace::cow_break`). A
//! mapping count per shared frame says when a store may just take the
//! frame (the other mappings are gone) and when the last mapping frees it.
//! The page cache's frames are counted by the cache itself (`pagecache`).

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::mm;

/// Per shared frame: how many address spaces map it.
static SHARED: Mutex<BTreeMap<u64, u32>> = Mutex::new(BTreeMap::new());

/// Forks done, and pages copied by a store since (`/proc/meminfo`).
static FORKS: AtomicU64 = AtomicU64::new(0);
static COPIES: AtomicU64 = AtomicU64::new(0);

/// Run `f` with the table locked and interrupts off on this CPU.
fn locked<R>(f: impl FnOnce(&mut BTreeMap<u64, u32>) -> R) -> R {
    let flags = crate::arch::irq_save();
    crate::arch::irq_off();
    let r = f(&mut SHARED.lock());
    crate::arch::irq_restore(flags);
    r
}

/// One more address space maps `frame` (a fork): shared from now on.
pub(super) fn share(frame: u64) {
    locked(|s| *s.entry(frame).or_insert(1) += 1);
}

/// A mapping of `frame` went: true when it is a shared frame, which is
/// freed here with its last mapping (the caller frees an unshared one).
pub(super) fn release(frame: u64) -> bool {
    let last = locked(|s| {
        let n = s.get_mut(&frame)?;
        *n -= 1;
        if *n == 0 {
            s.remove(&frame);
            Some(true)
        } else {
            Some(false)
        }
    });
    match last {
        None => false,
        Some(last) => {
            if last {
                mm::free_frame(frame);
            }
            true
        }
    }
}

/// Before a store to `frame` through one of its mappings: true when that
/// mapping has the frame to itself now (the others went) and may write
/// it; false when it must copy it, that mapping counted out of the share.
pub(super) fn claim(frame: u64) -> bool {
    locked(|s| match s.get_mut(&frame) {
        None => true,
        Some(n) if *n <= 1 => {
            s.remove(&frame);
            true
        }
        Some(n) => {
            *n -= 1;
            false
        }
    })
}

pub(super) fn note_fork() {
    FORKS.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn note_copy() {
    COPIES.fetch_add(1, Ordering::Relaxed);
}

/// How many forks shared their pages, and how many pages a store copied
/// since (`/proc/meminfo`).
pub fn counts() -> (u64, u64) {
    (FORKS.load(Ordering::Relaxed), COPIES.load(Ordering::Relaxed))
}
