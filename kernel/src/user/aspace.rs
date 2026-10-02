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

pub(super) fn virt_to_phys(aspace: u64, va: u64) -> Option<u64> {
    #[cfg(target_arch = "x86_64")]
    {
        virt_to_phys_x86(aspace, va)
    }
    #[cfg(target_arch = "aarch64")]
    {
        virt_to_phys_aarch64(aspace, va)
    }
    #[cfg(target_arch = "riscv64")]
    {
        virt_to_phys_riscv64(aspace, va)
    }
}

#[cfg(target_arch = "x86_64")]
fn virt_to_phys_x86(pml4_phys: u64, va: u64) -> Option<u64> {
    const PRESENT: u64 = 1;
    const HUGE: u64 = 1 << 7;
    let i4 = ((va >> 39) & 0x1ff) as usize;
    let i3 = ((va >> 30) & 0x1ff) as usize;
    let i2 = ((va >> 21) & 0x1ff) as usize;
    let i1 = ((va >> 12) & 0x1ff) as usize;
    unsafe {
        let pml4 = &*mm::table(pml4_phys);
        if pml4[i4] & PRESENT == 0 {
            return None;
        }
        let pdpt = &*mm::table(pml4[i4]);
        if pdpt[i3] & PRESENT == 0 || pdpt[i3] & HUGE != 0 {
            return None;
        }
        let pd = &*mm::table(pdpt[i3]);
        if pd[i2] & PRESENT == 0 || pd[i2] & HUGE != 0 {
            return None;
        }
        let pt = &*mm::table(pd[i2]);
        if pt[i1] & PRESENT == 0 {
            return None;
        }
        Some(pt[i1] & 0x000f_ffff_ffff_f000)
    }
}

#[cfg(target_arch = "aarch64")]
fn aarch64_user_page_idx(va: u64) -> usize {
    let base = USER_BASE.load(Ordering::SeqCst);
    va.saturating_sub(base) as usize / PAGE
}

/// Return the L3 table for `page` (allocating L2/L3 spill slots as needed).
#[cfg(target_arch = "aarch64")]
fn aarch64_l3_table_mut(l0_phys: u64, page: usize) -> Option<*mut [u64; 512]> {
    const TABLE: u64 = 0b11;
    const PA: u64 = 0x0000_FFFF_FFFF_F000;
    let l2_idx = page / AARCH64_USER_L3_PAGES;
    if l2_idx >= AARCH64_USER_L2_TABLES {
        return None;
    }
    unsafe {
        let l0 = &*mm::table(l0_phys);
        let l1_phys = l0[0] & PA;
        if l1_phys == 0 {
            return None;
        }
        let l1 = &*mm::table(l1_phys);
        let l2_phys = l1[1] & PA;
        if l2_phys == 0 {
            return None;
        }
        let l2 = &mut *mm::table(l2_phys);
        if l2[l2_idx] & 0b11 != TABLE {
            let l3 = mm::alloc_frame_site(3);
            l2[l2_idx] = l3 | TABLE;
        }
        Some(mm::table(l2[l2_idx] & PA))
    }
}

#[cfg(target_arch = "aarch64")]
fn virt_to_phys_aarch64(l0_phys: u64, va: u64) -> Option<u64> {
    const PA: u64 = 0x0000_FFFF_FFFF_F000;
    let page = aarch64_user_page_idx(va);
    let l3_idx = page % AARCH64_USER_L3_PAGES;
    let l2_idx = page / AARCH64_USER_L3_PAGES;
    if l2_idx >= AARCH64_USER_L2_TABLES {
        return None;
    }
    unsafe {
        let l0 = &*mm::table(l0_phys);
        let l1_phys = l0[0] & PA;
        if l1_phys == 0 {
            return None;
        }
        let l1 = &*mm::table(l1_phys);
        let l2_phys = l1[1] & PA;
        if l2_phys == 0 {
            return None;
        }
        let l2 = &*mm::table(l2_phys);
        let l3_phys = l2[l2_idx] & PA;
        if l3_phys == 0 {
            return None;
        }
        let l3 = &*mm::table(l3_phys);
        let pte = l3[l3_idx];
        if pte & 0b11 != 0b11 {
            return None;
        }
        Some(pte & PA)
    }
}

#[cfg(target_arch = "riscv64")]
fn satp_ppn(satp: u64) -> u64 {
    paging::satp_root_phys(satp)
}

