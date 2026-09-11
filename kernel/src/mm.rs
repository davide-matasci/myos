//! Physical frame bump allocator with a free-list for reclaimed user pages.
//!
//! Starts after the kernel heap. Skips the Limine-loaded kernel image when it
//! sits inside a usable region (common on AArch64). Does **not** bump the
//! cursor to `kernel_end` globally — on x86 that can sit above all usable RAM
//! and starve the allocator.
//!
//! `limine_boot::alloc_usable` does not bump, so a second call would overlap
//! the heap. Page tables and user pages come from here instead.
//!
//! Freed user frames (process exit / abandoned exec aspace) go onto an
//! intrusive freelist so the `/heap` smoke can fork+exec large ELFs many times
//! without walking the bump allocator into garbage (riscv64 `sepc=0` after
//! find+cat+ls+rg — same class as #84).

use core::sync::atomic::{AtomicU64, Ordering};

use limine::memmap;

use crate::heap::HEAP_SIZE;
use crate::limine_boot;

const PAGE: u64 = 4096;
const SKIP: u64 = 64 * 1024;

static NEXT: AtomicU64 = AtomicU64::new(0);
/// Intrusive freelist head (physical address), or 0. Each free page stores the
/// next phys at offset 0 via HHDM.
static FREE_HEAD: AtomicU64 = AtomicU64::new(0);

// --- Frame-ownership bitmap (double-free / double-alloc detector) ---
// One bit per RAM frame: 1 = allocated (in use), 0 = free/never allocated.
// Catches the freelist-cycle corruption class (free_frame called twice on the
// same phys → freelist node points at itself → the same frame handed out
// twice → two owners scribble over each other: corrupted TLS handshakes,
// git objects, user data) at the allocator, before the damage spreads.
static FB_READY: AtomicU64 = AtomicU64::new(0); // 0=uninit 1=ready
static FB_BITS: AtomicU64 = AtomicU64::new(0); // phys of bitmap (via HHDM)
static FB_WORDS: AtomicU64 = AtomicU64::new(0);
static FB_RAM_MIN: AtomicU64 = AtomicU64::new(0);

/// Allocate and arm the frame bitmap. Call once, after heap::init.
pub fn init_frame_bitmap() {
    if FB_READY.load(Ordering::SeqCst) != 0 {
        return;
    }
    let entries = limine_boot::MEMMAP
        .response()
        .expect("Limine memmap")
        .entries();
    let mut min = u64::MAX;
    let mut max = 0u64;
    for e in entries {
        if e.type_ != memmap::MEMMAP_USABLE {
            continue;
        }
        if e.base < min {
            min = e.base;
        }
        if e.base.saturating_add(e.length) > max {
            max = e.base.saturating_add(e.length);
        }
    }
    if min == u64::MAX {
        return;
    }
    let pages = ((max - min) / PAGE).div_ceil(64) as usize / (PAGE as usize / 8) + 1;
    let Some(bit_phys) = alloc_contiguous_frames(pages) else {
        return;
    };
    FB_BITS.store(bit_phys, Ordering::SeqCst);
    FB_WORDS.store((((max - min) / PAGE).div_ceil(64)) as u64, Ordering::SeqCst);
    FB_RAM_MIN.store(min, Ordering::SeqCst);
    // Mark the bitmap's own frames allocated (alloc_contiguous skipped marking
    // because the bitmap was not armed yet).
    for i in 0..pages as u64 {
        fb_mark(bit_phys + i * PAGE);
    }
    FB_READY.store(1, Ordering::SeqCst);
}

#[inline]
fn fb_word(phys: u64) -> Option<(usize, u64)> {
    if FB_READY.load(Ordering::SeqCst) == 0 {
        return None;
    }
    let min = FB_RAM_MIN.load(Ordering::SeqCst);
    let words = FB_WORDS.load(Ordering::SeqCst) as usize;
    if phys < min || phys & 0xfff != 0 {
        return None;
    }
    let idx = (phys - min) / PAGE;
    let w = (idx / 64) as usize;
    if w >= words {
        return None;
    }
    let bit = 1u64 << (idx % 64);
    Some((w, bit))
}

/// Mark a frame allocated; panic if it already was.
fn fb_mark(phys: u64) {
    let Some((w, bit)) = fb_word(phys) else {
        return;
    };
    let base = (FB_BITS.load(Ordering::SeqCst) + limine_boot::hhdm_offset()) as *mut u64;
    let cell = unsafe { &*(base.wrapping_add(w) as *const AtomicU64) };
    loop {
        let cur = cell.load(Ordering::SeqCst);
        if cur & bit != 0 {
            panic!("mm: double alloc of frame {:#x}", phys);
        }
        if cell
            .compare_exchange(cur, cur | bit, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return;
        }
    }
}

/// Mark a frame free; panic if it was already free (double free).
fn fb_unmark(phys: u64) {
    let Some((w, bit)) = fb_word(phys) else {
        return;
    };
    let base = (FB_BITS.load(Ordering::SeqCst) + limine_boot::hhdm_offset()) as *mut u64;
    let cell = unsafe { &*(base.wrapping_add(w) as *const AtomicU64) };
    loop {
        let cur = cell.load(Ordering::SeqCst);
        if cur & bit == 0 {
            panic!("mm: double free of frame {:#x}", phys);
        }
        if cell
            .compare_exchange(cur, cur & !bit, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return;
        }
    }
}

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

