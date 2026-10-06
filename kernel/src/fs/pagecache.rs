//! The page cache: the pages of files that processes map without write
//! permission (programs' and libraries' code and constant data), kept in
//! frames that every mapping of the page shares.
//!
//! One frame per file page however many processes map it: the memory, the
//! read from the file and, under QEMU's TCG, the translation of the code in
//! it (QEMU keeps translated code by physical address) are paid once. The
//! cache holds a [`Vnode`] on every file it has pages of, so the pages
//! outlive the processes that mapped them (the next run of a compiler finds
//! them) and follow the file through a rename.
//!
//! A frame counts its mappings ([`map`], [`share`], [`release`]). Only a
//! mapping without write permission gets the cached frame; a writable
//! private one gets a copy ([`copy`]), and nothing writes to a cached frame
//! (the kernel's copies to user memory refuse: `user::uaccess`). A write
//! to the file or its truncation drops its pages from the cache
//! ([`invalidate`]); unlinking it, or unmounting its filesystem, lets go of
//! the file first ([`forget_at`], [`forget_mount`]), so the cache keeps no
//! removed file alive and no filesystem busy. Pages still mapped then stay
//! with their mappings, and the last one frees them. The cache holds up to
//! a quarter of usable RAM; past that, and when memory runs out
//! ([`release_unmapped`]), the pages no mapping holds go.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;

use super::{node, Vnode};
use crate::mm;

const PAGE: usize = 4096;
/// The share of usable memory the cache may hold.
const RAM_SHARE: u64 = 4;

struct File {
    node: Vnode,
    /// Page index in the file -> frame.
    pages: BTreeMap<u32, u64>,
}

struct Frame {
    /// The mappings of it.
    maps: u32,
    /// The file page it holds while the cache has it: (node, page index).
    page: Option<(usize, u32)>,
}

struct Cache {
    files: BTreeMap<usize, File>,
    frames: BTreeMap<u64, Frame>,
    /// Frames the cache has (with a `page`).
    held: usize,
    /// Files with a page being read in ([`map`]), and how many.
    reading: BTreeMap<usize, u32>,
    /// Bumped by every invalidation of a file the cache has or reads: a
    /// page read meanwhile may be older than the file, so it is not kept.
    changes: u64,
}

static CACHE: Mutex<Cache> = Mutex::new(Cache {
    files: BTreeMap::new(),
    frames: BTreeMap::new(),
    held: 0,
    reading: BTreeMap::new(),
    changes: 0,
});

/// Run `f` with the cache locked and interrupts off on this CPU. Nothing
/// allocates or frees a frame, reads a file or drops a [`Vnode`] in there.
fn locked<R>(f: impl FnOnce(&mut Cache) -> R) -> R {
    let flags = crate::arch::irq_save();
    crate::arch::irq_off();
    let r = f(&mut CACHE.lock());
    crate::arch::irq_restore(flags);
    r
}

fn bytes(frame: u64) -> &'static mut [u8] {
    unsafe { core::slice::from_raw_parts_mut(mm::hhdm(frame), PAGE) }
}

/// The frame holding page `page` of `node`'s file, mapped once more: the
/// cached one, or one read from the file now (zero past its end) and kept.
/// A page read while the file changed is not kept: the caller's mapping
/// owns that frame alone (its [`release`] says no, and the caller frees it).
pub fn map(node: &Vnode, page: usize) -> u64 {
    let (id, index) = (node.key(), page as u32);
    let cached = locked(|c| {
        let frame = *c.files.get(&id)?.pages.get(&index)?;
        c.frames.get_mut(&frame)?.maps += 1;
        Some(frame)
    });
    if let Some(frame) = cached {
        return frame;
    }
    let changes = locked(|c| {
        *c.reading.entry(id).or_insert(0) += 1;
        c.changes
    });
    let frame = mm::alloc_frame();
    let _ = super::read(node, page * PAGE, bytes(frame));
    // Taken out here: cloning a Vnode takes the node table's lock.
    let mut spare = Some(node.clone());
    let (frame, unused) = locked(|c| {
        if let Some(n) = c.reading.get_mut(&id) {
            *n -= 1;
            if *n == 0 {
                c.reading.remove(&id);
            }
        }
        if c.changes != changes {
            return (frame, None);
        }
        if let Some(&had) = c.files.get(&id).and_then(|f| f.pages.get(&index)) {
            // Another CPU read it first.
            if let Some(f) = c.frames.get_mut(&had) {
                f.maps += 1;
            }
            return (had, Some(frame));
        }
        let file = c.files.entry(id).or_insert_with(|| File { node: spare.take().unwrap(), pages: BTreeMap::new() });
        file.pages.insert(index, frame);
        c.frames.insert(frame, Frame { maps: 1, page: Some((id, index)) });
        c.held += 1;
        (frame, None)
    });
    drop(spare);
    if let Some(unused) = unused {
        mm::free_frame(unused);
    }
    trim();
    frame
}

