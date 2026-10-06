//! Per-process heap break and mmap region bookkeeping (kept in the leader
//! slot, shared by the process's threads).

use alloc::vec::Vec;

use super::*;

fn region_end(r: &MmapRegion) -> usize {
    (r.va as usize).saturating_add(r.pages as usize * crate::user::PAGE)
}

/// `[ptr, ptr+len)` is covered by mmap regions (possibly several adjacent).
pub(super) fn mmap_range_in(mmap: &[MmapRegion], ptr: usize, len: usize) -> bool {
    if len == 0 {
        return true;
    }
    let Some(end) = ptr.checked_add(len) else {
        return false;
    };
    let mut cur = ptr;
    while cur < end {
        match region_at(mmap, cur) {
            Some(r) => cur = region_end(r),
            None => return false,
        }
    }
    true
}

/// The region of the sorted `mmap` that holds address `va`.
fn region_at(mmap: &[MmapRegion], va: usize) -> Option<&MmapRegion> {
    let i = mmap.partition_point(|r| region_end(r) <= va);
    mmap.get(i).filter(|r| r.va as usize <= va)
}

/// The part `[a, b)` of `r` (page aligned, inside it), with its file
/// offset moved along.
fn piece(r: &MmapRegion, a: usize, b: usize) -> MmapRegion {
    let page = crate::user::PAGE;
    let skip = ((a - r.va as usize) / page) as u32;
    MmapRegion {
        va: a as u64,
        pages: ((b - a) / page) as u32,
        fpage: if r.file != 0 { r.fpage + skip } else { 0 },
        ..*r
    }
}

/// Remove `[lo, hi)` from the sorted table, splitting regions it cuts.
/// Returns the pieces removed, or `None` if a split would take the table
/// past [`MAX_MMAP_REGIONS`] (nothing is changed then).
fn carve(mmap: &mut Vec<MmapRegion>, lo: usize, hi: usize) -> Option<Vec<MmapRegion>> {
    let first = mmap.partition_point(|r| region_end(r) <= lo);
    let last = first + mmap[first..].partition_point(|r| (r.va as usize) < hi);
    let mut kept = Vec::new();
    let mut removed = Vec::new();
    for r in &mmap[first..last] {
        let (rlo, rhi) = (r.va as usize, region_end(r));
        removed.push(piece(r, rlo.max(lo), rhi.min(hi)));
        for (a, b) in [(rlo, lo), (hi, rhi)] {
            if a < b {
                kept.push(piece(r, a, b));
            }
        }
    }
    if mmap.len() - (last - first) + kept.len() > MAX_MMAP_REGIONS {
        return None;
    }
    mmap.splice(first..last, kept);
    Some(removed)
}

/// Per-task user map: (USER_BASE, IMAGE_SPAN, STACK_OFF).
pub fn current_user_map() -> (u64, usize, u64) {
    with_process_mut(|t| (t.user_base, t.image_span, t.stack_off))
}

pub fn current_brk() -> u64 {
    with_process_mut(|t| t.brk_cur)
}

pub fn set_brk(brk: u64) {
    with_process_mut(|t| t.brk_cur = brk);
}

/// Drop the current task's mmap table without freeing frames (caller already
/// reclaimed them). Used after in-place exec frees anonymous maps before
/// `expand_user_elf` so a later `reclaim_user_aspace` / exit cannot double-free.
pub fn clear_mmap() {
    with_process_mut(|t| t.mmap.clear());
}

/// A copy of the current process's regions, sorted by address.
pub fn mmap_regions() -> Vec<MmapRegion> {
    with_process_mut(|t| t.mmap.clone())
}

pub fn mmap_contains(ptr: usize, len: usize) -> bool {
    with_process_mut(|t| mmap_range_in(&t.mmap, ptr, len))
}

/// Lowest free `len`-byte gap in `[area_lo, area_hi)` (page aligned).
fn free_gap(mmap: &[MmapRegion], area_lo: usize, area_hi: usize, len: usize) -> Option<usize> {
    let mut cand = area_lo;
    for r in mmap {
        let (lo, hi) = (r.va as usize, region_end(r));
        if hi <= cand {
            continue;
        }
        if cand.checked_add(len)? <= lo {
            break;
        }
        cand = hi;
    }
    (cand.checked_add(len)? <= area_hi).then_some(cand)
}

/// Record a new mapping as [`mmap_add`] does, in the lowest free gap of
/// `[area_lo, area_hi)` that holds it: its address. Found and recorded in
/// one step, so threads mapping at once get gaps of their own.
pub fn mmap_add_free(
    area_lo: usize,
    area_hi: usize,
    pages: u32,
    prot: u32,
    file: Option<(&crate::fs::Vnode, usize)>,
) -> Option<usize> {
    with_process_mut(|t| {
        let va = free_gap(&t.mmap, area_lo, area_hi, pages as usize * crate::user::PAGE)?;
        add_region(t, va as u64, pages, prot, file).then_some(va)
    })
}

