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
//!
//! The bump cursor and the freelist change under [`FRAMES`] only. Both were
//! lock-free and racy on SMP: two CPUs bumping at once got the same frame,
//! and a freelist pop could read the next-ptr of a head another CPU had just
//! popped and handed to a user (the "freelist node corrupt" panic with a
//! value like `0x5b01`), or, with that head pushed back meanwhile, put a
//! frame in use back on the list (ABA).

use core::sync::atomic::{AtomicU64, Ordering};

use limine::memmap;
use spin::Mutex;

use crate::limine_boot;

const PAGE: u64 = 4096;

static NEXT: AtomicU64 = AtomicU64::new(0);
/// Intrusive freelist head (physical address), or 0. Each free page stores the
/// next phys at offset 0 via HHDM.
static FREE_HEAD: AtomicU64 = AtomicU64::new(0);
/// Held (interrupts off) while [`NEXT`] or [`FREE_HEAD`] change.
static FRAMES: Mutex<()> = Mutex::new(());

/// Run `f` with [`FRAMES`] held and interrupts off on this CPU.
fn with_frames<R>(f: impl FnOnce() -> R) -> R {
    let flags = crate::arch::irq_save();
    crate::arch::irq_off();
    let r = {
        let _held = FRAMES.lock();
        f()
    };
    crate::arch::irq_restore(flags);
    r
}

/// Diagnostics for the frame allocator: total 4 KiB frames handed out vs
/// returned to the freelist. Printed verbatim in the `out of usable memory`
/// panic so an exhaustion can be attributed to a leak vs a small memmap.
pub static FRAME_ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
pub static FRAME_FREE_COUNT: AtomicU64 = AtomicU64::new(0);

/// Per-call-site allocation attribution (leak triage). Sites:
/// 0=virtq 1=fault-zero 2=exec-copy 3=pagetable 4=mmap 5=other-explicit
pub static FRAME_SITE_COUNTS: [AtomicU64; 6] = [
    AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
    AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
];

/// Zero one 4 KiB frame through its HHDM mapping.
///
/// The kernel is built unoptimized and `write_bytes` lowers to a byte loop
/// there (very slow under TCG), so use word stores: `rep stosq` on x86_64.
///
/// # Safety
/// `page` must point at a writable, 8-byte-aligned 4 KiB page.
#[inline(always)]
pub unsafe fn zero_page(page: *mut u8) {
    unsafe { crate::arch::zero_page(page) }
}

/// Allocate one frame and attribute it to a leak-triage call site.
#[inline(always)]
pub fn alloc_frame_site(site: usize) -> u64 {
    let f = alloc_frame();
    if site < FRAME_SITE_COUNTS.len() {
        FRAME_SITE_COUNTS[site].fetch_add(1, Ordering::Relaxed);
    }
    f
}

/// `/proc/meminfo`: frame allocator counters (4 KiB frames). `FramesLive` is
/// allocated minus freed; the `Site*` lines are cumulative allocations per
/// call site, the same numbers the out-of-memory panic prints.
pub fn meminfo_text() -> alloc::vec::Vec<u8> {
    let a = FRAME_ALLOC_COUNT.load(Ordering::Relaxed);
    let f = FRAME_FREE_COUNT.load(Ordering::Relaxed);
    let site = |i: usize| FRAME_SITE_COUNTS[i].load(Ordering::Relaxed);
    alloc::format!(
        "FramesAlloc: {}\nFramesFree: {}\nFramesLive: {}\nLiveKiB: {}\n\
         SiteVirtq: {}\nSiteFault0: {}\nSiteExec: {}\nSitePageTable: {}\nSiteMmap: {}\nSiteOther: {}\n\
         BlockCacheKiB: {}\n",
        a,
        f,
        a - f,
        (a - f) * (PAGE / 1024),
        site(0),
        site(1),
        site(2),
        site(3),
        site(4),
        site(5),
        crate::blk::cache_frames() as u64 * (PAGE / 1024),
    )
    .into_bytes()
}

/// All usable RAM, in 4 KiB frames.
pub fn usable_frames() -> u64 {
    limine_boot::MEMMAP.response().map_or(0, |r| {
        r.entries().iter().filter(|e| e.type_ == memmap::MEMMAP_USABLE).map(|e| e.length / PAGE).sum()
    })
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
    let hhdm = limine_boot::hhdm_offset();
    with_frames(|| {
        let head = FREE_HEAD.load(Ordering::SeqCst);
        // Only the next-ptr is written: `alloc_frame` validates it (a stray
        // write into a free frame still surfaces as a bad next-ptr) and zeroes
        // the page. A 4 KiB 0x5A fill here cost ~600k cycles per frame in the
        // debug kernel under TCG (byte-wise memset) — fine while exits leaked,
        // ~0.5 s per process once exits actually reclaim their pages.
        unsafe {
            core::ptr::write_unaligned((phys + hhdm) as *mut u64, head);
        }
        FREE_HEAD.store(phys, Ordering::SeqCst);
    });
    FRAME_FREE_COUNT.fetch_add(1, Ordering::Relaxed);
}

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
    let need = (n as u64).saturating_mul(PAGE);
    let phys = with_frames(|| bump_run(n, need))?;
    unsafe {
        core::ptr::write_bytes((phys + hhdm) as *mut u8, 0, need as usize);
    }
    Some(phys)
}

