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
//! A frame counts its mappings ([`map`], [`share`], [`release`]). A
//! mapping without write permission gets the cached frame; a writable
//! private one gets a copy ([`copy`]), and nothing writes to such a frame
//! (the kernel's copies to user memory refuse: `user::uaccess`). A write
//! to the file or its truncation drops those pages from the cache
//! ([`invalidate`], [`truncated`]); unlinking it, or unmounting its
//! filesystem, lets go of the file first ([`forget_at`], [`forget_mount`]),
//! so the cache keeps no removed file alive and no filesystem busy. Pages
//! still mapped then stay with their mappings, and the last one frees them.
//! The cache holds up to a quarter of usable RAM; past that, and when
//! memory runs out ([`release_unmapped`]), the pages no mapping holds go.
//!
//! A shared writable mapping (`MAP_SHARED` of a regular file) gets the
//! cached frame too, and writes to it ([`map_shared`]): the frame is then
//! the file's page, *dirty*, until it is written back. Every mapping of the
//! page, shared or not, sees the writes at once; a `read` of the file sees
//! them too ([`overlay`]), a `write` to the file lands in the frame
//! ([`written`]) and a truncation cuts it ([`truncated`]). The frame goes
//! back to the file ([`sync`]: `msync`, `munmap`, exit, exec, the file
//! forgotten) and is clean again once no mapping holds it; a dirty frame
//! is never evicted, it is written back first ([`trim`]).

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
    /// How many of them are dirty.
    dirty: usize,
}

struct Frame {
    /// The mappings of it.
    maps: u32,
    /// The file page it holds while the cache has it: (node, page index).
    page: Option<(usize, u32)>,
    /// A shared writable mapping has or had it: its bytes are the file's
    /// page, to be written back. Clean again only once nothing maps it.
    dirty: bool,
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
    map_as(node, page, false, false)
}

/// [`map`] for a shared mapping (`MMAP_SHARED`): the frame is always the
/// cached one (a page read while the file changed is read again), and the
/// file's page from now on (dirty) when the mapping is `writable`.
pub fn map_shared(node: &Vnode, page: usize, writable: bool) -> u64 {
    map_as(node, page, true, writable)
}