/// Return a previously freed frame to the allocator (page need not be zeroed).
pub fn free_frame(phys: u64) {
    if phys == 0 || phys & 0xfff != 0 {
        return;
    }
    if overlaps_kernel(phys) {
        return;
    }
    // Double-free detection BEFORE touching the frame: a second free would
    // overwrite the live freelist node's next-ptr (self-cycle → same frame
    // handed out twice → two owners, random data corruption).
    fb_unmark(phys);
    let hhdm = limine_boot::hhdm_offset();
    loop {
        let head = FREE_HEAD.load(Ordering::SeqCst);
        unsafe {
            core::ptr::write_unaligned((phys + hhdm) as *mut u64, head);
        }
        // Sentinel-fill the remainder so stray writes into a freed frame
        // are detectable (a popped frame's next-ptr landing misaligned
        // means something scribbled here while it sat on the freelist).
        unsafe {
            core::ptr::write_bytes((phys + hhdm + 8) as *mut u8, 0x5A, (PAGE - 8) as usize);
        }
        if FREE_HEAD
            .compare_exchange(head, phys, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return;
        }
    }
}

/// Allocate a 4 KiB frame, zero it, return its physical address.

/// Allocate `n` physically contiguous zeroed frames from the bump cursor only
/// (skip the freelist). Needed for ELF scratch: HHDM byte slices require
/// contiguous phys, but freelist pages are typically scattered — mixing them
/// into a contiguous grab made `elf_scratch_mut` return `None` after
/// `expand_user_elf` had already rewritten the aspace (ISO login → rip=0).
///
/// Returns the physical address of the first frame, or `None` if no usable
/// contiguous run of `n` pages remains.
pub fn alloc_contiguous_frames(n: usize) -> Option<u64> {
    if n == 0 {
        return None;
    }
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
    let need = (n as u64).saturating_mul(PAGE);

    for e in entries {
        if e.type_ != memmap::MEMMAP_USABLE {
            continue;
        }
        let region_end = e.base + e.length;
        let mut phys = e.base.max(next);
        phys = (phys + 0xfff) & !0xfff;
        while phys.saturating_add(need) <= region_end {
            // Skip runs that overlap the kernel image.
            let mut ok = true;
            let mut p = phys;
            for _ in 0..n {
                if overlaps_kernel(p) {
                    ok = false;
                    let (_, k1) = kernel_phys_range();
                    phys = (k1 + 0xfff) & !0xfff;
                    break;
                }
                p = p.wrapping_add(PAGE);
            }
            if !ok {
                continue;
            }
            NEXT.store(phys + need, Ordering::SeqCst);
            unsafe {
                core::ptr::write_bytes((phys + hhdm) as *mut u8, 0, need as usize);
            }
            if FB_READY.load(Ordering::SeqCst) != 0 {
                for i in 0..n as u64 {
                    fb_mark(phys + i * PAGE);
                }
            }
            return Some(phys);
        }
    }
    None
}

/// True if `phys` lies in a usable memmap region (for freelist validation).
fn frame_in_usable_memmap(phys: u64) -> bool {
    let entries = limine_boot::MEMMAP
        .response()
        .expect("Limine memmap")
        .entries();
    for e in entries {
        if e.type_ == memmap::MEMMAP_USABLE
            && phys >= e.base
            && phys.saturating_add(PAGE) <= e.base.saturating_add(e.length)
        {
            return true;
        }
    }
    false
}

/// Freelist-node sanity: page-aligned, usable memmap, not kernel image.
/// A bad head means something wrote into a frame while it sat on the
/// freelist (stale mapping / DMA into freed memory) — panic with the value
/// so the culprit's write pattern is diagnosable instead of spreading as
/// random data corruption (git objects / TLS transcripts / sepc=0).
fn validate_free_frame(phys: u64) {
    if phys & 0xfff != 0 || overlaps_kernel(phys) || !frame_in_usable_memmap(phys) {
        panic!(
            "mm: freelist head corrupt: phys={:#x} aligned={} kernel={} usable={}",
            phys,
            phys & 0xfff == 0,
            overlaps_kernel(phys),
            frame_in_usable_memmap(phys),
        );
    }
}

pub fn alloc_frame() -> u64 {
    let hhdm = limine_boot::hhdm_offset();

    // Prefer reclaimed user frames (process exit / abandoned exec).
    loop {
        let head = FREE_HEAD.load(Ordering::SeqCst);
        if head == 0 {
            break;
        }
        validate_free_frame(head);
        let next = unsafe { core::ptr::read_unaligned((head + hhdm) as *const u64) };
        if next != 0
            && (next & 0xfff != 0 || overlaps_kernel(next) || !frame_in_usable_memmap(next))
        {
            // head's own next-ptr was corrupted while it sat on the freelist.
            panic!(
                "mm: freelist node corrupt: head={:#x} next={:#x} aligned={} kernel={} usable={}",
                head,
                next,
                next & 0xfff == 0,
                overlaps_kernel(next),
                frame_in_usable_memmap(next),
            );
        }
        if FREE_HEAD
            .compare_exchange(head, next, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            unsafe {
                core::ptr::write_bytes((head + hhdm) as *mut u8, 0, PAGE as usize);
            }
            fb_mark(head);
            return head;
        }
    }

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
            fb_mark(phys);
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
