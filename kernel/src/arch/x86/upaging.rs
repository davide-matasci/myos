//! User address spaces on 4-level x86_64 paging: a per-process PML4 cloned
//! from the kernel's, with a private PDPT/PD/PT tree under the user slot.


use crate::mm;
use crate::task;
use crate::user::PAGE;

use super::user::DEFAULT_USER_BASE;

const PRESENT: u64 = 1;
const WRITE: u64 = 1 << 1;
const USER: u64 = 1 << 2;
const HUGE: u64 = 1 << 7;
const NX: u64 = 1 << 63;

/// Physical address of `va` in `aspace`, if mapped.
pub fn virt_to_phys(aspace: u64, va: u64) -> Option<u64> {
    virt_to_phys_x86(aspace, va)
}

fn virt_to_phys_x86(pml4_phys: u64, va: u64) -> Option<u64> {
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

/// The VA where user images are loaded.
///
/// Prefer DEFAULT (PML4[1]). If Limine already occupied that slot (common on
/// UEFI), pick another free low-half slot. `create_aspace` clears *this*
/// index after the kernel PML4 clone, and `free_user_page_tables` tears
/// down the same index via the chosen base — never hardcode slot 1 for both
/// map and reclaim while pick walks away from it.
pub fn pick_user_base() -> u64 {
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

/// The address space currently loaded (CR3).
pub fn read_aspace() -> u64 {
    unsafe {
        let c: u64;
        core::arch::asm!(
            "mov {c}, cr3",
            c = out(reg) c,
            options(nomem, nostack, preserves_flags)
        );
        c
    }
}

pub fn switch_aspace(aspace: u64) {
    unsafe {
        core::arch::asm!(
            "mov cr3, {a}",
            a = in(reg) aspace,
            options(nostack, preserves_flags)
        );
    }
}

/// Free private user page tables for an abandoned aspace (data pages must
/// already be unmapped and freed).
pub fn free_user_page_tables(aspace: u64) {
    free_user_page_tables_x86(aspace);
}

/// Tear down 4-level tables owned by a user aspace (x86_64).
///
/// `create_aspace_x86` clones the kernel PML4 then allocates a private PDPT/PD/PT
/// tree under PML4[1] (VA `0x80_0000_0000`). Free any remaining present leaf
/// pages under that index (catches orphans outside the code/stack/heap/mmap
/// windows), then free the private tables and the cloned PML4. Other PML4
/// slots are value-copies of kernel entries and must not be freed.
fn free_user_page_tables_x86(pml4_phys: u64) {
    const PHYS_MASK: u64 = 0x000f_ffff_ffff_f000;
    // Tear down the slot that holds USER_BASE (pinned to PML4[1] / DEFAULT).
    let user_pml4_idx = ((crate::user::user_base() >> 39) & 0x1ff) as usize;
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

/// A new address space with `code` mapped RWX at `base` and `stack` RW at
/// `base + stack_off`.
pub fn create_aspace(code: &[u64], stack: &[u64], base: u64, stack_off: u64) -> u64 {
    create_aspace_x86(code, stack, base, stack_off)
}

fn create_aspace_x86(code: &[u64], stack: &[u64], base: u64, stack_off: u64) -> u64 {

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
        let user_idx = ((crate::user::user_base() >> 39) & 0x1ff) as usize;
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

/// Map one user page with the given permissions (always readable).
pub fn map_user_page_prot(aspace: u64, va: u64, pa: u64, w: bool, x: bool) {
    let mut flags = PRESENT | USER;
    if w {
        flags |= WRITE;
    }
    if !x {
        flags |= NX;
    }
    map_page_x86(aspace, va, pa, flags);
}

pub fn unmap_user_page(aspace: u64, va: u64) {
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

/// Map a code page: RW so the loader can fill it, and executable.
pub fn map_user_code_page(aspace: u64, va: u64, pa: u64) {
    map_page_x86(aspace, va, pa, PRESENT | WRITE | USER);
}

pub fn map_user_stack_page(aspace: u64, va: u64, pa: u64) {
    map_page_x86(aspace, va, pa, PRESENT | WRITE | USER | NX);
}

pub fn flush_user_tlb() {
    unsafe {
        core::arch::asm!(
            "mov {cr3}, cr3",
            "mov cr3, {cr3}",
            cr3 = out(reg) _,
            options(nostack, preserves_flags),
        );
    }
    // Other CPUs only when one has this aspace loaded (a thread of the
    // process runs there): a CR3 switch drops the rest. Broadcasting a TLB
    // IPI barrier after every map/unmap (~ELF load, brk, munmap, exit
    // reclaim) roughly doubled MYOS_CI_MINI under -smp 4 TCG and pushed GH
    // runners past the 600s wall — do not paper that with a longer QEMU
    // timeout. aarch64's `tlbi ...is` broadcasts; riscv shoots down.
    if crate::task::aspace_loaded_elsewhere(crate::task::current_aspace()) {
        crate::smp::tlb_shootdown();
    }
}

pub fn map_heap_page(pml4_phys: u64, va: u64, pa: u64) {
    map_page_x86(pml4_phys, va, pa, PRESENT | WRITE | USER | NX);
}

fn map_page_x86(pml4_phys: u64, va: u64, pa: u64, flags: u64) {

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

fn ensure_user(entry: &mut u64, table_flags: u64, huge: u64) -> *mut [u64; 512] {
    if *entry & 1 != 0 {
        assert!(*entry & huge == 0, "user map: huge page in the way");
        return mm::table(*entry);
    }
    let phys = mm::alloc_frame_site(3);
    *entry = phys | table_flags;
    mm::table(phys)
}
