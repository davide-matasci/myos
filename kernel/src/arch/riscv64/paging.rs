//! Add a low-half device map to Limine's existing Sv39 root. Do not replace
//! `satp`: unlike AArch64 TTBR0, RISC-V has a single page-table register,
//! so the user window (`upaging`, from VA 0) and the kernel's devices
//! share the low half: a device below the window's end is mapped above it,
//! from [`RELOC_VA`].

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

pub fn pte_is_table(pte: u64) -> bool {
    pte & PTE_V != 0 && pte & (PTE_R | PTE_W | PTE_X) == 0
}

unsafe fn install_mid_root(root: &mut [u64; 512], slot: usize) {
    let mid = &raw mut MID;
    let mid_phys = limine_boot::kernel_virt_to_phys(mid as usize);
    for i in 0..512 {
        unsafe { (*mid).0[i] = pte_leaf_2m((i as u64) << 21, DEV) };
    }
    root[slot] = pte_table(mid_phys);
}

/// Map the low 1 GiB (UART, CLINT, PLIC, virtio-MMIO, PCI ECAM) at
/// [`RELOC_VA`], where [`map_mmio`] finds it, and identity-mapped in the
/// kernel's root for the boot until the platform's devices move there
/// (`apply_platform`): a user address space has its own pages in root[0].
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
        install_mid_root(root, RELOC_SLOT);
        root[0] = root[RELOC_SLOT];
        asm!("sfence.vma", options(nostack));
    }
}

/// Where device memory below the user window's end (`user::USER_WINDOW`,
/// 66 GiB: the low gigabyte's devices, QEMU `virt`'s 32-bit PCIe window at
/// 1 GiB and its 64-bit one at 16 GiB) is mapped instead, at its physical
/// address from here: 128 GiB, above the window and below the upper half.
pub const RELOC_VA: u64 = 0x20_0000_0000;
const RELOC_SLOT: usize = (RELOC_VA >> 30) as usize;
const _: () = assert!(RELOC_VA >= super::user::USER_WINDOW && RELOC_VA + super::user::USER_WINDOW <= 1 << 38);

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

/// Map device MMIO: from [`RELOC_VA`] when `phys` is below the user
/// window's end (so syscalls, on a user satp, can still reach it; the low
/// gigabyte is mapped there already), identity-mapped above.
pub fn map_mmio(phys: u64, size: u64) -> Option<usize> {
    if size == 0 {
        return None;
    }
    let end = phys.checked_add(size)?;
    if end <= 1 << 30 {
        return Some((RELOC_VA + phys) as usize);
    }
    let satp: u64;
    unsafe {
        asm!("csrr {s}, satp", s = out(reg) satp, options(nomem, nostack, preserves_flags));
    }
    if satp >> 60 != 8 {
        return None;
    }
    let root = unsafe { &mut *crate::mm::table(satp_root_phys(satp)) };
    let conflict = phys < super::user::USER_WINDOW;
    let relocate = |pa: u64| if conflict { pa + RELOC_VA } else { pa };
    let pa_end = (end + 0x1F_FFFF) & !0x1F_FFFF;
    let mut pa = phys & !0x1F_FFFF;
    while pa < pa_end {
        map_2m(root, pa, relocate(pa));
        pa += 0x20_0000;
    }
    let va_base = relocate(phys);
    unsafe {
        asm!("sfence.vma", options(nostack));
    }
    Some(va_base as usize)
}
