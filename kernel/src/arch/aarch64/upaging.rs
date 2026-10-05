//! User address spaces on aarch64 4 KiB granule paging (TTBR0): a private
//! L0/L1/L2 tree with up to [`USER_L2_TABLES`] L3 tables for the process span.

use crate::mm;
use crate::task;
use crate::user::PAGE;

use super::user::DEFAULT_USER_BASE;

const PAGE_DESC: u64 = 0b11;
const SH_INNER: u64 = 0b11 << 8;
const AF: u64 = 1 << 10;
const AP_RW: u64 = 0b01 << 6;
const AP_RO: u64 = 0b11 << 6;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;
const PA: u64 = 0x0000_FFFF_FFFF_F000;

/// Pages per L3 table (2 MiB).
pub const USER_L3_PAGES: usize = 512;
/// L3 tables (2 MiB each, allocated as pages are mapped) a user aspace may
/// use under its L2 table: the whole L1[1] gigabyte.
pub const USER_L2_TABLES: usize = 512;

/// Physical address of `va` in `aspace`, if mapped.
pub fn virt_to_phys(aspace: u64, va: u64) -> Option<u64> {
    virt_to_phys_aarch64(aspace, va)
}

fn aarch64_user_page_idx(va: u64) -> usize {
    let base = crate::user::user_base();
    va.saturating_sub(base) as usize / PAGE
}

/// Return the L3 table for `page` (allocating L2/L3 spill slots as needed).
fn aarch64_l3_table_mut(l0_phys: u64, page: usize) -> Option<*mut [u64; 512]> {
    const TABLE: u64 = 0b11;
    let l2_idx = page / USER_L3_PAGES;
    if l2_idx >= USER_L2_TABLES {
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

fn virt_to_phys_aarch64(l0_phys: u64, va: u64) -> Option<u64> {
    let page = aarch64_user_page_idx(va);
    let l3_idx = page % USER_L3_PAGES;
    let l2_idx = page / USER_L3_PAGES;
    if l2_idx >= USER_L2_TABLES {
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

/// The VA where user images are loaded.
pub fn pick_user_base() -> u64 {
    DEFAULT_USER_BASE
}

/// The address space currently loaded (TTBR0).
pub fn read_aspace() -> u64 {
    unsafe {
        let t: u64;
        core::arch::asm!(
            "mrs {t}, ttbr0_el1",
            t = out(reg) t,
            options(nomem, nostack, preserves_flags)
        );
        t
    }
}

pub fn switch_aspace(aspace: u64) {
    unsafe {
        core::arch::asm!(
            "msr ttbr0_el1, {a}",
            "dsb sy",
            a = in(reg) aspace,
            options(nostack),
        );
        if super::cpu::current_el() >= 2 {
            core::arch::asm!("tlbi alle2is", options(nostack));
        } else {
            core::arch::asm!("tlbi vmalle1", options(nostack));
        }
        core::arch::asm!("dsb sy; isb", options(nostack));
    }
}

/// Free private user page tables for an abandoned aspace: not reclaimed on
/// aarch64 yet (the L0/L1/L2/L3 frames of a dead process are leaked).
pub fn free_user_page_tables(aspace: u64) {
    let _ = aspace;
}

/// A new address space with `code` mapped RWX at `base` and `stack` RW at
/// `base + stack_off`.
pub fn create_aspace(code: &[u64], stack: &[u64], base: u64, stack_off: u64) -> u64 {
    create_aspace_aarch64(code, stack, base, stack_off)
}

fn create_aspace_aarch64(code: &[u64], stack: &[u64], _base: u64, stack_off: u64) -> u64 {
    const TABLE: u64 = 0b11;
    const AP_RW: u64 = 0b01 << 6; // EL1 RW, EL0 RW

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
            let slot = i % USER_L3_PAGES;
            l3_t[slot] = PAGE_DESC | (phys & PA) | SH_INNER | AF | AP_RW | PXN;
        }
        let stack_i = (stack_off as usize) / PAGE;
        for (i, &phys) in stack.iter().enumerate() {
            let page = stack_i + i;
            let Some(l3p) = aarch64_l3_table_mut(l0, page) else {
                break;
            };
            let l3_t = &mut *l3p;
            let slot = page % USER_L3_PAGES;
            l3_t[slot] = PAGE_DESC | (phys & PA) | SH_INNER | AF | AP_RW | PXN | UXN;
        }
    }
    l0
}

/// Map one user page with the given permissions (always readable).
pub fn map_user_page_prot(aspace: u64, va: u64, pa: u64, w: bool, x: bool) {
    let page = aarch64_user_page_idx(va);
    let l3_idx = page % USER_L3_PAGES;
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

pub fn unmap_user_page(aspace: u64, va: u64) {
    let page = aarch64_user_page_idx(va);
    let l3_idx = page % USER_L3_PAGES;
    let Some(l3) = aarch64_l3_table_mut(aspace, page) else {
        return;
    };
    unsafe {
        (*l3)[l3_idx] = 0;
    }
}

/// Map a code page: RW so the loader can fill it, and executable.
pub fn map_user_code_page(aspace: u64, va: u64, pa: u64) {
    map_user_page_aarch64(aspace, va, pa, false);
}

pub fn map_user_stack_page(aspace: u64, va: u64, pa: u64) {
    map_user_page_aarch64(aspace, va, pa, true);
}

fn map_user_page_aarch64(l0_phys: u64, va: u64, pa: u64, stack: bool) {
    let page = aarch64_user_page_idx(va);
    let l3_idx = page % USER_L3_PAGES;
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

pub fn flush_user_tlb() {
    unsafe {
        core::arch::asm!("dsb ishst", options(nostack));
        if super::cpu::current_el() >= 2 {
            // VHE EL2&0 uses ALLE2; also drop EL1&0 in case TGE/E2H is off.
            core::arch::asm!("tlbi alle2is", options(nostack));
            core::arch::asm!("tlbi vmalle1is", options(nostack));
        } else {
            core::arch::asm!("tlbi vmalle1is", options(nostack));
        }
        core::arch::asm!("dsb ish; isb", options(nostack));
    }
    // The inner-shareable `tlbi` reaches every CPU, one with this aspace
    // loaded (a thread of the process runs there) too, without an IPI.
}

pub fn map_heap_page(l0_phys: u64, va: u64, pa: u64) {
    map_user_page_aarch64(l0_phys, va, pa, false);
}
