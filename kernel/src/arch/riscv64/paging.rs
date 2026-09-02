//! Add a low-half device map to Limine's existing Sv39 root. Do not replace
//! `satp`: unlike AArch64 TTBR0, RISC-V has a single page-table register.

use core::arch::asm;

use crate::limine_boot;

#[repr(align(4096))]
struct Table([u64; 512]);

static mut MID: Table = Table([0; 512]);

pub const PTE_V: u64 = 1 << 0;
pub const PTE_R: u64 = 1 << 1;
pub const PTE_W: u64 = 1 << 2;
pub const PTE_X: u64 = 1 << 3;
pub const PTE_U: u64 = 1 << 4;
pub const PTE_A: u64 = 1 << 6;
pub const PTE_D: u64 = 1 << 7;

const DEV: u64 = PTE_V | PTE_R | PTE_W | PTE_A | PTE_D;

/// Physical address encoded in a PTE's PPN field (page-aligned).
pub fn pte_phys(pte: u64) -> u64 {
    (pte >> 10) << 12
}

pub fn pte_table(table_phys: u64) -> u64 {
    ((table_phys >> 12) << 10) | PTE_V
}

pub fn pte_leaf_4k(phys: u64, flags: u64) -> u64 {
    ((phys >> 12) << 10) | flags
}

pub fn pte_leaf_2m(phys_2m: u64, flags: u64) -> u64 {
    debug_assert_eq!(phys_2m & 0x1F_FFFF, 0);
    ((phys_2m >> 12) << 10) | flags
}

pub fn satp_root_phys(satp: u64) -> u64 {
    (satp & ((1 << 44) - 1)) << 12
}

pub fn make_satp(root_phys: u64) -> u64 {
    (8_u64 << 60) | (root_phys >> 12)
}

fn pte_is_table(pte: u64) -> bool {
    pte & PTE_V != 0 && pte & (PTE_R | PTE_W | PTE_X) == 0
}

unsafe fn install_mid_root(root: &mut [u64; 512]) {
    let mid = &raw mut MID;
    let mid_phys = limine_boot::kernel_virt_to_phys(mid as usize);
    for i in 0..512 {
        (*mid).0[i] = pte_leaf_2m((i as u64) << 21, DEV);
    }
    root[0] = pte_table(mid_phys);
}

/// Identity-map the low 1 GiB for UART, CLINT, PLIC, virtio-MMIO, and PCI ECAM.
pub fn map_devices() {
    unsafe {
        let satp: u64;
        asm!("csrr {s}, satp", s = out(reg) satp, options(nomem, nostack, preserves_flags));
        if satp >> 60 != 8 {
            return;
        }
        let root_phys = satp_root_phys(satp);
        if root_phys == 0 {
            return;
        }
        let root = &mut *crate::mm::table(root_phys);
        if root[0] & PTE_V == 0 || !pte_is_table(root[0]) {
            install_mid_root(root);
        } else {
            let mid = &mut *crate::mm::table(pte_phys(root[0]));
            for i in 0..512 {
                mid[i] = pte_leaf_2m((i as u64) << 21, DEV);
            }
        }
        asm!("sfence.vma", options(nostack));
    }
}

/// Sv39 root[1] is user space (`0x4000_0000`). Relocate that window's BAR.
const RELOC_VA: u64 = 0x1_0000_0000;

fn map_2m(root: &mut [u64; 512], pa: u64, va: u64) {
    let i2 = ((va >> 30) & 0x1ff) as usize;
    let i1 = ((va >> 21) & 0x1ff) as usize;
    let mid_pte = root[i2];
    if mid_pte & PTE_V == 0 {
        let mid = crate::mm::alloc_frame();
        root[i2] = pte_table(mid);
    } else if mid_pte & (PTE_R | PTE_W | PTE_X) != 0 {
        return;
    }
    let mid = unsafe { &mut *crate::mm::table(pte_phys(root[i2])) };
    if mid[i1] & PTE_V == 0 {
        mid[i1] = pte_leaf_2m(pa & !0x1F_FFFF, DEV);
    }
}

/// Identity-map device MMIO. If `phys` sits in the user gigabyte, map it at
/// [`RELOC_VA`] instead so syscalls (user satp) can still reach the BAR.
pub fn map_mmio(phys: u64, size: u64) -> Option<usize> {
    if size == 0 {
        return None;
    }
    let end = phys.checked_add(size)?;
    if phys < 0x4000_0000 && end <= 0x4000_0000 {
        return Some(phys as usize);
    }
    let satp: u64;
    unsafe {
        asm!("csrr {s}, satp", s = out(reg) satp, options(nomem, nostack, preserves_flags));
    }
    if satp >> 60 != 8 {
        return None;
    }
    let root = unsafe { &mut *crate::mm::table(satp_root_phys(satp)) };
    let conflict = phys < 0x8000_0000 && end > 0x4000_0000;
    let pa0 = phys & !0x1F_FFFF;
    let va_base = if conflict {
        RELOC_VA + (phys - pa0)
    } else {
        phys
    };
    let pa_end = (end + 0x1F_FFFF) & !0x1F_FFFF;
    let mut pa = pa0;
    while pa < pa_end {
        let va = if conflict { RELOC_VA + (pa - pa0) } else { pa };
        map_2m(root, pa, va);
        pa += 0x20_0000;
    }
    unsafe {
        asm!("sfence.vma", options(nostack));
    }
    Some(va_base as usize)
}
