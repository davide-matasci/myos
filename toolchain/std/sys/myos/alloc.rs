//! The heap: dlmalloc (as on Xous and wasm) on memory from the kernel's mmap
//! window, anonymous mappings paged in as they are touched. Freed memory is
//! reused, and a region dlmalloc no longer needs at all goes back with
//! `munmap`. One thread at a time: dlmalloc takes no lock of its own.

use crate::alloc::Layout;
use crate::sys::myos::abi;

const PAGE: usize = 4096;

/// dlmalloc's source of memory: whole anonymous mappings.
struct Mmap;

unsafe impl dlmalloc::Allocator for Mmap {
    fn alloc(&self, size: usize) -> (*mut u8, usize, u32) {
        let size = size.next_multiple_of(PAGE);
        match abi::mmap_anon(size) {
            Some(addr) => (crate::ptr::with_exposed_provenance_mut(addr), size, 0),
            None => (crate::ptr::null_mut(), 0, 0),
        }
    }

    fn remap(&self, _ptr: *mut u8, _oldsize: usize, _newsize: usize, _can_move: bool) -> *mut u8 {
        crate::ptr::null_mut()
    }

    fn free_part(&self, ptr: *mut u8, oldsize: usize, newsize: usize) -> bool {
        // The tail goes; the mapping stays where it was.
        let keep = newsize.next_multiple_of(PAGE);
        keep >= oldsize || abi::munmap(ptr.addr() + keep, oldsize - keep)
    }

    fn free(&self, ptr: *mut u8, size: usize) -> bool {
        abi::munmap(ptr.addr(), size)
    }

    fn can_release_part(&self, _flags: u32) -> bool {
        true
    }

    fn allocates_zeros(&self) -> bool {
        true
    }

    fn page_size(&self) -> usize {
        PAGE
    }
}

static LOCK: crate::sys::sync::Mutex = crate::sys::sync::Mutex::new();
static mut DLMALLOC: dlmalloc::Dlmalloc<Mmap> = dlmalloc::Dlmalloc::new_with_allocator(Mmap);

/// Run `f` on the allocator, alone.
fn locked<R>(f: impl FnOnce(&mut dlmalloc::Dlmalloc<Mmap>) -> R) -> R {
    LOCK.lock();
    // SAFETY: `LOCK` makes this the only reference.
    let r = f(unsafe { &mut *(&raw mut DLMALLOC) });
    unsafe { LOCK.unlock() };
    r
}

#[inline]
pub unsafe fn alloc(layout: Layout) -> *mut u8 {
    locked(|d| unsafe { d.malloc(layout.size(), layout.align()) })
}

#[inline]
pub unsafe fn alloc_zeroed(layout: Layout) -> *mut u8 {
    locked(|d| unsafe { d.calloc(layout.size(), layout.align()) })
}

#[inline]
pub unsafe fn dealloc(ptr: *mut u8, layout: Layout) {
    locked(|d| unsafe { d.free(ptr, layout.size(), layout.align()) })
}

#[inline]
pub unsafe fn realloc(ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    locked(|d| unsafe { d.realloc(ptr, layout.size(), layout.align(), new_size) })
}