#[cfg(target_arch = "riscv64")]
fn make_satp(root_phys: u64) -> u64 {
    paging::make_satp(root_phys)
}

#[cfg(target_arch = "riscv64")]
fn virt_to_phys_riscv64(satp: u64, va: u64) -> Option<u64> {
    let root_phys = satp_ppn(satp);
    let i2 = ((va >> 30) & 0x1ff) as usize;
    let i1 = ((va >> 21) & 0x1ff) as usize;
    let i0 = ((va >> 12) & 0x1ff) as usize;
    unsafe {
        let root = &*mm::table(root_phys);
        let mid_pte = root[i2];
        if mid_pte & paging::PTE_V == 0 {
            return None;
        }
        if mid_pte & (paging::PTE_R | paging::PTE_W | paging::PTE_X) != 0 {
            // A leaf mapping at L2 with a zeroed physical address is corruption
            // (never legitimate: phys 0 is not RAM we hand out). Reject it so
            // callers see a fault instead of dereferencing hhdm(0) in kernel.
            let phys = paging::pte_phys(mid_pte);
            if phys == 0 {
                return None;
            }
            // Sv39 L2 leaf = 1 GiB page: offset is VPN[1]|VPN[0]|page-off.
            return Some(phys | (va & 0x3FFF_FFFF));
        }
        let mid = &*mm::table(paging::pte_phys(mid_pte));
        let leaf_pte = mid[i1];
        if leaf_pte & paging::PTE_V == 0 {
            return None;
        }
        if leaf_pte & (paging::PTE_R | paging::PTE_W | paging::PTE_X) != 0 {
            // Same guard: V set with a zeroed physical address is a corrupt
            // descriptor; report unmapped so callers (sys_brk) re-map cleanly.
            let phys = paging::pte_phys(leaf_pte);
            if phys == 0 {
                return None;
            }
            // Sv39 L1 leaf = 2 MiB page: offset is VPN[0]|page-off.
            return Some(phys | (va & 0x1F_FFFF));
        }
        let leaf = &*mm::table(paging::pte_phys(leaf_pte));
        let pte = leaf[i0];
        if pte & paging::PTE_V == 0 {
            return None;
        }
        // Must be a leaf PTE (any of R/W/X). A table descriptor here would make
        // reclaim free_frame a page-table phys as if it were user data.
        if pte & (paging::PTE_R | paging::PTE_W | paging::PTE_X) == 0 {
            return None;
        }
        let phys = paging::pte_phys(pte);
        if phys == 0 {
            // V set with a zeroed physical address is never a legitimate user
            // mapping. Treat as absent so callers re-map cleanly instead of
            // the kernel dereferencing hhdm(0) (riscv64 CI kernel page fault).
            return None;
        }
        Some(phys)
    }
}

pub(super) fn pick_user_base() -> u64 {
    // Prefer DEFAULT (PML4[1]). If Limine already occupied that slot (common on
    // UEFI), pick another free low-half slot. create_aspace_x86 clears *this*
    // index after the kernel PML4 clone, and free_user_page_tables_x86 tears
    // down the same index via USER_BASE — never hardcode slot 1 for both map
    // and reclaim while pick walks away from it.
    #[cfg(target_arch = "x86_64")]
    {
        let src = task::kernel_aspace() & !0xfff;
        let pml4 = unsafe { &*mm::table(src) };
        if pml4[1] == 0 {
            return DEFAULT_USER_BASE;
        }
        for i in 1..256 {
            if pml4[i] == 0 {
                return (i as u64) << 39;
            }
        }
        panic!("no free PML4 slot for user");
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        DEFAULT_USER_BASE
    }
}

pub fn read_aspace() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let c: u64;
        core::arch::asm!(
            "mov {c}, cr3",
            c = out(reg) c,
            options(nomem, nostack, preserves_flags)
        );
        c
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let t: u64;
        core::arch::asm!(
            "mrs {t}, ttbr0_el1",
            t = out(reg) t,
            options(nomem, nostack, preserves_flags)
        );
        t
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        let t: u64;
        core::arch::asm!(
            "csrr {t}, satp",
            t = out(reg) t,
            options(nomem, nostack, preserves_flags)
        );
        t
    }
}

