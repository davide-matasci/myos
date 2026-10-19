//! DMA pages: from the kernel (`dma_alloc`, never returned) through a pool
//! this module recycles, since devices come and go; each controller's own
//! (its DMA handle, `dma_attach`: what it reaches, whether it sees the
//! caches). [`clean`] and [`invalidate`] keep a controller without coherent
//! DMA and the caches in step (`dma_sync`).

use core::sync::atomic::{Ordering, compiler_fence};

use myos_abi::{DMA_FROM_DEVICE, DMA_TO_DEVICE, Lock};

use crate::api;

const POOL: usize = 128;

/// The pages given back, `(dma handle, bus address, va)`, the last freed
/// first.
struct Pool {
    free: [(i32, u64, usize); POOL],
    n: usize,
}

static FREE: Lock<Pool> = Lock::new(Pool { free: [(-1, 0, 0); POOL], n: 0 });

/// One zeroed page of controller `dma`: `(bus address, va)`.
pub fn page(dma: i32) -> Option<(u64, *mut u8)> {
    let recycled = {
        let mut pool = FREE.lock();
        let n = pool.n;
        let hit = pool.free[..n].iter().rposition(|&(d, _, _)| d == dma);
        hit.map(|i| {
            let page = pool.free[i];
            pool.free[i] = pool.free[n - 1];
            pool.n = n - 1;
            page
        })
    };
    if let Some((_, bus, va)) = recycled {
        let va = va as *mut u8;
        unsafe {
            core::ptr::write_bytes(va, 0, 4096);
        }
        clean(dma, va, 4096);
        return Some((bus, va));
    }
    pages(dma, 1)
}

/// `n` contiguous zeroed pages of controller `dma` from the kernel.
pub fn pages(dma: i32, n: usize) -> Option<(u64, *mut u8)> {
    let mut bus = 0u64;
    let va = api().dma_alloc(dma, n, &mut bus);
    if va.is_null() { None } else { Some((bus, va)) }
}

/// A page back to the pool (dropped when the pool is full).
pub fn free(dma: i32, bus: u64, va: *mut u8) {
    let mut pool = FREE.lock();
    if pool.n < POOL {
        let n = pool.n;
        pool.free[n] = (dma, bus, va as usize);
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

/// Hand `len` bytes at `va` over to controller `dma`: what the CPU wrote
/// reaches it, and nothing cached is written over what it is to write.
pub fn clean(dma: i32, va: *mut u8, len: usize) {
    api().dma_sync(dma, va, len, DMA_TO_DEVICE);
}

/// Read what controller `dma` wrote at `va`.
pub fn invalidate(dma: i32, va: *mut u8, len: usize) {
    api().dma_sync(dma, va, len, DMA_FROM_DEVICE);
    compiler_fence(Ordering::SeqCst);
}

/// `Controller::mmio`: the register base, for the duplicate check.
impl crate::hc::Controller {
    pub fn mmio(&self) -> usize {
        self.mmio_base
    }
}