fn map_as(node: &Vnode, page: usize, shared: bool, dirty: bool) -> u64 {
    let (id, index) = (node.key(), page as u32);
    loop {
        let cached = locked(|c| {
            let frame = *c.files.get(&id)?.pages.get(&index)?;
            let f = c.frames.get_mut(&frame)?;
            f.maps += 1;
            if dirty && !f.dirty {
                f.dirty = true;
                c.files.get_mut(&id)?.dirty += 1;
            }
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
        let (frame, unused, kept) = locked(|c| {
            if let Some(n) = c.reading.get_mut(&id) {
                *n -= 1;
                if *n == 0 {
                    c.reading.remove(&id);
                }
            }
            if c.changes != changes {
                return (frame, None, false);
            }
            if let Some(&had) = c.files.get(&id).and_then(|f| f.pages.get(&index)) {
                // Another CPU read it first.
                if let Some(f) = c.frames.get_mut(&had) {
                    f.maps += 1;
                    if dirty && !f.dirty {
                        f.dirty = true;
                        if let Some(file) = c.files.get_mut(&id) {
                            file.dirty += 1;
                        }
                    }
                }
                return (had, Some(frame), true);
            }
            let file = c
                .files
                .entry(id)
                .or_insert_with(|| File { node: spare.take().unwrap(), pages: BTreeMap::new(), dirty: 0 });
            file.pages.insert(index, frame);
            file.dirty += dirty as usize;
            c.frames.insert(frame, Frame { maps: 1, page: Some((id, index)), dirty });
            c.held += 1;
            (frame, None, true)
        });
        drop(spare);
        if let Some(unused) = unused {
            mm::free_frame(unused);
        }
        if !kept && shared {
            // The file changed while it was read: a shared mapping must
            // have the cached frame, so it is read again.
            mm::free_frame(frame);
            continue;
        }
        trim();
        return frame;
    }
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

/// `frame` is a cache frame (a private mapping made writable copies it).
pub fn is_cached(frame: u64) -> bool {
    locked(|c| c.frames.contains_key(&frame))
}

/// A shared mapping of `frame` became writable (`mprotect`): dirty from
/// now on, if the cache has it.
pub fn dirtied(frame: u64) {
    locked(|c| {
        let Some(f) = c.frames.get_mut(&frame) else {
            return;
        };
        if let (false, Some((id, _))) = (f.dirty, f.page) {
            f.dirty = true;
            if let Some(file) = c.files.get_mut(&id) {
                file.dirty += 1;
            }
        }
    });
}

/// `node`'s file was written at `pos` (`buf` is what went there): the
/// dirty pages take the bytes, and the clean ones leave the cache, as
/// [`invalidate`] drops them.
pub fn written(node: &Vnode, pos: usize, buf: &[u8]) {
    each_dirty_page(node, pos, buf.len(), |part, lo, hi| part.copy_from_slice(&buf[lo..hi]));
    invalidate(node);
}

/// Copy the dirty pages of `node`'s file over `out`, read from `pos`
/// (`read`): the bytes the mappings wrote are the file's.
pub fn overlay(node: &Vnode, pos: usize, out: &mut [u8]) {
    each_dirty_page(node, pos, out.len(), |part, lo, hi| out[lo..hi].copy_from_slice(part));
}

/// Run `f(part, lo, hi)` for each dirty page of `node`'s file that
/// `[pos, pos + len)` touches: `part` is the page's bytes in the range,
/// `lo..hi` where they are in it. One page per hold of the lock, with
/// interrupts off: a copy of the whole range would be a long one.
fn each_dirty_page(node: &Vnode, pos: usize, len: usize, mut f: impl FnMut(&mut [u8], usize, usize)) {
    let id = node.key();
    let Some(end) = pos.checked_add(len).filter(|_| len > 0) else {
        return;
    };
    if !locked(|c| c.files.get(&id).is_some_and(|f| f.dirty > 0)) {
        return;
    }
    for index in pos / PAGE..=(end - 1) / PAGE {
        let base = index * PAGE;
        let (lo, hi) = (pos.max(base), end.min(base + PAGE));
        // Under the lock: an eviction could free the frame otherwise.
        locked(|c| {
            let Some(&frame) = c.files.get(&id).and_then(|file| file.pages.get(&(index as u32))) else {
                return;
            };
            if c.frames.get(&frame).is_some_and(|f| f.dirty) {
                f(&mut bytes(frame)[lo - base..hi - base], lo - pos, hi - pos);
            }
        });
    }
}

/// `node`'s file changed (a write, a truncation): its pages leave the cache,
/// the dirty ones excepted (they are the file's pages). Nothing to do for
/// the files it neither has nor reads (most writes).
pub fn invalidate(node: &Vnode) {
    let id = node.key();
    if locked(|c| c.files.contains_key(&id) || c.reading.contains_key(&id)) {
        drop_pages(|fid, _| fid == id, true);
    }
}

/// `node`'s file is `size` bytes long now (`ftruncate`, `O_TRUNC`): the
/// pages past the end go, dirty or not (a mapping of one keeps its frame,
/// which the file has no page for any more), the dirty page the end falls
/// in is zero past it, and the clean pages leave the cache as after a write.
pub fn truncated(node: &Vnode, size: usize) {
    let id = node.key();
    let mut frees = Vec::new();
    let node = locked(|c| {
        if c.reading.contains_key(&id) {
            c.changes += 1;
        }
        let file = c.files.get_mut(&id)?;
        c.changes += 1;
        let keep = size.div_ceil(PAGE) as u32;
        let mut dropped = 0;
        file.pages.retain(|&index, &mut frame| {
            let Some(f) = c.frames.get_mut(&frame) else {
                return false;
            };
            if index < keep && f.dirty {
                if index == (size / PAGE) as u32 {
                    bytes(frame)[size % PAGE..].fill(0);
                }
                return true;
            }
            dropped += 1;
            f.page = None;
            if f.maps == 0 {
                c.frames.remove(&frame);
                frees.push(frame);
            }
            false
        });
        c.held -= dropped;
        file.dirty = file.pages.len();
        if file.pages.is_empty() {
            return c.files.remove(&id).map(|f| f.node);
        }
        None
    });
    for frame in frees {
        mm::free_frame(frame);
    }
    drop(node);
}

/// Write the dirty pages of `node`'s file back to it (`msync`, and the
/// end of a shared mapping). A page nothing maps any more is clean after
/// it; one still mapped stays dirty, to be written again.
pub fn sync(node: &Vnode) {
    sync_files(|id, _| id == node.key());
}

/// [`sync`] for the files `pick` chooses.
fn sync_files(pick: impl Fn(usize, &File) -> bool) {
    let ids: Vec<usize> = locked(|c| c.files.iter().filter(|(id, f)| f.dirty > 0 && pick(**id, f)).map(|(id, _)| *id).collect());
    for id in ids {
        write_back(id, |_| true);
    }
}

/// Write the dirty pages `pick` chooses of file `id` back to it, cleaning
/// the ones nothing maps. The frame is copied under the lock (an eviction
/// might free it meanwhile otherwise), the copy written outside it.
fn write_back(id: usize, pick: impl Fn(&Frame) -> bool) {
    let mut copy = alloc::vec![0u8; PAGE];
    let mut from = 0u32;
    loop {
        let next = locked(|c| {
            let file = c.files.get_mut(&id)?;
            let (&index, &frame) = file.pages.range(from..).find(|(_, frame)| c.frames.get(frame).is_some_and(|f| f.dirty && pick(f)))?;
            let f = c.frames.get_mut(&frame)?;
            if f.maps == 0 {
                f.dirty = false;
                file.dirty -= 1;
            }
            copy.copy_from_slice(bytes(frame));
            Some((index, file.node.clone()))
        });
        let Some((index, node)) = next else {
            break;
        };
        let pos = index as usize * PAGE;
        // Only as far as the file goes: a mapping is page-sized, the file
        // is not (and a page past its end is not written at all).
        if let Some(size) = super::vfs::size_of(&node).filter(|&size| size > pos) {
            let _ = super::vfs::write_through(&node, pos, &copy[..(size - pos).min(PAGE)]);
        }
        drop(node);
        from = index + 1;
    }
}

/// Let go of the file at `rel` of mount `mount`, about to be unlinked or
/// replaced (the cache would keep it otherwise).
pub fn forget_at(mount: usize, rel: &str) {
    let pick = |_: usize, f: &File| node::location(&f.node).is_some_and(|(m, r)| m == mount && r.as_str() == rel);
    sync_files(pick);
    drop_pages(pick, false);
}

/// Let go of every file on mount `mount`, about to be unmounted (its dirty
/// pages written back first).
pub fn forget_mount(mount: usize) {
    let pick = |_: usize, f: &File| node::name(&f.node).is_some_and(|(m, ..)| m == mount);
    sync_files(pick);
    drop_pages(pick, false);
}

/// Drop the pages of the files `pick` chooses (the dirty ones too, unless
/// `keep_dirty`), the mapped ones staying with their mappings, and the
/// files left without pages.
fn drop_pages(pick: impl Fn(usize, &File) -> bool, keep_dirty: bool) {
    let mut frees = Vec::new();
    let nodes = locked(|c| {
        c.changes += 1;
        let ids: Vec<usize> = c.files.iter().filter(|(id, f)| pick(**id, f)).map(|(id, _)| *id).collect();
        let mut nodes = Vec::new();
        for id in ids {
            let Some(file) = c.files.get_mut(&id) else {
                continue;
            };
            let mut dropped = 0;
            file.pages.retain(|_, &mut frame| {
                let Some(f) = c.frames.get_mut(&frame) else {
                    return false;
                };
                if keep_dirty && f.dirty {
                    return true;
                }
                dropped += 1;
                f.page = None;
                if f.maps == 0 {
                    c.frames.remove(&frame);
                    frees.push(frame);
                }
                false
            });
            c.held -= dropped;
            file.dirty = if keep_dirty { file.pages.len() } else { 0 };
            if file.pages.is_empty() {
                if let Some(file) = c.files.remove(&id) {
                    nodes.push(file.node);
                }
            }
        }
        nodes
    });
    for frame in frees {
        mm::free_frame(frame);
    }
    drop(nodes);
}

/// Over its share of RAM, the cache gives back unmapped pages until it is
/// an eighth under it. The dirty ones nothing maps are written back first
/// (and so clean, to go): here, where a file may be written, not in
/// [`release_unmapped`], which the frame allocator calls from anywhere.
fn trim() {
    let cap = (mm::usable_frames() / RAM_SHARE) as usize;
    if locked(|c| c.held) > cap {
        let ids: Vec<usize> = locked(|c| c.files.iter().filter(|(_, f)| f.dirty > 0).map(|(id, _)| *id).collect());
        for id in ids {
            write_back(id, |f| f.maps == 0);
        }
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
            .filter(|(_, f)| f.maps == 0 && f.page.is_some() && !f.dirty)
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