pub fn switch_aspace(aspace: u64) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!(
            "mov cr3, {a}",
            a = in(reg) aspace,
            options(nostack, preserves_flags)
        );
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!(
            "msr ttbr0_el1, {a}",
            "dsb sy",
            a = in(reg) aspace,
            options(nostack),
        );
        if current_el() >= 2 {
            core::arch::asm!("tlbi alle2is", options(nostack));
        } else {
            core::arch::asm!("tlbi vmalle1", options(nostack));
        }
        core::arch::asm!("dsb sy; isb", options(nostack));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!(
            "csrw satp, {a}",
            "sfence.vma",
            a = in(reg) aspace,
            options(nostack),
        );
    }
}

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

/// Free private user page tables for an abandoned aspace.
///
/// Data pages must already be unmapped/freed. Kernel table frames that were
/// only *referenced* by a copied root (x86/aarch64/riscv root clone) are not
/// freed — only tables allocated for this aspace.
fn free_user_page_tables(aspace: u64) {
    #[cfg(target_arch = "riscv64")]
    free_user_page_tables_riscv(aspace);
    #[cfg(target_arch = "x86_64")]
    free_user_page_tables_x86(aspace);
    #[cfg(target_arch = "aarch64")]
    let _ = aspace;
}

/// Tear down 4-level tables owned by a user aspace (x86_64).
///
/// `create_aspace_x86` clones the kernel PML4 then allocates a private PDPT/PD/PT
/// tree under PML4[1] (VA `0x80_0000_0000`). Free any remaining present leaf
/// pages under that index (catches orphans outside the code/stack/heap/mmap
/// windows), then free the private tables and the cloned PML4. Other PML4
/// slots are value-copies of kernel entries and must not be freed.
#[cfg(target_arch = "x86_64")]
fn free_user_page_tables_x86(pml4_phys: u64) {
    const PRESENT: u64 = 1;
    const HUGE: u64 = 1 << 7;
    const PHYS_MASK: u64 = 0x000f_ffff_ffff_f000;
    // Tear down the slot that holds USER_BASE (pinned to PML4[1] / DEFAULT).
    let user_pml4_idx = ((USER_BASE.load(Ordering::SeqCst) >> 39) & 0x1ff) as usize;
    if pml4_phys == 0 {
        return;
    }
    let k = task::kernel_aspace() & !0xfff;
    if pml4_phys == k {
        return;
    }
    unsafe {
        let pml4 = &mut *mm::table(pml4_phys);
        let pml4e = pml4[user_pml4_idx];
        if pml4e & PRESENT != 0 && pml4e & HUGE == 0 {
            let pdpt_phys = pml4e & PHYS_MASK;
            let pdpt = &mut *mm::table(pdpt_phys);
            for i3 in 0..512 {
                let pdpte = pdpt[i3];
                if pdpte & PRESENT == 0 || pdpte & HUGE != 0 {
                    continue;
                }
                let pd_phys = pdpte & PHYS_MASK;
                let pd = &mut *mm::table(pd_phys);
                for i2 in 0..512 {
                    let pde = pd[i2];
                    if pde & PRESENT == 0 || pde & HUGE != 0 {
                        continue;
                    }
                    let pt_phys = pde & PHYS_MASK;
                    let pt = &mut *mm::table(pt_phys);
                    for i1 in 0..512 {
                        let pte = pt[i1];
                        if pte & PRESENT != 0 {
                            // Orphan leaf still present after windowed reclaim.
                            mm::free_frame(pte & PHYS_MASK);
                            pt[i1] = 0;
                        }
                    }
                    mm::free_frame(pt_phys);
                    pd[i2] = 0;
                }
                mm::free_frame(pd_phys);
                pdpt[i3] = 0;
            }
            mm::free_frame(pdpt_phys);
            pml4[user_pml4_idx] = 0;
        }
        mm::free_frame(pml4_phys);
    }
}

