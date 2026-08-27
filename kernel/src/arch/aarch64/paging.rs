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
        // 0x0000_0000-0x3fff_ffff: devices (UART, GIC)
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
}
