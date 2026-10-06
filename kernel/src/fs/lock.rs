//! File locks: `flock` and `fcntl`'s record locks, kept by the kernel per
//! file ([`super::Vnode::key`]) and advisory, as on Unix: they bind only
//! those who ask for them.
//!
//! A lock is owned by an open file description (`flock`, and Linux's
//! "open file description" record locks) or by a process (POSIX record
//! locks). The two kinds Linux keeps apart stay apart: an `flock` lock never
//! meets a record lock. A lock goes when its owner lets go of it: a
//! description's with the description's last fd, a process's with the
//! process, or when the process closes any fd on the file (POSIX's rule).
//! A blocking request waits on the file, a signal ending the wait.

use alloc::vec::Vec;
use spin::Mutex;

/// Whose a lock is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// An `flock` lock of an open file description.
    Flock(usize),
    /// A record lock of an open file description (Linux's `F_OFD_*`).
    Ofd(usize),
    /// A POSIX record lock of a process.
    Process(usize),
}

impl Owner {
    /// Locks of different families never conflict (`flock` and record locks).
    fn family(self) -> u8 {
        match self {
            Owner::Flock(_) => 0,
            Owner::Ofd(_) | Owner::Process(_) => 1,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Shared,
    Exclusive,
}

/// A held lock: bytes `start..end` (`end` [`TO_END`]: to the end of the
/// file, however long it grows) of a file.
#[derive(Clone, Copy)]
pub struct Lock {
    node: usize,
    pub owner: Owner,
    pub kind: Kind,
    pub start: u64,
    pub end: u64,
}

/// A range's end that follows the file's end.
pub const TO_END: u64 = u64::MAX;

static LOCKS: Mutex<Vec<Lock>> = Mutex::new(Vec::new());

/// Why a lock was not granted.
pub enum Refused {
    /// Someone else holds a conflicting lock (`EAGAIN`).
    Busy,
    /// A signal ended the wait (`EINTR`).
    Interrupted,
}

fn conflicting(locks: &[Lock], node: usize, owner: Owner, kind: Kind, start: u64, end: u64) -> Option<Lock> {
    locks
        .iter()
        .find(|l| {
            l.node == node
                && l.owner != owner
                && l.owner.family() == owner.family()
                && l.start < end
                && start < l.end
                && (l.kind == Kind::Exclusive || kind == Kind::Exclusive)
        })
        .copied()
}

/// The first lock of someone else's that a `kind` lock of `owner` on
/// `start..end` of `node` would conflict with (`F_GETLK`).
pub fn conflict(node: usize, owner: Owner, kind: Kind, start: u64, end: u64) -> Option<Lock> {
    conflicting(&LOCKS.lock(), node, owner, kind, start, end)
}

/// Lock `start..end` of `node` for `owner` (`kind`), or unlock it (`None`):
/// what `owner` held there before is replaced, the rest of its locks stay
/// (split where the range cuts one). `wait`: a conflicting lock is waited
/// out rather than refused. An unlock always succeeds.
pub fn set(node: usize, owner: Owner, kind: Option<Kind>, start: u64, end: u64, wait: bool) -> Result<(), Refused> {
    loop {
        let seq = crate::task::wait_seq();
        {
            let mut locks = LOCKS.lock();
            let busy = kind.is_some_and(|k| conflicting(&locks, node, owner, k, start, end).is_some());
            if !busy {
                replace(&mut locks, node, owner, kind, start, end);
                drop(locks);
                // Someone may wait for what was let go.
                crate::task::wake(crate::task::key_lock(node));
                return Ok(());
            }
        }
        if !wait {
            return Err(Refused::Busy);
        }
        if crate::signal::interrupt_wait() {
            return Err(Refused::Interrupted);
        }
        crate::task::block_until(crate::task::key_lock(node), seq, 0);
    }
}

fn replace(locks: &mut Vec<Lock>, node: usize, owner: Owner, kind: Option<Kind>, start: u64, end: u64) {
    let mut kept = Vec::with_capacity(locks.len() + 2);
    for l in locks.drain(..) {
        if l.node != node || l.owner != owner || l.end <= start || end <= l.start {
            kept.push(l);
            continue;
        }
        // The parts of `l` outside the new range stay.
        if l.start < start {
            kept.push(Lock { end: start, ..l });
        }
        if end < l.end {
            kept.push(Lock { start: end, ..l });
        }
    }
    if let Some(kind) = kind {
        kept.push(Lock { node, owner, kind, start, end });
    }
    *locks = kept;
}

/// `owner` let go of everything (a description closed, a process ended).
pub fn release(owner: Owner) {
    release_where(|l| l.owner == owner);
}

/// Process `pid` closed an fd on `node`: its record locks there go (POSIX).
pub fn closed(node: usize, pid: usize) {
    release_where(|l| l.node == node && l.owner == Owner::Process(pid));
}

fn release_where(gone: impl Fn(&Lock) -> bool) {
    let mut nodes = Vec::new();
    {
        let mut locks = LOCKS.lock();
        locks.retain(|l| {
            if gone(l) {
                nodes.push(l.node);
                false
            } else {
                true
            }
        });
    }
    nodes.sort_unstable();
    nodes.dedup();
    for node in nodes {
        crate::task::wake(crate::task::key_lock(node));
    }
}
