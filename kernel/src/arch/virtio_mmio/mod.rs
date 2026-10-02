//! Virtio-mmio v2 transport shared by the AArch64 and RISC-V `virt` boards:
//! keyboard input (`input`) and the keyboard shim on top of it (block
//! devices are the `virtio_blk` module).
//!
//! Only the transport window and the cache maintenance differ per arch.
//! Polling only; no virtio IRQ.

pub mod input;
pub mod keyboard;
pub mod virtq;

#[cfg(target_arch = "aarch64")]
pub(super) const MMIO_BASE: usize = 0x0A00_0000;
#[cfg(target_arch = "aarch64")]
pub(super) const MMIO_STRIDE: usize = 0x200;
#[cfg(target_arch = "aarch64")]
pub(super) const MMIO_SLOTS: usize = 32;

#[cfg(target_arch = "riscv64")]
pub(super) const MMIO_BASE: usize = 0x1000_1000;
#[cfg(target_arch = "riscv64")]
pub(super) const MMIO_STRIDE: usize = 0x1000;
#[cfg(target_arch = "riscv64")]
pub(super) const MMIO_SLOTS: usize = 8;

pub(super) const MAGIC: u32 = 0x7472_6976; // "virt"
pub(super) const VERSION_2: u32 = 2;

pub(super) const REG_MAGIC: u32 = 0x000;
pub(super) const REG_VERSION: u32 = 0x004;
pub(super) const REG_DEVICE_ID: u32 = 0x008;
pub(super) const REG_DEV_FEAT: u32 = 0x010;
pub(super) const REG_DEV_FEAT_SEL: u32 = 0x014;
pub(super) const REG_DRV_FEAT: u32 = 0x020;
pub(super) const REG_DRV_FEAT_SEL: u32 = 0x024;
pub(super) const REG_QUEUE_SEL: u32 = 0x030;
pub(super) const REG_QUEUE_NUM_MAX: u32 = 0x034;
pub(super) const REG_QUEUE_NUM: u32 = 0x038;
pub(super) const REG_QUEUE_READY: u32 = 0x044;
pub(super) const REG_QUEUE_NOTIFY: u32 = 0x050;
pub(super) const REG_ISR: u32 = 0x060;
pub(super) const REG_ISR_ACK: u32 = 0x064;
pub(super) const REG_STATUS: u32 = 0x070;
pub(super) const REG_DESC_LO: u32 = 0x080;
pub(super) const REG_DESC_HI: u32 = 0x084;
pub(super) const REG_AVAIL_LO: u32 = 0x090;
pub(super) const REG_AVAIL_HI: u32 = 0x094;
pub(super) const REG_USED_LO: u32 = 0x0A0;
pub(super) const REG_USED_HI: u32 = 0x0A4;

pub(super) const ACKNOWLEDGE: u32 = 1;
pub(super) const DRIVER: u32 = 2;
pub(super) const DRIVER_OK: u32 = 4;
pub(super) const FEATURES_OK: u32 = 8;
pub(super) const VIRTIO_F_VERSION_1: u32 = 1; // bit 32, in features dword 1

pub(super) fn r32(base: usize, off: u32) -> u32 {
    unsafe { core::ptr::read_volatile((base + off as usize) as *const u32) }
}

pub(super) fn w32(base: usize, off: u32, v: u32) {
    unsafe { core::ptr::write_volatile((base + off as usize) as *mut u32, v) }
}

pub(super) fn write_phys(base: usize, lo: u32, hi: u32, phys: u64) {
    w32(base, lo, phys as u32);
    w32(base, hi, (phys >> 32) as u32);
}

/// Full memory barrier between CPU and device accesses.
#[cfg(target_arch = "aarch64")]
pub(super) fn dsb() {
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

#[cfg(target_arch = "riscv64")]
pub(super) fn dsb() {
    unsafe {
        core::arch::asm!("fence rw,rw", options(nostack, preserves_flags));
    }
}

/// Clean+invalidate D-cache lines covering `[va, va+len)` so the device and
/// CPU agree on DMA / virtqueue memory. QEMU TCG on AArch64 needs this or
/// `used`/`status` stay stale; RISC-V `virt` is coherent, so a fence suffices.
#[cfg(target_arch = "aarch64")]
pub(super) fn dcache_civac(va: *mut u8, len: usize) {
    if len == 0 {
        return;
    }
    unsafe {
        let mut addr = va as usize & !63;
        let end = va as usize + len;
        while addr < end {
            core::arch::asm!("dc civac, {x}", x = in(reg) addr, options(nostack));
            addr += 64;
        }
        core::arch::asm!("dsb sy", options(nostack));
    }
}

#[cfg(target_arch = "riscv64")]
pub(super) fn dcache_civac(_va: *mut u8, _len: usize) {
    dsb();
}