/// Tear down Sv39 tables owned by a user aspace.
///
/// `create_aspace_riscv64` clones the kernel root then clears `root[1]` and
/// allocates private mid/leaf tables under VPN[2]=1 (VA `0x4000_0000`). Only
/// that private tree is freed (leaf → mid → root). Other root slots are
/// value-copies of kernel PTEs and must not be freed.
#[cfg(target_arch = "riscv64")]
fn free_user_page_tables_riscv(satp: u64) {
    let root_phys = paging::satp_root_phys(satp);
    if root_phys == 0 {
        return;
    }
    // Never free the live kernel root.
    if root_phys == paging::satp_root_phys(task::kernel_aspace()) {
        return;
    }
    unsafe {
        let root = &mut *mm::table(root_phys);
        // User image lives in root[1] (DEFAULT_USER_BASE = 0x4000_0000).
        const USER_ROOT_IDX: usize = 1;
        let mid_pte = root[USER_ROOT_IDX];
        if mid_pte & paging::PTE_V != 0 && paging::pte_is_table(mid_pte) {
            let mid_phys = paging::pte_phys(mid_pte);
            let mid = &mut *mm::table(mid_phys);
            for i in 0..512 {
                let leaf_pte = mid[i];
                if leaf_pte & paging::PTE_V != 0 && paging::pte_is_table(leaf_pte) {
                    mm::free_frame(paging::pte_phys(leaf_pte));
                    mid[i] = 0;
                }
            }
            mm::free_frame(mid_phys);
            root[USER_ROOT_IDX] = 0;
        }
        // Private root copy (kernel PTEs were value-copies into this frame).
        mm::free_frame(root_phys);
    }
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
    mm::free_frame(phys);
}

fn align_up_u64(x: u64, a: u64) -> u64 {
    (x + a - 1) & !(a - 1)
}

pub(super) fn create_aspace(code: &[u64], stack: &[u64], base: u64, stack_off: u64) -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        create_aspace_x86(code, stack, base, stack_off)
    }
    #[cfg(target_arch = "aarch64")]
    {
        create_aspace_aarch64(code, stack, base, stack_off)
    }
    #[cfg(target_arch = "riscv64")]
    {
        create_aspace_riscv64(code, stack, base, stack_off)
    }
}

#[cfg(target_arch = "x86_64")]
fn create_aspace_x86(code: &[u64], stack: &[u64], base: u64, stack_off: u64) -> u64 {
    const PRESENT: u64 = 1;
    const WRITE: u64 = 1 << 1;
    const USER: u64 = 1 << 2;
    const NX: u64 = 1 << 63;

    let src = task::kernel_aspace() & !0xfff;
    let pml4_phys = mm::alloc_frame_site(3);
    unsafe {
        let src_t = &*mm::table(src);
        let dst_t = &mut *mm::table(pml4_phys);
        dst_t.copy_from_slice(src_t);
        // Clear the USER_BASE PML4 slot so ensure_user owns a private PDPT
        // tree (same discipline as create_aspace_riscv64). Must match
        // pick_user_base() — hardcoding [1] while pick moved to another slot
        // left reclaim freeing the wrong tree (UEFI OOM) or clearing a
        // Limine-owned slot still needed on the user CR3 (early #PF).
        let user_idx = ((USER_BASE.load(Ordering::SeqCst) >> 39) & 0x1ff) as usize;
        dst_t[user_idx] = 0;
    }

    // RW so sys_read can fill PT_LOAD (user/ok MSG_BUF). Still executable.
    for (i, &phys) in code.iter().enumerate() {
        map_page_x86(
            pml4_phys,
            base + (i * PAGE) as u64,
            phys,
            PRESENT | WRITE | USER,
        );
    }
    let stack_va = base + stack_off;
    for (i, &phys) in stack.iter().enumerate() {
        map_page_x86(
            pml4_phys,
            stack_va + (i * PAGE) as u64,
            phys,
            PRESENT | WRITE | USER | NX,
        );
    }
    pml4_phys
}

