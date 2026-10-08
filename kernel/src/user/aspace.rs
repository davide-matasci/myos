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
    // The copies use `try_alloc_frame_user`, so a fork under memory pressure
    // fails with ENOMEM (handled by the caller) instead of aborting the kernel;
    // frames taken before a failure are freed so a failed fork leaks nothing.
    let mut frames = alloc::vec![0u64; n_pages];
    let free_all = |frames: &[u64]| frames.iter().copied().filter(|&f| f != 0).for_each(mm::free_frame);
    for i in 0..n_pages {
        let va = base + (i * PAGE) as u64;
        let Some(phys) = virt_to_phys(src, va) else {
            free_all(&frames);
            return None;
        };
        let Some(dst) = mm::try_alloc_frame_user(2) else {
            free_all(&frames);
            return None;
        };
        frames[i] = dst;
        unsafe {
            core::ptr::copy_nonoverlapping(mm::hhdm(phys), mm::hhdm(dst), PAGE);
        }
        sync_icache(mm::hhdm(dst) as usize, PAGE);
    }
    let stack_va = base + stack_off;
    let mut stack_frames = [0u64; USER_STACK_PAGES];
    for i in 0..USER_STACK_PAGES {
        let Some(phys) = virt_to_phys(src, stack_va + (i * PAGE) as u64) else {
            free_all(&frames);
            free_all(&stack_frames);
            return None;
        };
        let Some(dst) = mm::try_alloc_frame_user(2) else {
            free_all(&frames);
            free_all(&stack_frames);
            return None;
        };
        stack_frames[i] = dst;
        unsafe {
            core::ptr::copy_nonoverlapping(mm::hhdm(phys), mm::hhdm(dst), PAGE);
        }
    }
    let aspace = create_aspace(&frames[..n_pages], &stack_frames, base, stack_off);
    let heap_base = heap_base_va(base, stack_off);
    let heap_end = align_up_usize(brk_cur as usize, PAGE);
    let mut va = heap_base as usize;
    while va < heap_end {
        if let Some(phys) = virt_to_phys(src, va as u64) {
            // The code and stack frames now belong to `aspace`; on failure
            // reclaim the whole partial child (heap mapped so far included).
            let Some(dst) = mm::try_alloc_frame_user(2) else {
                reclaim_user_aspace(aspace, base, span, stack_off, va as u64, &[]);
                return None;
            };
            unsafe {
                core::ptr::copy_nonoverlapping(mm::hhdm(phys), mm::hhdm(dst), PAGE);
            }
            map_heap_page(aspace, va as u64, dst);
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
                // A device's pages are shared with the child, not copied,
                // and so are a file's from the page cache (a shared
                // mapping's, writable, among them).
                if r.prot & task::MMAP_DEVICE != 0 || fs::pagecache::share(phys) {
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
/// Frames unmapped from an address space that another CPU has loaded (a
/// thread of the process runs there), with that address space: the next
/// [`flush_user_tlb`] of it frees them, once no CPU can reach them, or
/// [`retire_aspace`] when the address space itself goes.
static UNMAPPED: Mutex<alloc::vec::Vec<(u64, u64)>> = Mutex::new(alloc::vec::Vec::new());

/// After a change that only added mappings (none removed, narrowed or
/// moved): no CPU can hold a translation that is now wrong, as x86_64 and
/// aarch64 keep no missing ones, so no other CPU is asked to flush. riscv64
/// may keep a missing translation: there it is [`flush_user_tlb`].
///
/// A multithreaded process grows its heap and maps memory all the time; a
/// shootdown each time waits for every CPU, some spinning on a lock with
/// interrupts off, and slowed the whole system to a crawl.
pub(super) fn flush_user_tlb_added() {
    if cfg!(target_arch = "riscv64") {
        flush_user_tlb();
    } else {
        crate::arch::flush_tlb_local();
    }
}

/// Flush the current address space's user translations (on every CPU that
/// has it loaded), then free the frames unmapped from it before.
pub(super) fn flush_user_tlb() {
    let aspace = task::current_aspace();
    // Taken before the flush: it covers only what was unmapped by then.
    let mut freed = alloc::vec::Vec::new();
    let flags = crate::arch::irq_save();
    crate::arch::irq_off();
    UNMAPPED.lock().retain(|&(a, frame)| {
        if a == aspace {
            freed.push(frame);
        }
        a != aspace
    });
    crate::arch::irq_restore(flags);
    crate::arch::upaging::flush_user_tlb();
    for frame in freed {
        release_frame(frame);
    }
}

/// A user frame no mapping of this one uses any more: back to the page
/// cache when it is one of its frames, freed otherwise.
pub(super) fn release_frame(frame: u64) {
    if !fs::pagecache::release(frame) {
        mm::free_frame(frame);
    }
}

/// Unmap and free anonymous mmap pages for `aspace` (table entries are left
/// to the caller); a device's pages are only unmapped. Shared by in-place
/// exec and [`reclaim_user_aspace`].
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
            release_mmap_page(aspace, va, r.prot & task::MMAP_DEVICE != 0);
        }
    }
}

