//! Per-task heap break and mmap region bookkeeping.

use super::*;

pub(super) fn mmap_range_in(mmap: &[MmapRegion], ptr: usize, len: usize) -> bool {
    if len == 0 {
        return true;
    }
    let end = match ptr.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    for r in mmap.iter() {
        if r.pages == 0 {
            continue;
        }
        let lo = r.va as usize;
        let hi = lo.saturating_add(r.pages as usize * crate::user::PAGE);
        if ptr >= lo && end <= hi {
            return true;
        }
    }
    false
}

/// Per-task user map: (USER_BASE, IMAGE_SPAN, STACK_OFF).
pub fn current_user_map() -> (u64, usize, u64) {
    let flags = irq_save();
    irq_off();
    let id = current_slot();
    let t = TASKS.lock()[id];
    let out = (t.user_base, t.image_span, t.stack_off);
    irq_restore(flags);
    out
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
    with_current_mut(|t| {
        t.mmap = EMPTY_MMAP;
        t.mmap_next = 0;
    });
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

pub fn mmap_overlaps(va: usize, len: usize) -> bool {
    let end = va.saturating_add(len);
    let flags = irq_save();
    irq_off();
    let id = current_slot();
    let mmap = TASKS.lock()[id].mmap;
    irq_restore(flags);
    for r in mmap.iter() {
        if r.pages == 0 {
            continue;
        }
        let lo = r.va as usize;
        let hi = lo.saturating_add(r.pages as usize * crate::user::PAGE);
        if va < hi && end > lo {
            return true;
        }
    }
    false
}

pub fn mmap_alloc(area_lo: usize, area_hi: usize, len: usize) -> Option<usize> {
    with_current_mut(|t| {
        let mut next = t.mmap_next as usize;
        if next < area_lo || next == 0 {
            next = area_lo;
        }
        next = (next + crate::user::PAGE - 1) & !(crate::user::PAGE - 1);
        if next.saturating_add(len) > area_hi {
            return None;
        }
        t.mmap_next = (next + len) as u64;
        Some(next)
    })
}

pub fn mmap_add(va: u64, pages: u32, prot: u32) -> bool {
    with_current_mut(|t| {
        for r in t.mmap.iter_mut() {
            if r.pages == 0 {
                *r = MmapRegion { va, pages, prot };
                return true;
            }
        }
        false
    })
}

pub fn mmap_remove(va: u64, pages: u32) {
    with_current_mut(|t| {
        for r in t.mmap.iter_mut() {
            if r.va == va && r.pages == pages {
                *r = MmapRegion { va: 0, pages: 0, prot: 0 };
                return;
            }
        }
        // Partial unmap: drop any region fully covered.
        let lo = va;
        let hi = va.saturating_add(pages as u64 * crate::user::PAGE as u64);
        for r in t.mmap.iter_mut() {
            if r.pages == 0 {
                continue;
            }
            let rhi = r.va.saturating_add(r.pages as u64 * crate::user::PAGE as u64);
            if r.va >= lo && rhi <= hi {
                *r = MmapRegion { va: 0, pages: 0, prot: 0 };
            }
        }
    });
}

pub fn mmap_set_prot(va: u64, pages: u32, prot: u32) {
    with_current_mut(|t| {
        let lo = va;
        let hi = va.saturating_add(pages as u64 * crate::user::PAGE as u64);
        for r in t.mmap.iter_mut() {
            if r.pages == 0 {
                continue;
            }
            let rhi = r.va.saturating_add(r.pages as u64 * crate::user::PAGE as u64);
            if r.va >= lo && rhi <= hi {
                r.prot = prot;
            }
        }
    });
}

pub(super) fn heap_base_for(base: u64, stack_off: u64) -> u64 {
    base + stack_off + (crate::user::USER_STACK_PAGES * crate::user::PAGE) as u64
}