/// Record a new mapping, backed by `file` from byte `off` (page aligned)
/// or anonymous. It joins an adjacent mapping that continues it (same
/// protection and backing) when there is one: malloc implementations such
/// as musl's map many small neighbouring blocks, hundreds of mappings in
/// few runs. False when the region or the mapped-file table is full.
pub fn mmap_add(va: u64, pages: u32, prot: u32, file: Option<(&crate::fs::Vnode, usize)>) -> bool {
    with_process_mut(|t| add_region(t, va, pages, prot, file))
}

fn add_region(t: &mut Process, va: u64, pages: u32, prot: u32, file: Option<(&crate::fs::Vnode, usize)>) -> bool {
    let (file, fpage) = match file {
        None => (0, 0),
        Some((node, off)) => match mapped_file_slot(t, node) {
            Some(i) => (i as u32 + 1, (off / crate::user::PAGE) as u32),
            None => return false,
        },
    };
    let i = t.mmap.partition_point(|r| r.va < va);
    t.mmap.insert(i, MmapRegion { va, pages, prot, file, fpage });
    coalesce(&mut t.mmap);
    if t.mmap.len() > MAX_MMAP_REGIONS {
        // It joined no neighbour: the table is full.
        t.mmap.remove(i);
        return false;
    }
    true
}

/// The `mapped_files` entry for `node`: the one already holding it, else
/// one no region names any more.
fn mapped_file_slot(t: &mut Process, node: &crate::fs::Vnode) -> Option<usize> {
    let used = |i: usize| t.mmap.iter().any(|r| r.file as usize == i + 1);
    if let Some(i) = (0..MAX_MAPPED_FILES).find(|&i| used(i) && t.mapped_files[i].as_ref() == Some(node)) {
        return Some(i);
    }
    let i = (0..MAX_MAPPED_FILES).find(|&i| !used(i))?;
    t.mapped_files[i] = Some(node.clone());
    Some(i)
}

/// What backs the mmap page at `va`: its region's protection, and the file
/// and byte offset it reads from (`None`: zero-filled). `None` outside the
/// regions.
pub fn mmap_backing(va: usize) -> Option<(u32, Option<(crate::fs::Vnode, usize)>)> {
    let page = crate::user::PAGE;
    // An idle task or the boot task has no process, and so no mappings: a
    // fault there is the kernel's, reported as such.
    with_process_opt(|t| {
        let t = t?;
        let r = region_at(&t.mmap, va)?;
        let file = t.mapped_files.get((r.file as usize).wrapping_sub(1)).and_then(|f| {
            let off = (r.fpage as usize + (va - r.va as usize) / page) * page;
            Some((f.clone()?, off))
        });
        Some((r.prot, file))
    })
}

/// Merge neighbouring regions that continue each other (same protection
/// and backing) in the sorted table.
fn coalesce(regions: &mut Vec<MmapRegion>) {
    let page = crate::user::PAGE as u64;
    let joinable = |a: &MmapRegion, b: &MmapRegion| {
        a.prot == b.prot
            && a.file == b.file
            && (a.file == 0 || a.fpage + a.pages == b.fpage)
            && a.va + a.pages as u64 * page == b.va
    };
    regions.dedup_by(|b, a| {
        if joinable(a, b) {
            a.pages += b.pages;
            true
        } else {
            false
        }
    });
}

/// Forget `[va, va + pages)` (callers free the frames). False if the table
/// has no room for the split this needs.
pub fn mmap_remove(va: u64, pages: u32) -> bool {
    let lo = va as usize;
    let hi = lo + pages as usize * crate::user::PAGE;
    with_process_mut(|t| carve(&mut t.mmap, lo, hi).is_some())
}

/// Record `prot` for the mapped parts of `[va, va + pages)` (a device
/// mapping stays one).
pub fn mmap_set_prot(va: u64, pages: u32, prot: u32) -> bool {
    let page = crate::user::PAGE;
    let lo = va as usize;
    let hi = lo + pages as usize * page;
    with_process_mut(|t| {
        let saved = t.mmap.clone();
        let Some(parts) = carve(&mut t.mmap, lo, hi) else {
            return false;
        };
        let at = t.mmap.partition_point(|r| (r.va as usize) < lo);
        let reprotected = parts
            .into_iter()
            .map(|part| MmapRegion { prot: prot | (part.prot & MMAP_DEVICE), ..part });
        t.mmap.splice(at..at, reprotected);
        coalesce(&mut t.mmap);
        if t.mmap.len() > MAX_MMAP_REGIONS {
            t.mmap = saved;
            return false;
        }
        true
    })
}

pub(super) fn heap_base_for(base: u64, stack_off: u64) -> u64 {
    base + stack_off + (crate::user::USER_STACK_PAGES * crate::user::PAGE) as u64
}
