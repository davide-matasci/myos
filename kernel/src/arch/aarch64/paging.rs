//! Identity-map RAM and MMIO, then turn the EL1 MMU on.
//!
//! QEMU virt starts with the MMU off. Two 1 GiB blocks cover devices
//! (0x0000_0000, including UART and GIC) and RAM (0x4000_0000). That is
//! enough for the kernel, heap, and serial after SCTLR_EL1.M is set.

use core::arch::asm;

#[repr(align(4096))]
struct Table([u64; 512]);

static mut L0: Table = Table([0; 512]);
static mut L1: Table = Table([0; 512]);

const TABLE: u64 = 0b11;
const BLOCK: u64 = 0b01;
const ATTR_DEVICE: u64 = 0 << 2;
const ATTR_NORMAL: u64 = 1 << 2;
const SH_OUTER: u64 = 0b10 << 8;
const SH_INNER: u64 = 0b11 << 8;
const AF: u64 = 1 << 10;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;

pub fn init() {
    unsafe {
        let l0 = &raw mut L0;
        let l1 = &raw mut L1;

        let l1_phys = l1 as u64;
        (*l0).0[0] = l1_phys | TABLE;

        // 0x0000_0000–0x3fff_ffff: devices (UART 0x0900_0000, GIC 0x0800_0000)
        (*l1).0[0] = 0x0000_0000 | BLOCK | ATTR_DEVICE | SH_OUTER | AF | PXN | UXN;
        // 0x4000_0000–0x7fff_ffff: RAM (kernel, stack, heap)
        (*l1).0[1] = 0x4000_0000 | BLOCK | ATTR_NORMAL | SH_INNER | AF | UXN;

        let mair: u64 = 0x04 | (0xFF << 8); // Attr0 device-nGnRE, Attr1 normal WB
        // T0SZ=16 (48-bit VA), inner/outer WBWA, inner shareable, 4K, EPD1,
        // IPS=40-bit.
        let tcr: u64 = 16
            | (0b01 << 8)
            | (0b01 << 10)
            | (0b11 << 12)
            | (1 << 23)
            | (0b010 << 32);
        let ttbr = l0 as u64;
        let sctlr_or: u64 = (1 << 0) | (1 << 2) | (1 << 12); // M | C | I

        asm!(
            "msr mair_el1, {mair}",
            "msr tcr_el1, {tcr}",
            "msr ttbr0_el1, {ttbr}",
            "tlbi vmalle1",
            "dsb sy",
            "isb",
            "mrs {tmp}, sctlr_el1",
            "orr {tmp}, {tmp}, {sctlr_or}",
            "msr sctlr_el1, {tmp}",
            "isb",
            mair = in(reg) mair,
            tcr = in(reg) tcr,
            ttbr = in(reg) ttbr,
            sctlr_or = in(reg) sctlr_or,
            tmp = out(reg) _,
            options(nostack),
        );
    }
}

/// RAM is already identity-mapped; a BSS heap needs no extra PTEs.
pub fn map_writable(_start: usize, _size: usize) {}
