//! Split virtqueue helpers (legacy contiguous or modern split both work).

use core::sync::atomic::{compiler_fence, Ordering};

pub const DESC_F_NEXT: u16 = 1;
pub const DESC_F_WRITE: u16 = 2;
pub const AVAIL_F_NO_INTERRUPT: u16 = 1;

pub const DESC_SIZE: usize = 16;

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
    // AArch64: caller must D-cache-invalidate `used` before this read when
    // the device updated it via DMA into cacheable RAM.
    unsafe { core::ptr::read_volatile(used.add(2) as *const u16) }
}

pub unsafe fn set_avail_no_interrupt(avail: *mut u8) {
    unsafe { core::ptr::write_volatile(avail as *mut u16, AVAIL_F_NO_INTERRUPT) };
}

/// Submit descriptor chain `head` and spin until the device returns it.
pub unsafe fn push_and_wait(
    num: u16,
    avail: *mut u8,
    used: *mut u8,
    last_used: &mut u16,
    head: u16,
    notify: impl FnOnce(),
) -> Result<(), ()> {
    let idx = unsafe { core::ptr::read_volatile(avail_idx_ptr(avail)) };
    let slot = (idx as usize) % (num as usize);
    unsafe {
        core::ptr::write_volatile(avail.add(4 + slot * 2) as *mut u16, head);
    }
    compiler_fence(Ordering::Release);
    unsafe {
        core::ptr::write_volatile(avail_idx_ptr(avail), idx.wrapping_add(1));
    }
    compiler_fence(Ordering::SeqCst);
    notify();

    let want = last_used.wrapping_add(1);
    for _ in 0..50_000_000u32 {
        compiler_fence(Ordering::SeqCst);
        #[cfg(target_arch = "aarch64")]
        unsafe {
            // Drop stale used-ring lines so the device's DMA write is visible.
            let mut addr = used as usize & !63;
            let end = used as usize + 4096;
            while addr < end {
                core::arch::asm!("dc civac, {x}", x = in(reg) addr, options(nostack));
                addr += 64;
            }
            core::arch::asm!("dsb sy", options(nostack));
        }
        if unsafe { used_idx(used) } == want {
            *last_used = want;
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(())
}

/// One-sector IN: header at `dma_phys+0`, status at +16, data at +512.
pub unsafe fn read_one(
    num: u16,
    desc: *mut u8,
    avail: *mut u8,
    used: *mut u8,
    last_used: &mut u16,
    dma_phys: u64,
    dma_va: *mut u8,
    lba: u64,
    out: &mut [u8],
    notify: impl FnOnce(),
) -> Result<(), ()> {
    debug_assert!(out.len() == crate::blk::SECTOR);
    unsafe {
        core::ptr::write_volatile(dma_va as *mut u32, crate::blk::VIRTIO_BLK_T_IN);
        core::ptr::write_volatile(dma_va.add(4) as *mut u32, 0);
        core::ptr::write_volatile(dma_va.add(8) as *mut u64, lba);
        core::ptr::write_volatile(dma_va.add(crate::blk::DMA_STATUS), 0xFFu8);
        write_desc(
            desc,
            0,
            dma_phys,
            16,
            DESC_F_NEXT,
            1,
        );
        write_desc(
            desc,
            1,
            dma_phys + crate::blk::DMA_DATA as u64,
            crate::blk::SECTOR as u32,
            DESC_F_NEXT | DESC_F_WRITE,
            2,
        );
        write_desc(
            desc,
            2,
            dma_phys + crate::blk::DMA_STATUS as u64,
            1,
            DESC_F_WRITE,
            0,
        );
        push_and_wait(num, avail, used, last_used, 0, notify)?;
        #[cfg(target_arch = "aarch64")]
        {
            let mut addr = dma_va as usize & !63;
            let end = dma_va as usize + 512 + 16;
            while addr < end {
                core::arch::asm!("dc civac, {x}", x = in(reg) addr, options(nostack));
                addr += 64;
            }
            core::arch::asm!("dsb sy", options(nostack));
        }
        let status = core::ptr::read_volatile(dma_va.add(crate::blk::DMA_STATUS));
        if status != 0 {
            return Err(());
        }
        core::ptr::copy_nonoverlapping(
            dma_va.add(crate::blk::DMA_DATA),
            out.as_mut_ptr(),
            crate::blk::SECTOR,
        );
    }
    Ok(())
}

pub unsafe fn read_buf(
    num: u16,
    desc: *mut u8,
    avail: *mut u8,
    used: *mut u8,
    last_used: &mut u16,
    dma_phys: u64,
    dma_va: *mut u8,
    mut lba: u64,
    buf: &mut [u8],
    mut notify: impl FnMut(),
) -> Result<(), ()> {
    for chunk in buf.chunks_mut(crate::blk::SECTOR) {
        unsafe {
            read_one(
                num,
                desc,
                avail,
                used,
                last_used,
                dma_phys,
                dma_va,
                lba,
                chunk,
                || notify(),
            )?;
        }
        lba += 1;
    }
    Ok(())
}

/// Allocate `n` consecutive 4 KiB frames. Fails if the bump allocator
/// crossed a memmap hole (should not happen for a handful of pages).
pub fn alloc_pages(n: usize) -> Option<(u64, *mut u8)> {
    if n == 0 {
        return None;
    }
    let first = crate::mm::alloc_frame();
    for i in 1..n {
        let p = crate::mm::alloc_frame();
        if p != first + (i as u64) * 4096 {
            return None;
        }
    }
    Some((first, crate::mm::hhdm(first)))
}
