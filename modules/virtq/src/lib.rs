//! Split virtqueue helpers (legacy contiguous or modern split both work),
//! shared by the virtio kernel modules. No allocation here: callers hand in
//! the ring and DMA pages they got from `KernelApi::dma_alloc`.
//!
//! DMA buffers live in cacheable RAM. QEMU TCG on AArch64 still needs
//! D-cache clean/invalidate around device-visible reads/writes or
//! `used`/`status` stay stale; the other arches are coherent and only need a
//! fence.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

use core::sync::atomic::{Ordering, compiler_fence};

pub const DESC_F_NEXT: u16 = 1;
pub const DESC_F_WRITE: u16 = 2;
pub const AVAIL_F_NO_INTERRUPT: u16 = 1;

pub const DESC_SIZE: usize = 16;
pub const SECTOR: usize = 512;

/// DMA page layout used by the block drivers: header at 0, status at 16,
/// one sector at 512.
pub const DMA_STATUS: usize = 16;
pub const DMA_DATA: usize = 512;
pub const VIRTIO_BLK_T_IN: u32 = 0;
pub const VIRTIO_BLK_T_OUT: u32 = 1;

/// Legacy contiguous vring byte size (page-aligned used ring).
pub fn vring_size(num: usize, align: usize) -> usize {
    let after_avail = DESC_SIZE * num + 6 + 2 * num;
    let used_off = after_avail.div_ceil(align) * align;
    used_off + 6 + 8 * num
}

pub fn used_offset(num: usize, align: usize) -> usize {
    let after_avail = DESC_SIZE * num + 6 + 2 * num;
    after_avail.div_ceil(align) * align
}

/// Full memory barrier between CPU and device accesses.
pub fn dsb() {
    compiler_fence(Ordering::SeqCst);
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("fence rw,rw", options(nostack, preserves_flags));
    }
}

/// Clean+invalidate D-cache lines covering `[va, va+len)` so the device and
/// CPU agree on DMA / virtqueue memory (a fence elsewhere).
pub fn dcache_civac(va: *mut u8, len: usize) {
    if len == 0 {
        return;
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let mut addr = va as usize & !63;
        let end = va as usize + len;
        while addr < end {
            core::arch::asm!("dc civac, {x}", x = in(reg) addr, options(nostack));
            addr += 64;
        }
        core::arch::asm!("dsb sy", options(nostack));
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = va;
        dsb();
    }
}

pub unsafe fn write_desc(base: *mut u8, i: u16, addr: u64, len: u32, flags: u16, next: u16) {
    let p = unsafe { base.add(i as usize * DESC_SIZE) };
    unsafe {
        core::ptr::write_volatile(p as *mut u64, addr);
        core::ptr::write_volatile(p.add(8) as *mut u32, len);
        core::ptr::write_volatile(p.add(12) as *mut u16, flags);
        core::ptr::write_volatile(p.add(14) as *mut u16, next);
    }
}

unsafe fn avail_idx_ptr(avail: *mut u8) -> *mut u16 {
    unsafe { avail.add(2) as *mut u16 }
}

unsafe fn used_idx(used: *mut u8) -> u16 {
    unsafe { core::ptr::read_volatile(used.add(2) as *const u16) }
}

pub unsafe fn set_avail_no_interrupt(avail: *mut u8) {
    unsafe { core::ptr::write_volatile(avail as *mut u16, AVAIL_F_NO_INTERRUPT) };
}

/// One split virtqueue and the single-sector DMA page the block drivers use.
pub struct Ring {
    pub num: u16,
    pub desc: *mut u8,
    pub avail: *mut u8,
    pub used: *mut u8,
    pub last_used: u16,
    pub dma_phys: u64,
    pub dma_va: *mut u8,
}

