//! The heap on the mlibc flavour (`sys::myos::libc`): the C library's, so
//! that the program's Rust and its C share one (`Box::from_raw` of what C
//! allocated, `free` of what Rust did, at alignments `malloc` gives).

use crate::alloc::Layout;
use crate::ptr;
use crate::sys::myos::libc;

/// What `malloc` aligns to on the three arches.
const MIN_ALIGN: usize = 16;

#[inline]
unsafe fn aligned(layout: Layout) -> *mut u8 {
    let mut out = ptr::null_mut();
    // posix_memalign wants a multiple of the pointer's size.
    let align = layout.align().max(crate::mem::size_of::<usize>());
    if unsafe { libc::posix_memalign(&mut out, align, layout.size()) } != 0 {
        return ptr::null_mut();
    }
    out.cast()
}

#[inline]
pub unsafe fn alloc(layout: Layout) -> *mut u8 {
    if layout.align() <= MIN_ALIGN && layout.align() <= layout.size() {
        unsafe { libc::malloc(layout.size()).cast() }
    } else {
        unsafe { aligned(layout) }
    }
}

#[inline]
pub unsafe fn alloc_zeroed(layout: Layout) -> *mut u8 {
    if layout.align() <= MIN_ALIGN && layout.align() <= layout.size() {
        unsafe { libc::calloc(layout.size(), 1).cast() }
    } else {
        let p = unsafe { aligned(layout) };
        if !p.is_null() {
            unsafe { ptr::write_bytes(p, 0, layout.size()) };
        }
        p
    }
}

#[inline]
pub unsafe fn dealloc(ptr: *mut u8, _layout: Layout) {
    unsafe { libc::free(ptr.cast()) }
}

#[inline]
pub unsafe fn realloc(ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    if layout.align() <= MIN_ALIGN && layout.align() <= new_size {
        unsafe { libc::realloc(ptr.cast(), new_size).cast() }
    } else {
        // An over-aligned block: a new one, the bytes moved.
        let new = unsafe { aligned(Layout::from_size_align_unchecked(new_size, layout.align())) };
        if !new.is_null() {
            unsafe {
                ptr::copy_nonoverlapping(ptr, new, layout.size().min(new_size));
                libc::free(ptr.cast());
            }
        }
        new
    }
}
