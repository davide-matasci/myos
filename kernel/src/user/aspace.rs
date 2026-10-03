//! User address spaces: per-arch page tables (create, map, translate,
//! copy for fork, free), the user VA layout, and TLB / I-cache maintenance.

use super::*;

/// Copy this process's user code+stack+heap pages into a new aspace at the same VA.
pub fn copy_user_aspace(base: u64, span: usize, stack_off: u64, brk_cur: u64) -> Option<u64> {
    let n_pages = span.div_ceil(PAGE);
    if n_pages == 0 || n_pages > MAX_ELF_PAGES {
        return None;
    }
    let src = task::current_aspace();
    if src == 0 {
        return None;
    }
    // Heap, not kstack: MAX_ELF_PAGES×8 ≈ 9KiB on every fork's syscall stack.
    let mut frames = alloc::vec![0u64; n_pages];
    for i in 0..n_pages {
        let va = base + (i * PAGE) as u64;
        let phys = virt_to_phys(src, va)?;
        frames[i] = mm::alloc_frame_site(2);
        unsafe {
            core::ptr::copy_nonoverlapping(mm::hhdm(phys), mm::hhdm(frames[i]), PAGE);
        }
        sync_icache(mm::hhdm(frames[i]) as usize, PAGE);
    }
    let stack_va = base + stack_off;
    let mut stack_frames = [0u64; USER_STACK_PAGES];
    for i in 0..USER_STACK_PAGES {
        let phys = virt_to_phys(src, stack_va + (i * PAGE) as u64)?;
        stack_frames[i] = mm::alloc_frame_site(2);
        unsafe {
            core::ptr::copy_nonoverlapping(mm::hhdm(phys), mm::hhdm(stack_frames[i]), PAGE);
        }
    }
    let aspace = create_aspace(&frames[..n_pages], &stack_frames, base, stack_off);
    let heap_base = heap_base_va(base, stack_off);
    let heap_end = align_up_usize(brk_cur as usize, PAGE);
    let mut va = heap_base as usize;
    while va < heap_end {
        if virt_to_phys(src, va as u64).is_some() {
            let phys = mm::alloc_frame_site(2);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    mm::hhdm(virt_to_phys(src, va as u64)?),
                    mm::hhdm(phys),
                    PAGE,
                );
            }
            map_heap_page(aspace, va as u64, phys);
        }
        va += PAGE;
    }
    copy_mmap_pages(src, aspace);
    Some(aspace)
}

pub(super) fn heap_base_va(base: u64, stack_off: u64) -> u64 {
    base + stack_off + (USER_STACK_PAGES * PAGE) as u64
}

pub(super) fn heap_limit_va(base: u64, stack_off: u64) -> u64 {
    heap_base_va(base, stack_off) + (HEAP_PAGES * PAGE) as u64
}

pub(super) fn mmap_base_va(base: u64, stack_off: u64) -> u64 {
    heap_limit_va(base, stack_off)
}

pub(super) fn mmap_limit_va(base: u64, stack_off: u64) -> u64 {
    mmap_base_va(base, stack_off) + (MMAP_AREA_PAGES * PAGE) as u64
}

fn copy_mmap_pages(src: u64, dst: u64) {
    let regions = task::mmap_regions();
    for r in regions.iter() {
        if r.pages == 0 {
            continue;
        }
        let mut va = r.va;
        let end = r.va.saturating_add(r.pages as u64 * PAGE as u64);
        while va < end {
            if let Some(phys) = virt_to_phys(src, va) {
                // A framebuffer mapping is shared with the child, not copied.
                if crate::fb::is_frame(phys) {
                    map_user_page_prot(dst, va, phys, r.prot as usize);
                    va += PAGE as u64;
                    continue;
                }
                let frame = mm::alloc_frame_site(2);
                unsafe {
                    core::ptr::copy_nonoverlapping(mm::hhdm(phys), mm::hhdm(frame), PAGE);
                }
                if r.prot & PROT_EXEC as u32 != 0 {
                    sync_icache(mm::hhdm(frame) as usize, PAGE);
                }
                map_user_page_prot(dst, va, frame, r.prot as usize);
            }
            va += PAGE as u64;
        }
    }
}

pub(super) fn align_up_usize(v: usize, align: usize) -> usize {
    (v + align - 1) & !(align - 1)
}

