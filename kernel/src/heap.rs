//! Kernel heap. 1 MiB, `linked_list_allocator`, backed by Limine HHDM.
//!
//! Sized for runtime modules (fat's cached root alone is ~150 KiB) plus VFS
//! registration logging when many `/c/` names are embedded at boot.

use linked_list_allocator::LockedHeap;

use crate::limine_boot;

pub const HEAP_SIZE: usize = 1024 * 1024;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init() {
    let start = limine_boot::alloc_usable(HEAP_SIZE);
    unsafe {
        ALLOCATOR.lock().init(start as *mut u8, HEAP_SIZE);
    }
}