/// Page `page` of `node`'s file copied into `dst` (a writable private
/// mapping's own frame), through the cache.
pub fn copy(node: &Vnode, page: usize, dst: &mut [u8]) {
    let frame = map(node, page);
    dst.copy_from_slice(bytes(frame));
    if !release(frame) {
        mm::free_frame(frame);
    }
}

/// One more mapping of `frame` if it is a cache frame (a fork's copy of a
/// read-only mapping shares it): whether it is.
pub fn share(frame: u64) -> bool {
    locked(|c| c.frames.get_mut(&frame).map(|f| f.maps += 1).is_some())
}

/// A mapping of `frame` went: false if it is not a cache frame (the caller
/// frees it). The last mapping of a frame the cache no longer has frees it.
pub fn release(frame: u64) -> bool {
    let gone = locked(|c| {
        let f = c.frames.get_mut(&frame)?;
        f.maps = f.maps.saturating_sub(1);
        let gone = f.maps == 0 && f.page.is_none();
        if gone {
            c.frames.remove(&frame);
        }
        Some(gone)
    });
    match gone {
        None => false,
        Some(gone) => {
            if gone {
                mm::free_frame(frame);
            }
            true
        }
    }
}

/// `frame` is shared through the cache (nothing may write to it).
pub fn is_cached(frame: u64) -> bool {
    locked(|c| c.frames.contains_key(&frame))
}

/// `node`'s file changed (a write, a truncation): its pages leave the cache.
/// Nothing to do for the files it neither has nor reads (most writes).
pub fn invalidate(node: &Vnode) {
    let id = node.key();
    if locked(|c| c.files.contains_key(&id) || c.reading.contains_key(&id)) {
        drop_files(|fid, _| fid == id);
    }
}

/// Let go of the file at `rel` of mount `mount`, about to be unlinked or
/// replaced (the cache would keep it otherwise).
pub fn forget_at(mount: usize, rel: &str) {
    drop_files(|_, f| node::location(&f.node).is_some_and(|(m, r)| m == mount && r.as_str() == rel));
}

/// Let go of every file on mount `mount`, about to be unmounted.
pub fn forget_mount(mount: usize) {
    drop_files(|_, f| node::name(&f.node).is_some_and(|(m, ..)| m == mount));
}

/// Drop the files `pick` chooses and their pages, the mapped ones staying
/// with their mappings.
fn drop_files(pick: impl Fn(usize, &File) -> bool) {
    let mut frees = Vec::new();
    let nodes = locked(|c| {
        c.changes += 1;
        let ids: Vec<usize> = c.files.iter().filter(|(id, f)| pick(**id, f)).map(|(id, _)| *id).collect();
        let mut nodes = Vec::new();
        for id in ids {
            let Some(file) = c.files.remove(&id) else {
                continue;
            };
            for frame in file.pages.into_values() {
                c.held -= 1;
                let Some(f) = c.frames.get_mut(&frame) else {
                    continue;
                };
                f.page = None;
                if f.maps == 0 {
                    c.frames.remove(&frame);
                    frees.push(frame);
                }
            }
            nodes.push(file.node);
        }
        nodes
    });
    for frame in frees {
        mm::free_frame(frame);
    }
    drop(nodes);
}

/// Over its share of RAM, the cache gives back unmapped pages until it is
/// an eighth under it.
fn trim() {
    let cap = (mm::usable_frames() / RAM_SHARE) as usize;
    if locked(|c| c.held) > cap {
        evict(cap - cap / 8);
    }
}

/// Give back every page no mapping holds (the frame allocator ran out):
/// how many.
pub fn release_unmapped() -> usize {
    evict(0)
}

/// Give back unmapped pages until the cache has `target`: how many went.
fn evict(target: usize) -> usize {
    let (frees, nodes) = locked(|c| {
        let over = c.held.saturating_sub(target);
        let frees: Vec<u64> = c
            .frames
            .iter()
            .filter(|(_, f)| f.maps == 0 && f.page.is_some())
            .map(|(&frame, _)| frame)
            .take(over)
            .collect();
        let mut nodes = Vec::new();
        for frame in &frees {
            let Some((id, index)) = c.frames.remove(frame).and_then(|f| f.page) else {
                continue;
            };
            c.held -= 1;
            let Some(file) = c.files.get_mut(&id) else {
                continue;
            };
            file.pages.remove(&index);
            if file.pages.is_empty() {
                if let Some(file) = c.files.remove(&id) {
                    nodes.push(file.node);
                }
            }
        }
        (frees, nodes)
    });
    for &frame in &frees {
        mm::free_frame(frame);
    }
    drop(nodes);
    frees.len()
}

/// Frames the cache has (`/proc/meminfo`).
pub fn frames() -> usize {
    locked(|c| c.held)
}
