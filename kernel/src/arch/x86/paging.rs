//! Smallest MMIO mapper on Limine's page tables. HHDM covers RAM only;
//! PCI BARs need uncacheable 4 KiB PTEs (same trick as the local APIC).

use crate::limine_boot;
use crate::mm;

fn cr3_phys() -> u64 {
    let c: u64;
    unsafe {
        core::arch::asm!(
            "mov {c}, cr3",
            c = out(reg) c,
            options(nomem, nostack, preserves_flags)
        );
    }
    c & !0xfff
}

fn table_at(phys: u64) -> *mut [u64; 512] {
    (limine_boot::hhdm_offset() + (phys & !0xfff)) as *mut [u64; 512]
}

fn split_huge(entry: &mut u64, child_huge: bool, child_size: u64) {
    let base = *entry & 0x000f_ffff_ffff_f000;
    let flags = (*entry & !0x000f_ffff_ffff_f000) & !(1 << 7);
    let phys = mm::alloc_frame();
    let t = unsafe { &mut *table_at(phys) };
    for i in 0..512 {
        let p = base + i as u64 * child_size;
        let mut e = p | flags;
        if child_huge {
            e |= 1 << 7;
        }
        t[i] = e;
    }
    *entry = phys | 0b11;
}

fn ensure_table(entry: &mut u64) -> *mut [u64; 512] {
    if *entry & 1 != 0 {
        assert!(*entry & (1 << 7) == 0, "mmio map: huge page at table level");
        return table_at(*entry);
    }
    let phys = mm::alloc_frame();
    *entry = phys | 0b11;
    table_at(phys)
}

fn ensure_leaf_parent(entry: &mut u64, child_huge: bool, child_size: u64) -> *mut [u64; 512] {
    if *entry & 1 != 0 {
        if *entry & (1 << 7) != 0 {
            split_huge(entry, child_huge, child_size);
        }
        return table_at(*entry);
    }
    let phys = mm::alloc_frame();
    *entry = phys | 0b11;
    table_at(phys)
}

fn map_one(pa: u64, va: u64) {
    let i4 = ((va >> 39) & 0x1ff) as usize;
    let i3 = ((va >> 30) & 0x1ff) as usize;
    let i2 = ((va >> 21) & 0x1ff) as usize;
    let i1 = ((va >> 12) & 0x1ff) as usize;
    unsafe {
        let pml4 = table_at(cr3_phys());
        let pdpt = ensure_table(&mut (*pml4)[i4]);
        // 1 GiB huge -> 512 x 2 MiB
        let pd = ensure_leaf_parent(&mut (*pdpt)[i3], true, 1 << 21);
        // 2 MiB huge -> 512 x 4 KiB
        let pt = ensure_leaf_parent(&mut (*pd)[i2], false, 1 << 12);
        // present, writable, PWT, PCD, NX: uncacheable MMIO
        (*pt)[i1] = (pa & !0xfff) | 0b11 | (1 << 3) | (1 << 4) | (1u64 << 63);
        core::arch::asm!(
            "invlpg [{v}]",
            v = in(reg) va,
            options(nostack, preserves_flags)
        );
    }
}

/// Map `[phys, phys+size)` as UC MMIO at HHDM+phys. Device memory is not HHDM.
pub fn map_mmio(phys: u64, size: u64) -> Option<usize> {
    if size == 0 {
        return None;
    }
    let start = phys & !0xfff;
    let end = phys.checked_add(size)?.checked_add(0xfff)? & !0xfff;
    let hhdm = limine_boot::hhdm_offset();
    let mut pa = start;
    while pa < end {
        map_one(pa, hhdm + pa);
        pa += 4096;
    }
    Some((hhdm + phys) as usize)
}
