//! Per-task heap break and mmap region bookkeeping.

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
        match mmap
            .iter()
            .find(|r| r.pages != 0 && (r.va as usize) <= cur && cur < region_end(r))
        {
            Some(r) => cur = region_end(r),
            None => return false,
        }
    }
    true
}

/// Remove `[lo, hi)` from the table, splitting regions it cuts. Returns the
/// pieces removed (with their protections), or `None` if a split needs a
/// free entry and the table is full (nothing is changed then).
fn carve(mmap: &mut [MmapRegion; MAX_MMAP_REGIONS], lo: usize, hi: usize) -> Option<alloc::vec::Vec<(usize, usize, u32)>> {
    let page = crate::user::PAGE;
    let mut out = [EMPTY_MMAP_REGION; MAX_MMAP_REGIONS];
    let mut n = 0;
    let mut removed = alloc::vec::Vec::new();
    for r in mmap.iter().filter(|r| r.pages != 0) {
        let (rlo, rhi) = (r.va as usize, region_end(r));
        if rhi <= lo || rlo >= hi {
            out[n] = *r;
            n += 1;
            continue;
        }
        removed.push((rlo.max(lo), rhi.min(hi), r.prot));
        for (a, b) in [(rlo, lo), (hi, rhi)] {
            if a < b {
                if n == MAX_MMAP_REGIONS {
                    return None;
                }
                out[n] = MmapRegion { va: a as u64, pages: ((b - a) / page) as u32, prot: r.prot };
                n += 1;
            }
        }
    }
    *mmap = out;
    Some(removed)
}

const EMPTY_MMAP_REGION: MmapRegion = MmapRegion { va: 0, pages: 0, prot: 0 };

/// Per-task user map: (USER_BASE, IMAGE_SPAN, STACK_OFF).
pub fn current_user_map() -> (u64, usize, u64) {
    with_current_mut(|t| (t.user_base, t.image_span, t.stack_off))
}

pub fn current_brk() -> u64 {
    let flags = irq_save();
    irq_off();
    let id = current_slot();
    let b = TASKS.lock()[id].brk_cur;
    irq_restore(flags);
    b
}

pub fn set_brk(brk: u64) {
    with_current_mut(|t| t.brk_cur = brk);
}

/// Drop the current task's mmap table without freeing frames (caller already
/// reclaimed them). Used after in-place exec frees anonymous maps before
/// `expand_user_elf` so a later `reclaim_user_aspace` / exit cannot double-free.
pub fn clear_mmap() {
    with_current_mut(|t| t.mmap = EMPTY_MMAP);
}

pub fn mmap_regions() -> [MmapRegion; MAX_MMAP_REGIONS] {
    let flags = irq_save();
    irq_off();
    let id = current_slot();
    let r = TASKS.lock()[id].mmap;
    irq_restore(flags);
    r
}

pub fn mmap_contains(ptr: usize, len: usize) -> bool {
    let flags = irq_save();
    irq_off();
    let id = current_slot();
    let mmap = TASKS.lock()[id].mmap;
    irq_restore(flags);
    mmap_range_in(&mmap, ptr, len)
}

/// Lowest free `len`-byte gap in `[area_lo, area_hi)` (page aligned).
pub fn mmap_alloc(area_lo: usize, area_hi: usize, len: usize) -> Option<usize> {
    let mut regions = mmap_regions();
    regions.sort_unstable_by_key(|r| r.va);
    let mut cand = area_lo;
    for r in regions.iter().filter(|r| r.pages != 0) {
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

/// Record a new mapping. It joins an adjacent mapping with the same
/// protection when there is one (malloc implementations such as musl's map
/// many small neighbouring blocks: hundreds of mappings, few runs).
pub fn mmap_add(va: u64, pages: u32, prot: u32) -> bool {
    with_current_mut(|t| {
        let added = match t.mmap.iter_mut().find(|r| r.pages == 0) {
            Some(r) => {
                *r = MmapRegion { va, pages, prot };
                true
            }
            None => false,
        };
        let joined = coalesce(&mut t.mmap, (!added).then_some(MmapRegion { va, pages, prot }));
        added || joined
    })
}

/// Merge adjacent regions with the same protection, and fold `extra` (a
/// region that found no free slot) into a neighbour if it touches one.
/// Returns whether `extra` was folded in.
fn coalesce(regions: &mut [MmapRegion; MAX_MMAP_REGIONS], mut extra: Option<MmapRegion>) -> bool {
    let page = crate::user::PAGE as u64;
    let joinable = |a: &MmapRegion, b: &MmapRegion| {
        a.pages != 0 && b.pages != 0 && a.prot == b.prot && a.va + a.pages as u64 * page == b.va
    };
    let mut folded = false;
    let mut changed = true;
    while changed {
        changed = false;
        if let Some(x) = extra {
            for r in regions.iter_mut() {
                if joinable(r, &x) {
                    r.pages += x.pages;
                } else if joinable(&x, r) {
                    r.va = x.va;
                    r.pages += x.pages;
                } else {
                    continue;
                }
                extra = None;
                folded = true;
                changed = true;
                break;
            }
        }
        for i in 0..MAX_MMAP_REGIONS {
            for j in 0..MAX_MMAP_REGIONS {
                if i != j && joinable(&regions[i], &regions[j]) {
                    regions[i].pages += regions[j].pages;
                    regions[j] = MmapRegion { va: 0, pages: 0, prot: 0 };
                    changed = true;
                }
            }
        }
    }
    folded
}

/// Forget `[va, va + pages)` (callers free the frames). False if the table
/// has no room for the split this needs.
pub fn mmap_remove(va: u64, pages: u32) -> bool {
    let lo = va as usize;
    let hi = lo + pages as usize * crate::user::PAGE;
    with_current_mut(|t| carve(&mut t.mmap, lo, hi).is_some())
}

/// Record `prot` for the mapped parts of `[va, va + pages)`.
pub fn mmap_set_prot(va: u64, pages: u32, prot: u32) -> bool {
    let page = crate::user::PAGE;
    let lo = va as usize;
    let hi = lo + pages as usize * page;
    with_current_mut(|t| {
        let saved = t.mmap;
        let Some(parts) = carve(&mut t.mmap, lo, hi) else {
            return false;
        };
        for (a, b, _) in parts {
            let Some(r) = t.mmap.iter_mut().find(|r| r.pages == 0) else {
                t.mmap = saved;
                return false;
            };
            *r = MmapRegion { va: a as u64, pages: ((b - a) / page) as u32, prot };
        }
        coalesce(&mut t.mmap, None);
        true
    })
}

pub(super) fn heap_base_for(base: u64, stack_off: u64) -> u64 {
    base + stack_off + (crate::user::USER_STACK_PAGES * crate::user::PAGE) as u64
}
