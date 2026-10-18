//! Device MMIO, in a window of the kernel's higher half (TTBR1, which
//! Limine owns: the higher half and the HHDM). Base revision 3+ HHDM does
//! not include device MMIO, so the UART, the GIC, the RTC, the PCIe ECAM
//! and BARs and the virtio-mmio windows (addresses from the platform
//! description) are mapped here, at `DEV_WINDOW + phys`, in 1 GiB
//! device-nGnRE blocks.
//!
//! The window is not TTBR0: user address spaces live there from 1 GiB on
//! (`upaging`), where a board's devices may well be (a Raspberry Pi 4's
//! are just below 4 GiB). TTBR0 holds an empty kernel table for kernel
//! threads, and each process's own.
//!
//! A 1 GiB block may cover RAM too (a board with devices and memory in one
//! gigabyte): that RAM then has a device alias here, never accessed, beside
//! its normal HHDM mapping.

use core::arch::asm;

use spin::Mutex;

use crate::limine_boot;

#[repr(align(4096))]
struct Table([u64; 512]);

/// The kernel's TTBR0: no mappings (kernel threads touch no user memory).
static mut L0: Table = Table([0; 512]);
/// The window's L1 tables, each hung off the L0 slot of the first device
/// in its 512 GiB of physical addresses: static, since the UART and the
/// GIC are mapped before there is a frame allocator.
static mut WINDOW_L1: [Table; WINDOW_L1_TABLES] = [const { Table([0; 512]) }; WINDOW_L1_TABLES];
/// Which 512 GiB each of [`WINDOW_L1`] covers (physical address >> 39),
/// for the first `used` of them; under the lock (`map_mmio` from modules
/// on any CPU).
struct Window {
    region: [usize; WINDOW_L1_TABLES],
    used: usize,
}

static WINDOW: Mutex<Window> = Mutex::new(Window {
    region: [0; WINDOW_L1_TABLES],
    used: 0,
});

/// Where physical address 0 is in the window: L0 slot 256 of TTBR1, the
/// first above the HHDM's (Limine's HHDM starts at the bottom of the
/// higher half, `0xffff_0000_0000_0000`, and spans the RAM).
const DEV_WINDOW: u64 = 0xffff_8000_0000_0000;
/// The window's L0 slots: 256 up to the kernel's (511), so physical
/// addresses below 127.5 TiB (an Ampere Altra's PCIe sits near 48 TiB).
const WINDOW_FIRST_SLOT: usize = (DEV_WINDOW >> 39) as usize & 0x1ff;
const WINDOW_SLOTS: usize = 511 - WINDOW_FIRST_SLOT;
/// The 512 GiB regions with devices the window can hold: QEMU `virt` has
/// two (its 64-bit PCIe window is at 512 GiB), a two-socket server about
/// a dozen.
const WINDOW_L1_TABLES: usize = 16;

const TABLE: u64 = 0b11;
const BLOCK: u64 = 0b01;
const ATTR_DEVICE: u64 = 2 << 2; // MAIR Attr2: device-nGnRE (we install it)
const SH_OUTER: u64 = 0b10 << 8;
const AF: u64 = 1 << 10;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;

/// The kernel's TTBR0 (an empty table) and MAIR Attr2 for the device
/// blocks; then the window's L1 tables into TTBR1.
pub fn map_devices() {
    unsafe {
        let l0 = &raw mut L0;
        let l0_phys = limine_boot::kernel_virt_to_phys(l0 as usize);

        let mut mair: u64;
        asm!("mrs {m}, mair_el1", m = out(reg) mair, options(nomem, nostack, preserves_flags));
        mair |= 0x04 << 16; // Attr2 = device-nGnRE; leave Attr0/Attr1 alone

        asm!(
            "msr mair_el1, {mair}",
            "msr ttbr0_el1, {ttbr}",
            "dsb sy",
            mair = in(reg) mair,
            ttbr = in(reg) l0_phys,
            options(nostack),
        );
        tlbi();
    }
    install_window();
    enable_el0_cache_ops();
}

/// TTBR1's L0 table, through the HHDM.
fn ttbr1_l0() -> *mut [u64; 512] {
    let ttbr1: u64;
    unsafe {
        asm!("mrs {t}, ttbr1_el1", t = out(reg) ttbr1, options(nomem, nostack, preserves_flags));
    }
    crate::mm::table(ttbr1 & PA)
}

