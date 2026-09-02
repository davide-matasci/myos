//! Map QEMU virt MMIO on TTBR0. Limine already owns TTBR1 (higher half + HHDM).
//!
//! Base revision 3+ HHDM does not include device MMIO, so UART (0x0900_0000)
//! and GICv2 (0x0800_0000 / 0x0801_0000) need their own identity map. TTBR0
//! is unspecified at handoff and free for the kernel.

use core::arch::asm;

use crate::limine_boot;

#[repr(align(4096))]
struct Table([u64; 512]);

static mut L0: Table = Table([0; 512]);
static mut L1: Table = Table([0; 512]);

const TABLE: u64 = 0b11;
const BLOCK: u64 = 0b01;
const ATTR_DEVICE: u64 = 2 << 2; // MAIR Attr2: device-nGnRE (we install it)
const SH_OUTER: u64 = 0b10 << 8;
const AF: u64 = 1 << 10;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;

pub fn map_devices() {
    unsafe {
        let l0 = &raw mut L0;
        let l1 = &raw mut L1;
        let l1_phys = limine_boot::kernel_virt_to_phys(l1 as usize);
        let l0_phys = limine_boot::kernel_virt_to_phys(l0 as usize);

        (*l0).0[0] = l1_phys | TABLE;
        // 0x0000_0000-0x3fff_ffff: devices (UART, GIC, PCI ECAM/MMIO)
        (*l1).0[0] = 0x0000_0000 | BLOCK | ATTR_DEVICE | SH_OUTER | AF | PXN | UXN;

        let mut mair: u64;
        asm!("mrs {m}, mair_el1", m = out(reg) mair, options(nomem, nostack, preserves_flags));
        mair |= 0x04 << 16; // Attr2 = device-nGnRE; leave Attr0/Attr1 alone

        let ttbr = l0_phys;
        let el: u64;
        asm!("mrs {el}, CurrentEL", el = out(reg) el, options(nomem, nostack, preserves_flags));
        let el = (el >> 2) & 3;

        asm!(
            "msr mair_el1, {mair}",
            "msr ttbr0_el1, {ttbr}",
            "dsb sy",
            mair = in(reg) mair,
            ttbr = in(reg) ttbr,
            options(nostack),
        );
        if el >= 2 {
            asm!("tlbi alle2is", options(nostack));
        } else {
            asm!("tlbi vmalle1", options(nostack));
        }
        asm!("dsb sy; isb", options(nostack));
    }
    enable_el0_cache_ops();
}

/// Allow EL0 `DC CVAU` / `IC IVAU` / `MRS CTR_EL0` (TinyCC `__clear_cache`).
///
/// SCTLR.UCI=0 traps those SYS insns from EL0 with ESR.EC=0x18 (CI aarch64
/// tcc -run: `dc cvau` in libgcc __clear_cache right after mprotect RX).
/// Write both EL1 and EL2 banks: Limine may be at EL1 or EL2+VHE (TGE uses
/// SCTLR_EL2 for EL0).
fn enable_el0_cache_ops() {
    const UCI: u64 = 1 << 26;
    const UCT: u64 = 1 << 15;
    const DZE: u64 = 1 << 14;
    const BITS: u64 = UCI | UCT | DZE;
    unsafe {
        let el: u64;
        asm!("mrs {el}, CurrentEL", el = out(reg) el, options(nomem, nostack, preserves_flags));
        let el = (el >> 2) & 3;
        let mut sctlr: u64;
        asm!("mrs {s}, sctlr_el1", s = out(reg) sctlr, options(nomem, nostack, preserves_flags));
        sctlr |= BITS;
        asm!("msr sctlr_el1, {s}", "isb", s = in(reg) sctlr, options(nostack));
        if el >= 2 {
            asm!("mrs {s}, sctlr_el2", s = out(reg) sctlr, options(nomem, nostack, preserves_flags));
            sctlr |= BITS;
            asm!("msr sctlr_el2, {s}", "isb", s = in(reg) sctlr, options(nostack));
        }
    }
}

const PA: u64 = 0x0000_FFFF_FFFF_F000;
const DEV_BLOCK: u64 = BLOCK | ATTR_DEVICE | SH_OUTER | AF | PXN | UXN;

fn tlbi() {
    unsafe {
        let el: u64;
        asm!("mrs {el}, CurrentEL", el = out(reg) el, options(nomem, nostack, preserves_flags));
        let el = (el >> 2) & 3;
        if el >= 2 {
            asm!("tlbi alle2is", options(nostack));
        } else {
            asm!("tlbi vmalle1", options(nostack));
        }
        asm!("dsb sy; isb", options(nostack));
    }
}

fn map_block_1g(pa: u64) -> Option<()> {
    let i0 = ((pa >> 39) & 0x1ff) as usize;
    let i1 = ((pa >> 30) & 0x1ff) as usize;
    let block_pa = pa & !0x3FFF_FFFF;
    unsafe {
        let l0 = &raw mut L0;
        if (*l0).0[i0] & 0b11 != TABLE {
            let l1phys = crate::mm::alloc_frame();
            (*l0).0[i0] = l1phys | TABLE;
        }
        let l1phys = (*l0).0[i0] & PA;
        let l1 = crate::mm::table(l1phys);
        let cur = (*l1)[i1];
        if cur == 0 {
            (*l1)[i1] = block_pa | DEV_BLOCK;
        }
    }
    Some(())
}

/// Identity-map device MMIO. The 0–1 GiB window is already mapped.
/// Extra 1 GiB device blocks are installed if a BAR sits above that.
pub fn map_mmio(phys: u64, size: u64) -> Option<usize> {
    if size == 0 {
        return None;
    }
    let end = phys.checked_add(size)?;
    if phys < 0x4000_0000 && end <= 0x4000_0000 {
        return Some(phys as usize);
    }
    let mut pa = phys & !0x3FFF_FFFF;
    while pa < end {
        map_block_1g(pa)?;
        pa = pa.checked_add(0x4000_0000)?;
    }
    tlbi();
    Some(phys as usize)
}
