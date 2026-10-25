//! User address spaces on aarch64 4 KiB granule paging (TTBR0): a private
//! L0 and L1, and under the L1's user slots ([`USER_L1_TABLES`] of them,
//! from L1[0]) private L2 and L3 tables made as pages are mapped.

use crate::mm;
use crate::task;
use crate::user::PAGE;


const PAGE_DESC: u64 = 0b11;
const SH_INNER: u64 = 0b11 << 8;
const SH_OUTER: u64 = 0b10 << 8;
/// MAIR Attr2: Device-nGnRE (`paging::ATTR_DEVICE`).
const ATTR_DEVICE: u64 = 2 << 2;
/// MAIR Attr3: Normal non-cacheable (`paging::ATTR_NOCACHE`).
const ATTR_NOCACHE: u64 = 3 << 2;
const AF: u64 = 1 << 10;
const AP_RW: u64 = 0b01 << 6;
const AP_RO: u64 = 0b11 << 6;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;
const PA: u64 = 0x0000_FFFF_FFFF_F000;

/// Pages per L3 table (2 MiB).
pub const USER_L3_PAGES: usize = 512;
/// L3 tables under an L2 table (1 GiB).
pub const USER_L2_TABLES: usize = 512;
/// L1 slots of the user window, from L1[0]: 66 GiB, room for an image
/// at `DEFAULT_USER_BASE` (1 GiB) or at its link address below, its stack
/// and heap and the 64 GiB mmap window after them. The kernel's devices are
/// in the higher half (`paging`), not here.
pub const USER_L1_TABLES: usize = 66;
/// The first L1 slot of the user window.
const USER_L1_FIRST: usize = 0;

/// Physical address of `va` in `aspace`, if mapped.
pub fn virt_to_phys(aspace: u64, va: u64) -> Option<u64> {
    virt_to_phys_aarch64(aspace, va)
}

fn aarch64_user_page_idx(va: u64) -> usize {
    va as usize / PAGE
}

/// The L1, L2 and L3 indices of user page `page`; none past the span.
fn table_indices(page: usize) -> Option<(usize, usize, usize)> {
    let l1_idx = USER_L1_FIRST + page / (USER_L2_TABLES * USER_L3_PAGES);
    if l1_idx >= USER_L1_FIRST + USER_L1_TABLES {
        return None;
    }
    Some((l1_idx, page / USER_L3_PAGES % USER_L2_TABLES, page % USER_L3_PAGES))
}

/// The L1 table of `l0_phys` (every user aspace has its own).
unsafe fn l1_table(l0_phys: u64) -> Option<*mut [u64; 512]> {
    let l0 = unsafe { &*mm::table(l0_phys) };
    let l1_phys = l0[0] & PA;
    if l1_phys == 0 {
        return None;
    }
    Some(mm::table(l1_phys))
}

/// Return the L3 table for `page` (allocating the L2 and L3 tables as needed).
fn aarch64_l3_table_mut(l0_phys: u64, page: usize) -> Option<*mut [u64; 512]> {
    const TABLE: u64 = 0b11;
    let (l1_idx, l2_idx, _) = table_indices(page)?;
    unsafe {
        let l1 = &mut *l1_table(l0_phys)?;
        if l1[l1_idx] & 0b11 != TABLE {
            let l2 = mm::alloc_frame_site(3);
            l1[l1_idx] = l2 | TABLE;
        }
        let l2 = &mut *mm::table(l1[l1_idx] & PA);
        if l2[l2_idx] & 0b11 != TABLE {
            let l3 = mm::alloc_frame_site(3);
            l2[l2_idx] = l3 | TABLE;
        }
        Some(mm::table(l2[l2_idx] & PA))
    }
}

/// The L3 table for `page` if it exists (allocates nothing).
fn aarch64_l3_table(l0_phys: u64, page: usize) -> Option<*mut [u64; 512]> {
    const TABLE: u64 = 0b11;
    let (l1_idx, l2_idx, _) = table_indices(page)?;
    unsafe {
        let l1 = &*l1_table(l0_phys)?;
        if l1[l1_idx] & 0b11 != TABLE {
            return None;
        }
        let l2 = &*mm::table(l1[l1_idx] & PA);
        if l2[l2_idx] & 0b11 != TABLE {
            return None;
        }
        Some(mm::table(l2[l2_idx] & PA))
    }
}