/// The higher half has 48 bits (TCR_EL1.T1SZ 16): the four-level layout
/// the window's L0 slots assume.
fn higher_half_48() -> bool {
    let tcr: u64;
    unsafe {
        asm!("mrs {t}, tcr_el1", t = out(reg) tcr, options(nomem, nostack, preserves_flags));
    }
    (tcr >> 16) & 0x3f == 16
}

/// Check that the window's L0 slots of TTBR1 are there and empty (Limine
/// leaves them so); its L1 tables are hung off them as devices are mapped.
/// Otherwise (another layout, a slot in use) `map_mmio` maps nothing and
/// the boot stops at its first device.
fn install_window() {
    if !higher_half_48() {
        return;
    }
    unsafe {
        let l0 = &*ttbr1_l0();
        if l0[WINDOW_FIRST_SLOT..WINDOW_FIRST_SLOT + WINDOW_SLOTS].iter().any(|&e| e != 0) {
            return;
        }
    }
    WINDOW_READY.store(true, core::sync::atomic::Ordering::SeqCst);
}

/// The window's L1 tables are in TTBR1.
static WINDOW_READY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Install the kernel's TTBR0 and MAIR Attr2 on a secondary CPU.
///
/// Limine's AP trampoline snapshots TTBR0/MAIR at bring-up (before the kernel
/// runs `map_devices`). Without Attr2 the window's device blocks would be
/// read with whatever attributes index 2 had (TTBR1 itself is shared).
pub fn apply_bsp_device_map() {
    unsafe {
        let l0 = &raw mut L0;
        let l0_phys = limine_boot::kernel_virt_to_phys(l0 as usize);
        let mut mair: u64;
        asm!("mrs {m}, mair_el1", m = out(reg) mair, options(nomem, nostack, preserves_flags));
        mair |= 0x04 << 16;
        let el: u64;
        asm!("mrs {el}, CurrentEL", el = out(reg) el, options(nomem, nostack, preserves_flags));
        let el = (el >> 2) & 3;
        asm!(
            "msr mair_el1, {mair}",
            "msr ttbr0_el1, {ttbr}",
            "dsb sy",
            mair = in(reg) mair,
            ttbr = in(reg) l0_phys,
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

/// The window's 1 GiB device block for physical `pa`, and the L1 table of
/// its 512 GiB first if it has none yet.
fn map_block_1g(w: &mut Window, pa: u64) -> Option<()> {
    let region = (pa >> 39) as usize;
    if region >= WINDOW_SLOTS {
        return None;
    }
    let table = match w.region[..w.used].iter().position(|&r| r == region) {
        Some(t) => t,
        None => {
            let t = w.used;
            if t == WINDOW_L1_TABLES {
                return None;
            }
            unsafe {
                let l1 = &raw mut WINDOW_L1[t];
                (*ttbr1_l0())[WINDOW_FIRST_SLOT + region] = limine_boot::kernel_virt_to_phys(l1 as usize) | TABLE;
            }
            w.region[t] = region;
            w.used += 1;
            t
        }
    };
    let i1 = ((pa >> 30) & 0x1ff) as usize;
    unsafe {
        let l1 = &raw mut WINDOW_L1[table];
        if (*l1).0[i1] == 0 {
            (*l1).0[i1] = (pa & !0x3FFF_FFFF) | DEV_BLOCK;
        }
    }
    Some(())
}

/// Map device MMIO at `phys` (`size` bytes) into the window: its kernel
/// virtual address, or `None` (beyond the window, or no window).
pub fn map_mmio(phys: u64, size: u64) -> Option<usize> {
    if size == 0 || !WINDOW_READY.load(core::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    let end = phys.checked_add(size)?;
    let mut w = WINDOW.lock();
    let mut pa = phys & !0x3FFF_FFFF;
    while pa < end {
        map_block_1g(&mut w, pa)?;
        pa = pa.checked_add(0x4000_0000)?;
    }
    // New entries only (nothing to invalidate on any CPU): make the table
    // writes visible before the first access.
    unsafe {
        asm!("dsb ish; isb", options(nostack));
    }
    Some((DEV_WINDOW + phys) as usize)
}
