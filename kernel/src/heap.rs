//! Kernel heap. 128 KiB, `linked_list_allocator`, backed by Limine HHDM.

use linked_list_allocator::LockedHeap;

use crate::limine_boot;

pub const HEAP_SIZE: usize = 128 * 1024;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init() {
    let start = limine_boot::alloc_usable(HEAP_SIZE);
    unsafe {
        ALLOCATOR.lock().init(start as *mut u8, HEAP_SIZE);
    }
}
