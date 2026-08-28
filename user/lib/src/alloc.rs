//! Bump allocator on syscall `brk` (nr 9).

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

const PAGE: usize = 4096;

static BRK_END: AtomicUsize = AtomicUsize::new(0);
static BRK_PTR: AtomicUsize = AtomicUsize::new(0);

/// Query the program break and seed the bump allocator.
pub fn heap_init() {
    let end = crate::brk(0);
    BRK_END.store(end, Ordering::SeqCst);
    BRK_PTR.store(end, Ordering::SeqCst);
}

pub struct Heap;

unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let align = layout.align().max(1);
        let size = layout.size().max(1);
        loop {
            let ptr = BRK_PTR.load(Ordering::SeqCst);
            let aligned = (ptr + align - 1) & !(align - 1);
            let Some(new_end) = aligned.checked_add(size) else {
                return core::ptr::null_mut();
            };
            let cur_end = BRK_END.load(Ordering::SeqCst);
            if new_end <= cur_end {
                BRK_PTR.store(new_end, Ordering::SeqCst);
                return aligned as *mut u8;
            }
            let new_brk = (new_end + PAGE - 1) & !(PAGE - 1);
            let got = crate::brk(new_brk);
            if got < new_brk {
                return core::ptr::null_mut();
            }
            BRK_END.store(got, Ordering::SeqCst);
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}
