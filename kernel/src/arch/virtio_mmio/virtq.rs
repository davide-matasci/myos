//! Split virtqueue descriptors for the in-kernel virtio-mmio input device
//! (the block drivers are modules and carry their own copy, `modules/virtq`).

pub const DESC_F_WRITE: u16 = 2;
pub const AVAIL_F_NO_INTERRUPT: u16 = 1;

pub const DESC_SIZE: usize = 16;

pub unsafe fn write_desc(base: *mut u8, i: u16, addr: u64, len: u32, flags: u16, next: u16) {
    let p = unsafe { base.add(i as usize * DESC_SIZE) };
    unsafe {
        core::ptr::write_volatile(p as *mut u64, addr);
        core::ptr::write_volatile(p.add(8) as *mut u32, len);
        core::ptr::write_volatile(p.add(12) as *mut u16, flags);
        core::ptr::write_volatile(p.add(14) as *mut u16, next);
    }
}

pub unsafe fn set_avail_no_interrupt(avail: *mut u8) {
    unsafe { core::ptr::write_volatile(avail as *mut u16, AVAIL_F_NO_INTERRUPT) };
}

/// Allocate `n` consecutive 4 KiB frames: `(phys, hhdm va)`.
pub fn alloc_pages(n: usize) -> Option<(u64, *mut u8)> {
    crate::mm::alloc_contiguous_pages(n)
}