pub(super) fn map_user_page_prot(aspace: u64, va: u64, pa: u64, prot: usize) {
    let w = prot & PROT_WRITE != 0;
    let x = prot & PROT_EXEC != 0;
    #[cfg(target_arch = "x86_64")]
    {
        const PRESENT: u64 = 1;
        const WRITE: u64 = 1 << 1;
        const USER: u64 = 1 << 2;
        const NX: u64 = 1 << 63;
        let mut flags = PRESENT | USER;
        if w {
            flags |= WRITE;
        }
        if !x {
            flags |= NX;
        }
        map_page_x86(aspace, va, pa, flags);
    }
    #[cfg(target_arch = "aarch64")]
    {
        const PAGE_DESC: u64 = 0b11;
        const SH_INNER: u64 = 0b11 << 8;
        const AF: u64 = 1 << 10;
        const AP_RW: u64 = 0b01 << 6;
        const AP_RO: u64 = 0b11 << 6;
        const PXN: u64 = 1 << 53;
        const UXN: u64 = 1 << 54;
        const PA: u64 = 0x0000_FFFF_FFFF_F000;
        let page = aarch64_user_page_idx(va);
        let l3_idx = page % AARCH64_USER_L3_PAGES;
        let Some(l3) = aarch64_l3_table_mut(aspace, page) else {
            return;
        };
        unsafe {
            let l3_t = &mut *l3;
            // UXN=0 iff PROT_EXEC so EL0 can fetch (anonymous RW→RX JIT).
            let mut ent = PAGE_DESC | (pa & PA) | SH_INNER | AF | PXN;
            ent |= if w { AP_RW } else { AP_RO };
            if !x {
                ent |= UXN;
            }
            l3_t[l3_idx] = ent;
        }
    }
    #[cfg(target_arch = "riscv64")]
    {
        let mut flags =
            paging::PTE_V | paging::PTE_U | paging::PTE_A | paging::PTE_D | paging::PTE_R;
        if w {
            flags |= paging::PTE_W;
        }
        if x {
            flags |= paging::PTE_X;
        }
        let i0 = ((va >> 12) & 0x1ff) as usize;
        let leaf = ensure_riscv_leaf(aspace, va);
        unsafe {
            (*leaf)[i0] = paging::pte_leaf_4k(pa, flags);
        }
    }
}