pub(super) use crate::arch::upaging::{create_aspace, free_user_page_tables, map_heap_page, map_user_code_page, map_user_stack_page, pick_user_base, unmap_user_page, virt_to_phys};
pub use crate::arch::upaging::{read_aspace, switch_aspace};
pub(super) use crate::arch::upaging::flush_user_tlb;

/// Unmap and free anonymous mmap pages for `aspace` (table entries are left
/// to the caller). Shared by in-place exec and [`reclaim_user_aspace`].
pub(super) fn free_mmap_regions(aspace: u64, mmap: &[task::MmapRegion]) {
    for r in mmap.iter() {
        if r.pages == 0 || r.va == 0 {
            continue;
        }
        // Concurrent SMP float once saw a torn/corrupt region and
        // `va + i*PAGE` overflow-panicked in debug. Bound + checked math.
        let pages = (r.pages as usize).min(MMAP_AREA_PAGES);
        for i in 0..pages {
            let Some(off) = (i as u64).checked_mul(PAGE as u64) else {
                break;
            };
            let Some(va) = r.va.checked_add(off) else {
                break;
            };
            free_mapped_page(aspace, va);
        }
    }
}

/// Free user data frames for `aspace` (code / stack / heap / mmap), then the
/// private page tables that owned them.
///
/// Used on process exit and when `load_user_elf` abandons a prior aspace so
/// `/heap` can fork+exec large ELFs repeatedly without freelist exhaustion
/// (riscv64 `sepc=0` after find+cat+ls). Leaving Sv39 mid/leaf/root tables
/// allocated used to leak several frames per fork forever — the remaining
/// "random" riscv OOM class after data-page reclaim was patched. x86 likewise
/// leaked its private PML4[1] PDPT/PD/PT tree (and any orphan leaves outside
/// the windowed walks) until `free_user_page_tables_x86` mirrored that teardown.
pub fn reclaim_user_aspace(
    aspace: u64,
    base: u64,
    image_span: usize,
    stack_off: u64,
    brk_cur: u64,
    mmap: &[task::MmapRegion],
) {
    if aspace == 0 || base == 0 {
        return;
    }
    // Must not free pages while they may still be walked via this aspace.
    task::unload_user_aspace(aspace);
    let n_code = image_span.div_ceil(PAGE).min(MAX_ELF_PAGES);
    for i in 0..n_code {
        let Some(va) = base.checked_add((i * PAGE) as u64) else {
            break;
        };
        free_mapped_page(aspace, va);
    }
    for i in 0..USER_STACK_PAGES {
        let Some(va) = base
            .checked_add(stack_off)
            .and_then(|s| s.checked_add((i * PAGE) as u64))
        else {
            break;
        };
        free_mapped_page(aspace, va);
    }
    let heap_base = heap_base_va(base, stack_off);
    let heap_end = if brk_cur > heap_base {
        align_up_u64(brk_cur, PAGE as u64)
    } else {
        heap_base
    };
    let heap_lim = heap_limit_va(base, stack_off);
    // Heap pages only exist below brk (sys_brk frees on shrink; nothing maps
    // the rest of the window), so stop there: walking all HEAP_PAGES (4096)
    // on every exit was most of the exit cost in the debug kernel.
    let mut va = heap_base;
    while va < heap_end.min(heap_lim) {
        free_mapped_page(aspace, va);
        va += PAGE as u64;
    }
    free_mmap_regions(aspace, mmap);
    free_user_page_tables(aspace);
    flush_user_tlb();
}

pub(super) fn free_mapped_page(aspace: u64, va: u64) {
    let Some(phys) = virt_to_phys(aspace, va) else {
        return;
    };
    unmap_user_page(aspace, va);
    // If unmap failed to clear, refuse to free — avoids freelist double-free when
    // reclaim walks overlapping VA ranges (code span vs heap/mmap).
    if virt_to_phys(aspace, va).is_some() {
        return;
    }
    // Device memory (the framebuffer) is not the allocator's to take back.
    if crate::fb::is_frame(phys) {
        return;
    }
    mm::free_frame(phys);
}

fn align_up_u64(x: u64, a: u64) -> u64 {
    (x + a - 1) & !(a - 1)
}

pub(super) fn map_user_page_prot(aspace: u64, va: u64, pa: u64, prot: usize) {
    crate::arch::upaging::map_user_page_prot(aspace, va, pa, prot & PROT_WRITE != 0, prot & PROT_EXEC != 0);
}

/// Make freshly written user code visible to instruction fetch.
pub(super) fn sync_icache(start: usize, size: usize) {
    crate::arch::sync_icache(start, size);
}