/// Unmap the mmap pages `[va, va + pages)` as `mmap` (the table before the
/// range was removed from it) describes them: freed, or a device's left to it.
pub(super) fn release_mmap_range(aspace: u64, mmap: &[task::MmapRegion], va: u64, pages: usize) {
    for i in 0..pages {
        let page_va = va + (i * PAGE) as u64;
        // Most pages of a lazy mapping were never touched.
        if virt_to_phys(aspace, page_va).is_none() {
            continue;
        }
        let device = mmap.iter().any(|r| {
            r.pages != 0
                && r.prot & task::MMAP_DEVICE != 0
                && r.va <= page_va
                && page_va < r.va + r.pages as u64 * PAGE as u64
        });
        release_mmap_page(aspace, page_va, device);
    }
}

fn release_mmap_page(aspace: u64, va: u64, device: bool) {
    if device {
        unmap_user_page(aspace, va);
    } else {
        free_mapped_page(aspace, va);
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
    flush_user_tlb();
    retire_aspace(aspace);
}

/// Reclaimed address spaces some CPU still had loaded (`unload_user_aspace`
/// stopped waiting for it): what is left of them is freed by a later
/// [`retire_aspace`], once no CPU has them loaded.
static RETIRED: Mutex<alloc::vec::Vec<u64>> = Mutex::new(alloc::vec::Vec::new());

/// Free what is left of the reclaimed `aspace`, its data frames deferred in
/// [`UNMAPPED`] and its page tables, once no CPU has it loaded: at once in
/// the usual case, else at a later reclaim. A CPU that has an address space
/// loaded may walk its tables at any time (speculatively too): tables freed
/// under it were recycled while it still read them (a kernel data abort in
/// `finish_switch` on aarch64). The process's own task no longer names it
/// (`die`, exec), so no CPU loads it again.
fn retire_aspace(aspace: u64) {
    let mut ready = alloc::vec::Vec::new();
    let mut frames = alloc::vec::Vec::new();
    let flags = crate::arch::irq_save();
    crate::arch::irq_off();
    {
        let mut retired = RETIRED.lock();
        retired.push(aspace);
        retired.retain(|&a| {
            if task::aspace_loaded_anywhere(a) {
                return true;
            }
            ready.push(a);
            false
        });
    }
    UNMAPPED.lock().retain(|&(a, frame)| {
        if ready.contains(&a) {
            frames.push(frame);
            return false;
        }
        true
    });
    crate::arch::irq_restore(flags);
    for a in ready {
        free_user_page_tables(a);
    }
    for frame in frames {
        release_frame(frame);
    }
}

/// How a fault touched a page.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    Exec,
}