pub(super) fn unmap_user_page(aspace: u64, va: u64) {
    #[cfg(target_arch = "x86_64")]
    {
        const PRESENT: u64 = 1;
        const HUGE: u64 = 1 << 7;
        let i4 = ((va >> 39) & 0x1ff) as usize;
        let i3 = ((va >> 30) & 0x1ff) as usize;
        let i2 = ((va >> 21) & 0x1ff) as usize;
        let i1 = ((va >> 12) & 0x1ff) as usize;
        unsafe {
            let pml4 = &*mm::table(aspace);
            if pml4[i4] & PRESENT == 0 {
                return;
            }
            let pdpt = &*mm::table(pml4[i4]);
            if pdpt[i3] & PRESENT == 0 || pdpt[i3] & HUGE != 0 {
                return;
            }
            let pd = &*mm::table(pdpt[i3]);
            if pd[i2] & PRESENT == 0 || pd[i2] & HUGE != 0 {
                return;
            }
            let pt = &mut *mm::table(pd[i2]);
            pt[i1] = 0;
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        let page = aarch64_user_page_idx(va);
        let l3_idx = page % AARCH64_USER_L3_PAGES;
        let Some(l3) = aarch64_l3_table_mut(aspace, page) else {
            return;
        };
        unsafe {
            (*l3)[l3_idx] = 0;
        }
    }
    #[cfg(target_arch = "riscv64")]
    {
        let i0 = ((va >> 12) & 0x1ff) as usize;
        let leaf = ensure_riscv_leaf(aspace, va);
        unsafe {
            (*leaf)[i0] = 0;
        }
    }
}

pub(super) fn map_user_code_page(aspace: u64, va: u64, pa: u64) {
    #[cfg(target_arch = "x86_64")]
    {
        const PRESENT: u64 = 1;
        const WRITE: u64 = 1 << 1;
        const USER: u64 = 1 << 2;
        map_page_x86(aspace, va, pa, PRESENT | WRITE | USER);
    }
    #[cfg(target_arch = "aarch64")]
    {
        map_user_page_aarch64(aspace, va, pa, false);
    }
    #[cfg(target_arch = "riscv64")]
    {
        map_user_page_riscv64(aspace, va, pa, true);
    }
}

pub(super) fn map_user_stack_page(aspace: u64, va: u64, pa: u64) {
    #[cfg(target_arch = "x86_64")]
    {
        const PRESENT: u64 = 1;
        const WRITE: u64 = 1 << 1;
        const USER: u64 = 1 << 2;
        const NX: u64 = 1 << 63;
        map_page_x86(aspace, va, pa, PRESENT | WRITE | USER | NX);
    }
    #[cfg(target_arch = "aarch64")]
    {
        map_user_page_aarch64(aspace, va, pa, true);
    }
    #[cfg(target_arch = "riscv64")]
    {
        map_user_page_riscv64(aspace, va, pa, false);
    }
}

#[cfg(target_arch = "aarch64")]
fn map_user_page_aarch64(l0_phys: u64, va: u64, pa: u64, stack: bool) {
    const PAGE_DESC: u64 = 0b11;
    const SH_INNER: u64 = 0b11 << 8;
    const AF: u64 = 1 << 10;
    const AP_RW: u64 = 0b01 << 6;
    const PXN: u64 = 1 << 53;
    const UXN: u64 = 1 << 54;
    const PA: u64 = 0x0000_FFFF_FFFF_F000;
    let page = aarch64_user_page_idx(va);
    let l3_idx = page % AARCH64_USER_L3_PAGES;
    let Some(l3) = aarch64_l3_table_mut(l0_phys, page) else {
        return;
    };
    unsafe {
        let l3_t = &mut *l3;
        let mut ent = PAGE_DESC | (pa & PA) | SH_INNER | AP_RW | AF | PXN;
        if stack {
            ent |= UXN;
        }
        l3_t[l3_idx] = ent;
    }
}

#[cfg(target_arch = "aarch64")]
pub(super) fn flush_user_tlb() {
    unsafe {
        core::arch::asm!("dsb ishst", options(nostack));
        if current_el() >= 2 {
            // VHE EL2&0 uses ALLE2; also drop EL1&0 in case TGE/E2H is off.
            core::arch::asm!("tlbi alle2is", options(nostack));
            core::arch::asm!("tlbi vmalle1is", options(nostack));
        } else {
            core::arch::asm!("tlbi vmalle1is", options(nostack));
        }
        core::arch::asm!("dsb ish; isb", options(nostack));
    }
    // Local flush only, matching the x86 path below: userspace is BSP-pinned
    // (`task::user_affinity()` returns Some(0) for every task and forks
    // inherit), so no AP ever loads a user aspace and no remote TLB can hold
    // its translations. The full IPI barrier after every map/unmap made each
    // shootdown a global event for all APs and — combined with the Dead-before-
    // reclaim die() window — was the -smp 4 interactive crawl. Keep the global
    // barrier available through `smp::tlb_shootdown` for the rare live-remote
    // reclaim case in `unload_user_aspace`.
}

#[cfg(target_arch = "x86_64")]
pub(super) fn flush_user_tlb() {
    unsafe {
        core::arch::asm!(
            "mov {cr3}, cr3",
            "mov cr3, {cr3}",
            cr3 = out(reg) _,
            options(nostack, preserves_flags),
        );
    }
    // x86 user tasks are affinity-pinned (no live migration). Each aspace is
    // only ever loaded on its home CPU, so remotes cannot cache its entries.
    // Broadcasting a TLB IPI barrier after every map/unmap (~ELF load, brk,
    // munmap, exit reclaim) roughly doubled MYOS_CI_MINI under -smp 4 TCG and
    // pushed GH runners past the 600s wall — do not paper that with a longer
    // QEMU timeout. Soft-ACK `tlb_shootdown` stays for unload fallback /
    // future float. aarch64/riscv still shoot down (float or different rules).
}

#[cfg(target_arch = "riscv64")]
pub(super) fn flush_user_tlb() {
    unsafe {
        core::arch::asm!("sfence.vma zero, zero", options(nostack));
    }
    crate::smp::tlb_shootdown();
}

/// Allocate Sv39 mid/leaf tables as needed so user maps can spill past one
/// 2 MiB leaf (code + 256 stack + 256 heap pages exceeds 512 PTEs).
#[cfg(target_arch = "riscv64")]
fn ensure_riscv_leaf(satp: u64, va: u64) -> *mut [u64; 512] {
    let root_phys = paging::satp_root_phys(satp);
    let i2 = ((va >> 30) & 0x1ff) as usize;
    let i1 = ((va >> 21) & 0x1ff) as usize;
    unsafe {
        let root = &mut *mm::table(root_phys);
        let mid_pte = root[i2];
        if mid_pte & paging::PTE_V == 0 {
            let mid = mm::alloc_frame_site(3);
            root[i2] = paging::pte_table(mid);
        } else {
            assert!(
                mid_pte & (paging::PTE_R | paging::PTE_W | paging::PTE_X) == 0,
                "riscv user map: huge page in the way at L2"
            );
        }
        let mid = &mut *mm::table(paging::pte_phys(root[i2]));
        let leaf_pte = mid[i1];
        if leaf_pte & paging::PTE_V == 0 {
            let leaf = mm::alloc_frame_site(3);
            mid[i1] = paging::pte_table(leaf);
        } else {
            assert!(
                leaf_pte & (paging::PTE_R | paging::PTE_W | paging::PTE_X) == 0,
                "riscv user map: huge page in the way at L1"
            );
        }
        mm::table(paging::pte_phys(mid[i1]))
    }
}

#[cfg(target_arch = "riscv64")]
fn map_user_page_riscv64(satp: u64, va: u64, pa: u64, exec: bool) {
    let mut flags = paging::PTE_V
        | paging::PTE_R
        | paging::PTE_W
        | paging::PTE_U
        | paging::PTE_A
        | paging::PTE_D;
    if exec {
        flags |= paging::PTE_X;
    }
    let i0 = ((va >> 12) & 0x1ff) as usize;
    let leaf = ensure_riscv_leaf(satp, va);
    unsafe {
        (*leaf)[i0] = paging::pte_leaf_4k(pa, flags);
    }
}

#[cfg(target_arch = "x86_64")]
pub(super) fn map_heap_page(pml4_phys: u64, va: u64, pa: u64) {
    const PRESENT: u64 = 1;
    const WRITE: u64 = 1 << 1;
    const USER: u64 = 1 << 2;
    const NX: u64 = 1 << 63;
    map_page_x86(pml4_phys, va, pa, PRESENT | WRITE | USER | NX);
}

#[cfg(target_arch = "aarch64")]
pub(super) fn map_heap_page(l0_phys: u64, va: u64, pa: u64) {
    map_user_page_aarch64(l0_phys, va, pa, false);
}

#[cfg(target_arch = "riscv64")]
pub(super) fn map_heap_page(satp: u64, va: u64, pa: u64) {
    let flags = paging::PTE_V
        | paging::PTE_R
        | paging::PTE_W
        | paging::PTE_U
        | paging::PTE_A
        | paging::PTE_D;
    let i0 = ((va >> 12) & 0x1ff) as usize;
    let leaf = ensure_riscv_leaf(satp, va);
    unsafe {
        (*leaf)[i0] = paging::pte_leaf_4k(pa, flags);
    }
}

#[cfg(target_arch = "x86_64")]
fn map_page_x86(pml4_phys: u64, va: u64, pa: u64, flags: u64) {
    const PRESENT: u64 = 1;
    const WRITE: u64 = 1 << 1;
    const USER: u64 = 1 << 2;
    const HUGE: u64 = 1 << 7;

    let i4 = ((va >> 39) & 0x1ff) as usize;
    let i3 = ((va >> 30) & 0x1ff) as usize;
    let i2 = ((va >> 21) & 0x1ff) as usize;
    let i1 = ((va >> 12) & 0x1ff) as usize;

    unsafe {
        let pml4 = &mut *mm::table(pml4_phys);
        let pdpt = ensure_user(&mut pml4[i4], PRESENT | WRITE | USER, HUGE);
        let pd = ensure_user(&mut (*pdpt)[i3], PRESENT | WRITE | USER, HUGE);
        let pt = ensure_user(&mut (*pd)[i2], PRESENT | WRITE | USER, HUGE);
        (*pt)[i1] = (pa & !0xfff) | flags;
    }
}

#[cfg(target_arch = "x86_64")]
fn ensure_user(entry: &mut u64, table_flags: u64, huge: u64) -> *mut [u64; 512] {
    if *entry & 1 != 0 {
        assert!(*entry & huge == 0, "user map: huge page in the way");
        return mm::table(*entry);
    }
    let phys = mm::alloc_frame_site(3);
    *entry = phys | table_flags;
    mm::table(phys)
}

#[cfg(target_arch = "aarch64")]
fn create_aspace_aarch64(code: &[u64], stack: &[u64], _base: u64, stack_off: u64) -> u64 {
    const TABLE: u64 = 0b11;
    const PAGE_DESC: u64 = 0b11;
    const SH_INNER: u64 = 0b11 << 8;
    const AF: u64 = 1 << 10;
    const AP_RW: u64 = 0b01 << 6; // EL1 RW, EL0 RW
    const PXN: u64 = 1 << 53;
    const UXN: u64 = 1 << 54;
    const PA: u64 = 0x0000_FFFF_FFFF_F000;

    let k_l0 = task::kernel_aspace() & PA;
    let k_l0_t = unsafe { &*mm::table(k_l0) };
    let k_l1_phys = k_l0_t[0] & PA;
    let k_l1 = unsafe { &*mm::table(k_l1_phys) };

    let l0 = mm::alloc_frame_site(3);
    let l1 = mm::alloc_frame_site(3);
    let l2 = mm::alloc_frame_site(3);
    let l3 = mm::alloc_frame_site(3);

    unsafe {
        let l0_t = &mut *mm::table(l0);
        let l1_t = &mut *mm::table(l1);
        let l2_t = &mut *mm::table(l2);
        // Copy kernel TTBR0 extras (high PCI) and L1 device blocks. L1[1] is user.
        for i in 0..512 {
            if i != 0 && k_l0_t[i] != 0 {
                l0_t[i] = k_l0_t[i];
            }
        }
        l0_t[0] = l1 | TABLE;
        for i in 0..512 {
            if i != 1 && k_l1[i] != 0 {
                l1_t[i] = k_l1[i];
            }
        }
        l1_t[1] = l2 | TABLE;
        l2_t[0] = l3 | TABLE;
        // AP_RW: EL1 sys_read copies into PT_LOAD. PXN: EL1 cannot execute it.
        for (i, &phys) in code.iter().enumerate() {
            let Some(l3p) = aarch64_l3_table_mut(l0, i) else {
                break;
            };
            let l3_t = &mut *l3p;
            let slot = i % AARCH64_USER_L3_PAGES;
            l3_t[slot] = PAGE_DESC | (phys & PA) | SH_INNER | AF | AP_RW | PXN;
        }
        let stack_i = (stack_off as usize) / PAGE;
        for (i, &phys) in stack.iter().enumerate() {
            let page = stack_i + i;
            let Some(l3p) = aarch64_l3_table_mut(l0, page) else {
                break;
            };
            let l3_t = &mut *l3p;
            let slot = page % AARCH64_USER_L3_PAGES;
            l3_t[slot] = PAGE_DESC | (phys & PA) | SH_INNER | AF | AP_RW | PXN | UXN;
        }
    }
    l0
}

#[cfg(target_arch = "riscv64")]
fn create_aspace_riscv64(code: &[u64], stack: &[u64], base: u64, stack_off: u64) -> u64 {
    let k_root_phys = paging::satp_root_phys(task::kernel_aspace());
    let root = mm::alloc_frame_site(5);

    unsafe {
        let k_root = &*mm::table(k_root_phys);
        let root_t = &mut *mm::table(root);
        root_t.copy_from_slice(k_root);
        // User lives in Sv39 root[1] (0x4000_0000). Clear any stale kernel
        // entry so ensure_riscv_leaf owns mid/leaf allocation for this aspace.
        root_t[1] = 0;
    }
    let satp = make_satp(root);
    for (i, &phys) in code.iter().enumerate() {
        map_user_page_riscv64(satp, base + (i * PAGE) as u64, phys, true);
    }
    let stack_va = base + stack_off;
    for (i, &phys) in stack.iter().enumerate() {
        map_user_page_riscv64(satp, stack_va + (i * PAGE) as u64, phys, false);
    }
    satp
}

pub(super) fn sync_icache(start: usize, size: usize) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!(
            "mov {cr3}, cr3",
            "mov cr3, {cr3}",
            cr3 = out(reg) _,
            options(nostack, preserves_flags),
        );
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        if size == 0 {
            return;
        }
        let mut addr = start & !63;
        let end = start + size;
        while addr < end {
            core::arch::asm!("dc cvau, {x}", x = in(reg) addr, options(nostack));
            addr += 64;
        }
        core::arch::asm!("dsb ish", options(nostack));
        addr = start & !63;
        while addr < end {
            core::arch::asm!("ic ivau, {x}", x = in(reg) addr, options(nostack));
            addr += 64;
        }
        core::arch::asm!("dsb ish; isb", options(nostack));
        core::arch::asm!("ic ialluis; dsb ish; isb", options(nostack));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        if size != 0 {
            core::arch::asm!("fence.i", options(nostack));
        }
    }
    let _ = (start, size);
}
