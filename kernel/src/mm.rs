//! Physical frame bump allocator. Starts after the kernel heap.
//!
//! Skips the Limine-loaded kernel image when it sits inside a usable region
//! (common on AArch64). Does **not** bump the cursor to `kernel_end` globally —
//! on x86 that can sit above all usable RAM and starve the allocator.
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

/// Physical `[start, end)` of the Limine-loaded kernel image.
fn kernel_phys_range() -> (u64, u64) {
    unsafe extern "C" {
        static _end: u8;
    }
    let r = limine_boot::EXECUTABLE_ADDRESS
        .response()
        .expect("Limine executable address");
    let end_va = core::ptr::addr_of!(_end) as u64;
    let start = r.physical_base;
    let end = (end_va - r.virtual_base + r.physical_base + 0xfff) & !0xfff;
    (start, end)
}

/// True if `[phys, phys+PAGE)` overlaps the loaded kernel image.
fn overlaps_kernel(phys: u64) -> bool {
    let (k0, k1) = kernel_phys_range();
    phys < k1 && phys.saturating_add(PAGE) > k0
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
        next = heap_phys() + HEAP_SIZE as u64;
    }
    next = (next + 0xfff) & !0xfff;

    for e in entries {
        if e.type_ != memmap::MEMMAP_USABLE {
            continue;
        }
        let region_end = e.base + e.length;
        let mut phys = e.base.max(next);
        phys = (phys + 0xfff) & !0xfff;
        while phys.saturating_add(PAGE) <= region_end {
            if overlaps_kernel(phys) {
                let (_, k1) = kernel_phys_range();
                phys = (k1 + 0xfff) & !0xfff;
                continue;
            }
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
