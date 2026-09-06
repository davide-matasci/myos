use crate::alloc::Layout;

const PAGE: usize = 4096;
/// Soft cap for std's eager brk claim. Kernel `HEAP_PAGES` is the hard limit
/// (aarch64 768 / else 1024). Keep std's claim modest — std programs do not
/// host the mbedtls arena.
#[cfg(target_arch = "aarch64")]
const HEAP_PAGES: usize = 180;
#[cfg(not(target_arch = "aarch64"))]
const HEAP_PAGES: usize = 256;

static mut BRK_END: usize = 0;
static mut BRK_PTR: usize = 0;
static mut BRK_INIT: bool = false;

#[inline]
fn brk(addr: usize) -> usize {
    crate::sys::myos::abi::brk(addr)
}

#[cold]
fn init_heap() {
    unsafe {
        if BRK_INIT {
            return;
        }
        BRK_INIT = true;
        let base = brk(0);
        BRK_PTR = base;
        if let Some(mapped_end) = base.checked_add(HEAP_PAGES * PAGE) {
            let got = brk(mapped_end);
            BRK_END = if got >= mapped_end { got } else { base };
        } else {
            BRK_END = base;
        }
    }
}

#[inline]
pub unsafe fn alloc(layout: Layout) -> *mut u8 {
    init_heap();
    let align = layout.align().max(1);
    let size = layout.size().max(1);
    loop {
        let ptr = unsafe { BRK_PTR };
        let aligned = (ptr + align - 1) & !(align - 1);
        let Some(new_end) = aligned.checked_add(size) else {
            return core::ptr::null_mut();
        };
        let cur_end = unsafe { BRK_END };
        if new_end <= cur_end {
            unsafe { BRK_PTR = new_end };
            return aligned as *mut u8;
        }
        let new_brk = (new_end + PAGE - 1) & !(PAGE - 1);
        let got = brk(new_brk);
        if got < new_brk {
            return core::ptr::null_mut();
        }
        unsafe { BRK_END = got };
    }
}

#[inline]
pub unsafe fn dealloc(_ptr: *mut u8, _layout: Layout) {}

#[inline]
pub unsafe fn realloc(ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
    let new_ptr = unsafe { alloc(new_layout) };
    if !new_ptr.is_null() {
        let copy = layout.size().min(new_size);
        unsafe {
            core::ptr::copy_nonoverlapping(ptr, new_ptr, copy);
            dealloc(ptr, layout);
        }
    }
    new_ptr
}