/// The bump cursor's next run of `n` frames (`need` bytes) clear of the
/// kernel image, cursor moved past it. Caller holds [`FRAMES`].
fn bump_run(n: usize, need: u64) -> Option<u64> {
    let entries = limine_boot::MEMMAP
        .response()
        .expect("Limine memmap")
        .entries();

    let mut next = NEXT.load(Ordering::SeqCst);
    if next == 0 {
        next = crate::heap::phys_end();
    }
    next = (next + 0xfff) & !0xfff;

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

/// Allocate a 4 KiB frame, zero it, return its physical address. When
/// memory runs out the block cache gives its frames back first.
pub fn alloc_frame() -> u64 {
    // Prefer reclaimed user frames (process exit / abandoned exec).
    let take = || with_frames(|| pop_free().or_else(|| bump_run(1, PAGE)));
    let Some(phys) = take().or_else(|| (crate::blk::release_cache() > 0).then(take).flatten()) else {
        out_of_memory();
    };
    unsafe {
        zero_page(hhdm(phys));
    }
    FRAME_ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
    phys
}

/// Take the freelist's head, or `None` when it is empty. Caller holds
/// [`FRAMES`].
fn pop_free() -> Option<u64> {
    let head = FREE_HEAD.load(Ordering::SeqCst);
    if head == 0 {
        return None;
    }
    validate_free_frame(head);
    let next = unsafe { core::ptr::read_unaligned(hhdm(head) as *const u64) };
    if next != 0 && (next & 0xfff != 0 || overlaps_kernel(next) || !frame_in_usable_memmap(next)) {
        // head's own next-ptr was overwritten while it sat on the freelist.
        panic!(
            "mm: freelist node corrupt: head={:#x} next={:#x} aligned={} kernel={} usable={}",
            head,
            next,
            next & 0xfff == 0,
            overlaps_kernel(next),
            frame_in_usable_memmap(next),
        );
    }
    FREE_HEAD.store(next, Ordering::SeqCst);
    Some(head)
}

fn out_of_memory() -> ! {
    let top = limine_boot::MEMMAP
        .response()
        .map(|r| {
            r.entries()
                .iter()
                .filter(|e| e.type_ == memmap::MEMMAP_USABLE)
                .map(|e| e.base + e.length)
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    panic!(
        "out of usable memory: alloc={} free={} live={} next={:#x} usable_top={:#x} sites virtq={} fault0={} exec={} pt={} mmap={} other={}",
        FRAME_ALLOC_COUNT.load(Ordering::Relaxed),
        FRAME_FREE_COUNT.load(Ordering::Relaxed),
        FRAME_ALLOC_COUNT.load(Ordering::Relaxed) - FRAME_FREE_COUNT.load(Ordering::Relaxed),
        NEXT.load(Ordering::SeqCst),
        top,
        FRAME_SITE_COUNTS[0].load(Ordering::Relaxed),
        FRAME_SITE_COUNTS[1].load(Ordering::Relaxed),
        FRAME_SITE_COUNTS[2].load(Ordering::Relaxed),
        FRAME_SITE_COUNTS[3].load(Ordering::Relaxed),
        FRAME_SITE_COUNTS[4].load(Ordering::Relaxed),
        FRAME_SITE_COUNTS[5].load(Ordering::Relaxed),
    );
}

pub fn hhdm(phys: u64) -> *mut u8 {
    (phys + limine_boot::hhdm_offset()) as *mut u8
}

pub fn table(phys: u64) -> *mut [u64; 512] {
    hhdm(phys & !0xfff) as *mut [u64; 512]
}

/// Allocate `n` consecutive 4 KiB frames for DMA rings: `(phys, hhdm va)`.
/// Fails if the bump allocator crossed a memmap hole (should not happen for
/// a handful of pages).
pub fn alloc_contiguous_pages(n: usize) -> Option<(u64, *mut u8)> {
    if n == 0 {
        return None;
    }
    let first = alloc_frame_site(0);
    for i in 1..n {
        let p = alloc_frame_site(0);
        if p != first + (i as u64) * 4096 {
            return None;
        }
    }
    Some((first, hhdm(first)))
}
