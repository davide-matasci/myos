//! Physical frame bump allocator. Starts after the 256 KiB heap and the
//! loaded kernel image in physical memory.
//!
//! `limine_boot::alloc_usable` does not bump, so a second call would overlap
//! the heap. Page tables and user pages come from here instead.

use core::sync::atomic::{AtomicU64, Ordering};

use limine::memmap;

use crate::heap::HEAP_SIZE;
use crate::limine_boot;

const PAGE: u64 = 4096;
const SKIP: u64 = 64 * 1024;

static NEXT: AtomicU64 = AtomicU64::new(0);

fn heap_phys() -> u64 {
    let entries = limine_boot::MEMMAP
        .response()
        .expect("Limine memmap")
        .entries();
    let need = HEAP_SIZE as u64 + SKIP;
    for e in entries {
        if e.type_ != memmap::MEMMAP_USABLE {
            continue;
        }
        if e.length >= need {
            return (e.base + SKIP + 0xfff) & !0xfff;
        }
    }
    panic!("no usable Limine memory for heap");
}

/// First physical address safe for the frame bump allocator.
fn frame_alloc_base() -> u64 {
    let after_heap = heap_phys() + HEAP_SIZE as u64;
    let after_kernel = kernel_phys_end();
    (after_heap.max(after_kernel) + 0xfff) & !0xfff
}

/// Physical end of the Limine-loaded kernel (page-aligned).
fn kernel_phys_end() -> u64 {
    unsafe extern "C" {
        static _end: u8;
    }
    let end_va = core::ptr::addr_of!(_end) as usize;
    limine_boot::kernel_virt_to_phys(end_va)
}

/// Allocate a 4 KiB frame, zero it, return its physical address.
pub fn alloc_frame() -> u64 {
    let hhdm = limine_boot::hhdm_offset();
    let entries = limine_boot::MEMMAP
        .response()
        .expect("Limine memmap")
        .entries();

    let mut next = NEXT.load(Ordering::SeqCst);
    if next == 0 {
        next = frame_alloc_base();
    }
    next = (next + 0xfff) & !0xfff;

    for e in entries {
        if e.type_ != memmap::MEMMAP_USABLE {
            continue;
        }
        let mut phys = e.base.max(next);
        phys = (phys + 0xfff) & !0xfff;
        if phys < e.base {
            continue;
        }
        if phys.saturating_add(PAGE) <= e.base + e.length {
            NEXT.store(phys + PAGE, Ordering::SeqCst);
            unsafe {
                core::ptr::write_bytes((phys + hhdm) as *mut u8, 0, PAGE as usize);
            }
            return phys;
        }
    }
    panic!("out of usable memory");
}

pub fn hhdm(phys: u64) -> *mut u8 {
    (phys + limine_boot::hhdm_offset()) as *mut u8
}

pub fn table(phys: u64) -> *mut [u64; 512] {
    hhdm(phys & !0xfff) as *mut [u64; 512]
}