/// Two threads of a process touching the same new page must not both map it.
static FAULT_LOCK: Mutex<()> = Mutex::new(());

/// Page in the mmap page at `va` of the current process on its first touch,
/// from userspace (a page fault) or from a kernel copy: a zeroed frame,
/// filled from the file that backs the region. False when `va` is in no
/// region or the region's protection forbids `access`: a real fault.
pub fn fault_in(va: usize, access: Access) -> bool {
    let page = va & !(PAGE - 1);
    let Some((prot, file)) = task::mmap_backing(page) else {
        return false;
    };
    // A device's pages are mapped with the region, never paged in.
    if prot & task::MMAP_DEVICE != 0 {
        return false;
    }
    let prot = prot as usize;
    let allowed = match access {
        Access::Read => prot != 0,
        Access::Write => prot & PROT_WRITE != 0,
        Access::Exec => prot & PROT_EXEC != 0,
    };
    if !allowed {
        return false;
    }
    let aspace = task::current_aspace();
    // The new page is filled before the lock is taken: a file read is a
    // disk request, which every other fault would wait for.
    //
    // A writable (anonymous or private) page is the memory a hog grows without
    // bound, so it comes from `try_alloc_frame_user`: once that would eat into
    // the kernel's reserve it returns None, and the fault fails (SIGSEGV to the
    // faulting process) instead of letting the frame allocator abort the whole
    // kernel. The read-only shared page is a program's code/rodata, bounded by
    // its size and kept safe by that same reserve; a shared mapping's page
    // is the file's, bounded by the files mapped.
    let fresh = if virt_to_phys(aspace, page as u64).is_none() {
        let frame = match &file {
            Some((node, off)) if prot & task::MMAP_SHARED as usize != 0 => {
                fs::pagecache::map_shared(node, off / PAGE, prot & PROT_WRITE != 0)
            }
            Some((node, off)) if prot & PROT_WRITE == 0 => fs::pagecache::map(node, off / PAGE),
            // A private copy of it (zero past the end of the file).
            Some((node, off)) => {
                let Some(frame) = mm::try_alloc_frame_user(4) else {
                    return false;
                };
                let dst = unsafe { core::slice::from_raw_parts_mut(mm::hhdm(frame), PAGE) };
                fs::pagecache::copy(node, off / PAGE, dst);
                frame
            }
            None => match mm::try_alloc_frame_user(4) {
                Some(frame) => frame,
                None => return false,
            },
        };
        if prot & PROT_EXEC != 0 {
            sync_icache(mm::hhdm(frame) as usize, PAGE);
        }
        Some(frame)
    } else {
        None
    };
    let flags = crate::arch::irq_save();
    crate::arch::irq_off();
    let guard = FAULT_LOCK.lock();
    // Another thread may have paged it in meanwhile: then only the
    // protection is (re)applied.
    let frame = match (virt_to_phys(aspace, page as u64), fresh) {
        (Some(mapped), fresh) => {
            if let Some(frame) = fresh {
                release_frame(frame);
            }
            Some(mapped)
        }
        (None, fresh) => fresh,
    };
    if let Some(frame) = frame {
        map_user_page_prot(aspace, page as u64, frame, prot);
        // The page was not mapped before: no other CPU can hold a
        // translation for it (one that faults on it meanwhile flushes its
        // own here), so this CPU's entry for it is all there is to drop.
        crate::arch::flush_tlb_page_local(page);
    }
    drop(guard);
    crate::arch::irq_restore(flags);
    // Mapped when looked at first and gone by the lock (unmapped by
    // another thread): look again.
    frame.is_some() || fault_in(va, access)
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
    // Another CPU may still hold a translation to it: freed after the flush.
    if task::aspace_loaded_elsewhere(aspace) {
        let flags = crate::arch::irq_save();
        crate::arch::irq_off();
        UNMAPPED.lock().push((aspace, phys));
        crate::arch::irq_restore(flags);
        return;
    }
    release_frame(phys);
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
