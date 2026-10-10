//! User address spaces on Sv39: a private root table cloned from the
//! kernel's, with private mid/leaf tables under root[1].

use crate::mm;
use crate::task;
use crate::user::PAGE;

use super::paging;
use super::user::DEFAULT_USER_BASE;

/// Physical address of `va` in `aspace`, if mapped.
pub fn virt_to_phys(aspace: u64, va: u64) -> Option<u64> {
    virt_to_phys_riscv64(aspace, va)
}

fn satp_ppn(satp: u64) -> u64 {
    paging::satp_root_phys(satp)
}

fn make_satp(root_phys: u64) -> u64 {
    paging::make_satp(root_phys)
}

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

/// The VA where user images are loaded.
pub fn pick_user_base() -> u64 {
    DEFAULT_USER_BASE
}

/// The address space currently loaded (`satp`).
pub fn read_aspace() -> u64 {
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
    unsafe {
        core::arch::asm!(
            "csrw satp, {a}",
            "sfence.vma",
            a = in(reg) aspace,
            options(nostack),
        );
    }
}

/// Free private user page tables for an abandoned aspace (data pages must
/// already be unmapped and freed).
pub fn free_user_page_tables(aspace: u64) {
    free_user_page_tables_riscv(aspace);
}

/// Tear down Sv39 tables owned by a user aspace.
///
/// `create_aspace_riscv64` clones the kernel root then clears `root[1]` and
/// allocates private mid/leaf tables under VPN[2]=1 (VA `0x4000_0000`). Only
/// that private tree is freed (leaf → mid → root). Other root slots are
/// value-copies of kernel PTEs and must not be freed.
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

/// A new address space with `code` mapped RWX at `base` and `stack` RW at
/// `base + stack_off`.
pub fn create_aspace(code: &[u64], stack: &[u64], base: u64, stack_off: u64) -> u64 {
    create_aspace_riscv64(code, stack, base, stack_off)
}

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

/// Map one user page with the given permissions (always readable).
pub fn map_user_page_prot(aspace: u64, va: u64, pa: u64, w: bool, x: bool) {
    let mut flags = paging::PTE_V | paging::PTE_U | paging::PTE_A | paging::PTE_D | paging::PTE_R;
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

pub fn unmap_user_page(aspace: u64, va: u64) {
    let i0 = ((va >> 12) & 0x1ff) as usize;
    let leaf = ensure_riscv_leaf(aspace, va);
    unsafe {
        (*leaf)[i0] = 0;
    }
}

/// A bit of a leaf entry reserved for software (RSW): the page's frame is
/// shared with another address space by a fork, copied before a store
/// (`user::cow`).
pub const LEAF_COW: u64 = 1 << 8;
/// The other RSW bit: the page was writable before the share (its copy is
/// again).
pub const LEAF_COW_WRITE: u64 = 1 << 9;

/// The leaf entry of `va` in `aspace`, if `va` is mapped by a 4 KiB page:
/// a pointer into the page table, for the `leaf_*` helpers to read and
/// change it in place. Allocates nothing.
pub fn leaf_mut(aspace: u64, va: u64) -> Option<*mut u64> {
    let root_phys = satp_ppn(aspace);
    let i2 = ((va >> 30) & 0x1ff) as usize;
    let i1 = ((va >> 21) & 0x1ff) as usize;
    let i0 = ((va >> 12) & 0x1ff) as usize;
    unsafe {
        let root = &*mm::table(root_phys);
        let mid_pte = root[i2];
        if mid_pte & paging::PTE_V == 0 || !paging::pte_is_table(mid_pte) {
            return None;
        }
        let mid = &*mm::table(paging::pte_phys(mid_pte));
        let leaf_pte = mid[i1];
        if leaf_pte & paging::PTE_V == 0 || !paging::pte_is_table(leaf_pte) {
            return None;
        }
        let leaf = &mut *mm::table(paging::pte_phys(leaf_pte));
        let pte = leaf[i0];
        if pte & paging::PTE_V == 0 || pte & (paging::PTE_R | paging::PTE_W | paging::PTE_X) == 0 {
            return None;
        }
        Some(&mut leaf[i0] as *mut u64)
    }
}

/// Set the leaf entry of `va` to `pte` as it is (the tables made as needed).
pub fn set_leaf(aspace: u64, va: u64, pte: u64) {
    let i0 = ((va >> 12) & 0x1ff) as usize;
    let leaf = ensure_riscv_leaf(aspace, va);
    unsafe {
        (*leaf)[i0] = pte;
    }
}

pub fn leaf_phys(pte: u64) -> u64 {
    paging::pte_phys(pte)
}

pub fn leaf_with_phys(pte: u64, phys: u64) -> u64 {
    paging::pte_leaf_4k(phys, pte & 0x3ff)
}

pub fn leaf_writable(pte: u64) -> bool {
    pte & paging::PTE_W != 0
}

pub fn leaf_with_write(pte: u64, w: bool) -> u64 {
    if w { pte | paging::PTE_W } else { pte & !paging::PTE_W }
}

pub fn leaf_executable(pte: u64) -> bool {
    pte & paging::PTE_X != 0
}

/// Map a code page: RW so the loader can fill it, and executable.
pub fn map_user_code_page(aspace: u64, va: u64, pa: u64) {
    map_user_page_riscv64(aspace, va, pa, true);
}

pub fn map_user_stack_page(aspace: u64, va: u64, pa: u64) {
    map_user_page_riscv64(aspace, va, pa, false);
}

pub fn flush_user_tlb() {
    unsafe {
        core::arch::asm!("sfence.vma zero, zero", options(nostack));
    }
    crate::smp::tlb_shootdown();
}

/// Allocate Sv39 mid/leaf tables as needed so user maps can spill past one
/// 2 MiB leaf (code + 256 stack + 256 heap pages exceeds 512 PTEs).
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

pub fn map_heap_page(satp: u64, va: u64, pa: u64) {
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