impl Ring {
    /// Submit descriptor chain `head` and spin until the device returns it.
    pub unsafe fn push_and_wait(&mut self, head: u16, notify: impl FnOnce()) -> Result<(), ()> {
        let idx = unsafe { core::ptr::read_volatile(avail_idx_ptr(self.avail)) };
        let slot = (idx as usize) % (self.num as usize);
        unsafe {
            core::ptr::write_volatile(self.avail.add(4 + slot * 2) as *mut u16, head);
        }
        compiler_fence(Ordering::Release);
        unsafe {
            core::ptr::write_volatile(avail_idx_ptr(self.avail), idx.wrapping_add(1));
        }
        compiler_fence(Ordering::SeqCst);
        notify();

        let want = self.last_used.wrapping_add(1);
        for _ in 0..50_000_000u32 {
            compiler_fence(Ordering::SeqCst);
            // Drop stale used-ring lines so the device's DMA write is visible.
            dcache_civac(self.used, 4096);
            if unsafe { used_idx(self.used) } == want {
                self.last_used = want;
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(())
    }

    /// One-sector block request: header at `dma_phys+0`, status at +16,
    /// data at +512 (`out` is filled on a read, `data` sent on a write).
    unsafe fn one_sector(
        &mut self,
        lba: u64,
        data: Option<&[u8]>,
        out: Option<&mut [u8]>,
        notify: impl FnOnce(),
    ) -> Result<(), ()> {
        let is_write = data.is_some();
        let dma_va = self.dma_va;
        let dma_phys = self.dma_phys;
        unsafe {
            core::ptr::write_volatile(
                dma_va as *mut u32,
                if is_write { VIRTIO_BLK_T_OUT } else { VIRTIO_BLK_T_IN },
            );
            core::ptr::write_volatile(dma_va.add(4) as *mut u32, 0);
            core::ptr::write_volatile(dma_va.add(8) as *mut u64, lba);
            core::ptr::write_volatile(dma_va.add(DMA_STATUS), 0xFFu8);
            if let Some(d) = data {
                core::ptr::copy_nonoverlapping(d.as_ptr(), dma_va.add(DMA_DATA), SECTOR);
            }
            write_desc(self.desc, 0, dma_phys, 16, DESC_F_NEXT, 1);
            write_desc(
                self.desc,
                1,
                dma_phys + DMA_DATA as u64,
                SECTOR as u32,
                DESC_F_NEXT | if is_write { 0 } else { DESC_F_WRITE },
                2,
            );
            write_desc(self.desc, 2, dma_phys + DMA_STATUS as u64, 1, DESC_F_WRITE, 0);
            dcache_civac(self.desc, 4096);
            dcache_civac(self.avail, 4096);
            dcache_civac(dma_va, DMA_DATA + SECTOR);
            self.push_and_wait(0, notify)?;
            dcache_civac(dma_va, DMA_DATA + SECTOR);
            let status = core::ptr::read_volatile(dma_va.add(DMA_STATUS));
            if status != 0 {
                return Err(());
            }
            if let Some(o) = out {
                core::ptr::copy_nonoverlapping(dma_va.add(DMA_DATA), o.as_mut_ptr(), SECTOR);
            }
        }
        Ok(())
    }

    /// Read `buf.len()` bytes (a multiple of [`SECTOR`]) from `lba`.
    pub unsafe fn read_buf(&mut self, mut lba: u64, buf: &mut [u8], mut notify: impl FnMut()) -> Result<(), ()> {
        for chunk in buf.chunks_mut(SECTOR) {
            if chunk.len() != SECTOR {
                return Err(());
            }
            unsafe { self.one_sector(lba, None, Some(chunk), &mut notify)? };
            lba += 1;
        }
        Ok(())
    }

    /// Write `buf.len()` bytes (a multiple of [`SECTOR`]) at `lba`.
    pub unsafe fn write_buf(&mut self, mut lba: u64, buf: &[u8], mut notify: impl FnMut()) -> Result<(), ()> {
        for chunk in buf.chunks(SECTOR) {
            if chunk.len() != SECTOR {
                return Err(());
            }
            unsafe { self.one_sector(lba, Some(chunk), None, &mut notify)? };
            lba += 1;
        }
        Ok(())
    }
}