/// The end of the span around `va` that has no L3 table (an L1 or L2
/// entry absent, or `va` past the span): every page in it is unmapped.
/// `None` when `va`'s L3 table exists (the page itself may be unmapped).
/// Walking a range page by page skips its empty tables with this.
pub fn unmapped_span_end(aspace: u64, va: u64) -> Option<u64> {
    const TABLE: u64 = 0b11;
    let Some((l1_idx, l2_idx, _)) = table_indices(aarch64_user_page_idx(va)) else {
        return Some((va | ((1 << 30) - 1)) + 1);
    };
    unsafe {
        let Some(l1) = l1_table(aspace) else {
            return Some((va | ((1 << 30) - 1)) + 1);
        };
        if (*l1)[l1_idx] & 0b11 != TABLE {
            return Some((va | ((1 << 30) - 1)) + 1);
        }
        let l2 = &*mm::table((*l1)[l1_idx] & PA);
        if l2[l2_idx] & 0b11 != TABLE {
            return Some((va | ((1 << 21) - 1)) + 1);
        }
    }
    None
}

fn virt_to_phys_aarch64(l0_phys: u64, va: u64) -> Option<u64> {
    let page = aarch64_user_page_idx(va);
    let l3 = aarch64_l3_table(l0_phys, page)?;
    let pte = unsafe { (*l3)[page % USER_L3_PAGES] };
    if pte & 0b11 != PAGE_DESC {
        return None;
    }
    Some(pte & PA)
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

/// Free private user page tables for an abandoned aspace (data pages must
/// already be unmapped and freed, and no CPU may have it loaded).
///
/// `create_aspace_aarch64` gives each aspace its own L0 and the L1 under
/// L0[0], and `aarch64_l3_table_mut` adds L2 tables under the L1's user
/// slots and L3 tables under those. That tree is freed (L3 → L2 → L1 → L0).
pub fn free_user_page_tables(aspace: u64) {
    const TABLE: u64 = 0b11;
    let l0_phys = aspace & PA;
    if l0_phys == 0 || l0_phys == task::kernel_aspace() & PA {
        return;
    }
    unsafe {
        let l0 = &mut *mm::table(l0_phys);
        if l0[0] & 0b11 == TABLE {
            let l1_phys = l0[0] & PA;
            let l1 = &mut *mm::table(l1_phys);
            for l1_slot in l1[USER_L1_FIRST..USER_L1_FIRST + USER_L1_TABLES].iter_mut() {
                if *l1_slot & 0b11 != TABLE {
                    continue;
                }
                let l2_phys = *l1_slot & PA;
                let l2 = &mut *mm::table(l2_phys);
                for slot in l2.iter_mut() {
                    if *slot & 0b11 == TABLE {
                        mm::free_frame(*slot & PA);
                        *slot = 0;
                    }
                }
                mm::free_frame(l2_phys);
                *l1_slot = 0;
            }
            mm::free_frame(l1_phys);
            l0[0] = 0;
        }
        mm::free_frame(l0_phys);
    }
}

/// A new address space with `code` mapped RWX at `base` and `stack` RW at
/// `base + stack_off`.
pub fn create_aspace(code: &[u64], stack: &[u64], base: u64, stack_off: u64) -> u64 {
    create_aspace_aarch64(code, stack, base, stack_off)
}

fn create_aspace_aarch64(code: &[u64], stack: &[u64], base: u64, stack_off: u64) -> u64 {
    const TABLE: u64 = 0b11;
    const AP_RW: u64 = 0b01 << 6; // EL1 RW, EL0 RW

    let l0 = mm::alloc_frame_site(3);
    let l1 = mm::alloc_frame_site(3);

    unsafe {
        let l0_t = &mut *mm::table(l0);
        // Nothing of the kernel's is in TTBR0 (its devices are in the
        // higher half): the L1's user slots fill as the process needs them
        // (`aarch64_l3_table_mut`).
        l0_t[0] = l1 | TABLE;
        // AP_RW: EL1 sys_read copies into PT_LOAD. PXN: EL1 cannot execute it.
        let base_i = aarch64_user_page_idx(base);
        for (i, &phys) in code.iter().enumerate() {
            let page = base_i + i;
            let Some(l3p) = aarch64_l3_table_mut(l0, page) else {
                break;
            };
            let l3_t = &mut *l3p;
            let slot = page % USER_L3_PAGES;
            l3_t[slot] = PAGE_DESC | (phys & PA) | SH_INNER | AF | AP_RW | PXN;
        }
        let stack_i = base_i + (stack_off as usize) / PAGE;
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

/// A device's registers at `pa` (a PCI BAR): MAIR Attr2, Device-nGnRE
/// (the kernel's own device mappings'), never executable.
pub fn map_user_device_page(aspace: u64, va: u64, pa: u64, w: bool) {
    let page = aarch64_user_page_idx(va);
    let l3_idx = page % USER_L3_PAGES;
    let Some(l3) = aarch64_l3_table_mut(aspace, page) else {
        return;
    };
    let mut ent = PAGE_DESC | (pa & PA) | ATTR_DEVICE | SH_OUTER | AF | PXN | UXN;
    ent |= if w { AP_RW } else { AP_RO };
    unsafe { (*l3)[l3_idx] = ent };
}

/// Memory at `pa` a device reads and writes without seeing the caches (a
/// DMA buffer of such a device): MAIR Attr3, Normal non-cacheable, never
/// executable.
pub fn map_user_nocache_page(aspace: u64, va: u64, pa: u64, w: bool) {
    let page = aarch64_user_page_idx(va);
    let l3_idx = page % USER_L3_PAGES;
    let Some(l3) = aarch64_l3_table_mut(aspace, page) else {
        return;
    };
    let mut ent = PAGE_DESC | (pa & PA) | ATTR_NOCACHE | SH_OUTER | AF | PXN | UXN;
    ent |= if w { AP_RW } else { AP_RO };
    unsafe { (*l3)[l3_idx] = ent };
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

/// A bit of a page descriptor reserved for software (bits 58:55): the
/// page's frame is shared with another address space by a fork, copied
/// before a store (`user::cow`).
pub const LEAF_COW: u64 = 1 << 55;
/// Another: the page was writable before the share (its copy is again).
pub const LEAF_COW_WRITE: u64 = 1 << 56;

/// AP[2]: 1 for a read-only page (`AP_RO`), 0 for a writable one.
const AP_READ_ONLY: u64 = 1 << 7;

/// The leaf entry of `va` in `aspace`, if `va` is mapped: a pointer into
/// the page table, for the `leaf_*` helpers to read and change it in
/// place. Allocates nothing.
pub fn leaf_mut(aspace: u64, va: u64) -> Option<*mut u64> {
    let page = aarch64_user_page_idx(va);
    let l3 = unsafe { &mut *aarch64_l3_table(aspace, page)? };
    let l3_idx = page % USER_L3_PAGES;
    if l3[l3_idx] & 0b11 != PAGE_DESC {
        return None;
    }
    Some(&mut l3[l3_idx] as *mut u64)
}

/// Set the leaf entry of `va` to `pte` as it is (the tables made as needed).
pub fn set_leaf(aspace: u64, va: u64, pte: u64) {
    let page = aarch64_user_page_idx(va);
    let Some(l3) = aarch64_l3_table_mut(aspace, page) else {
        return;
    };
    unsafe {
        (*l3)[page % USER_L3_PAGES] = pte;
    }
}

pub fn leaf_phys(pte: u64) -> u64 {
    pte & PA
}

pub fn leaf_with_phys(pte: u64, phys: u64) -> u64 {
    (pte & !PA) | (phys & PA)
}

pub fn leaf_writable(pte: u64) -> bool {
    pte & AP_READ_ONLY == 0
}

pub fn leaf_with_write(pte: u64, w: bool) -> u64 {
    if w { pte & !AP_READ_ONLY } else { pte | AP_READ_ONLY }
}

pub fn leaf_executable(pte: u64) -> bool {
    pte & UXN == 0
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
