//! DMA pages: from the kernel (`dma_alloc`, never returned) through a pool
//! this module recycles, since devices come and go. The cache maintenance
//! the arches without coherent DMA need (aarch64), as the NVMe module does it.

use core::sync::atomic::{Ordering, compiler_fence};

use myos_abi::Lock;

use crate::api;

const POOL: usize = 128;

/// The pages given back, `(phys, va)`, the last freed first.
struct Pool {
    free: [(u64, usize); POOL],
    n: usize,
}

static FREE: Lock<Pool> = Lock::new(Pool { free: [(0, 0); POOL], n: 0 });

/// One zeroed page: `(phys, va)`.
pub fn page() -> Option<(u64, *mut u8)> {
    let recycled = {
        let mut pool = FREE.lock();
        if pool.n > 0 {
            pool.n -= 1;
            Some(pool.free[pool.n])
        } else {
            None
        }
    };
    if let Some((phys, va)) = recycled {
        let va = va as *mut u8;
        unsafe {
            core::ptr::write_bytes(va, 0, 4096);
        }
        return Some((phys, va));
    }
    pages(1)
}

/// `n` contiguous pages from the kernel.
pub fn pages(n: usize) -> Option<(u64, *mut u8)> {
    let mut phys = 0u64;
    let va = api().dma_alloc(n, &mut phys);
    if va.is_null() { None } else { Some((phys, va)) }
}

/// A page back to the pool (dropped when the pool is full).
pub fn free(phys: u64, va: *mut u8) {
    let mut pool = FREE.lock();
    if pool.n < POOL {
        let n = pool.n;
        pool.free[n] = (phys, va as usize);
        pool.n = n + 1;
    }
}

/// Writes to memory reach the device before the doorbell.
pub fn wmb() {
    compiler_fence(Ordering::Release);
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("fence rw,rw", options(nostack, preserves_flags));
    }
}

/// Make `len` bytes at `va` visible to the device (clean the data cache).
pub fn clean(va: *mut u8, len: usize) {
    let _ = (va, len);
    #[cfg(target_arch = "aarch64")]
    unsafe {
        if len == 0 {
            return;
        }
        let mut addr = va as usize & !63;
        let end = va as usize + len;
        while addr < end {
            core::arch::asm!("dc civac, {x}", x = in(reg) addr, options(nostack));
            addr += 64;
        }
        core::arch::asm!("dsb sy", options(nostack));
    }
    wmb();
}

/// Read what the device wrote at `va` (invalidate the data cache).
pub fn invalidate(va: *mut u8, len: usize) {
    clean(va, len);
    compiler_fence(Ordering::SeqCst);
}

/// `Controller::mmio`: the register base, for the duplicate check.
impl crate::hc::Controller {
    pub fn mmio(&self) -> usize {
        self.mmio_base
    }
}
