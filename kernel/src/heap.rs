//! Kernel heap. 64 MiB, `linked_list_allocator`, backed by Limine HHDM.
//!
//! Sized for runtime modules, VFS registration logging, and kernel stacks
//! registration logging when many `/c/` names are embedded at boot.

use linked_list_allocator::LockedHeap;

use crate::limine_boot;

// Sized for the os-test copy to tmpfs (~30 MB: 6.5k files' data Vecs live
// on the kernel heap) plus runtime modules, VFS registration logging and
// kernel stacks. CI boots QEMU with 1 GiB guest RAM.
pub const HEAP_SIZE: usize = 64 * 1024 * 1024;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init() {
    let start = limine_boot::alloc_usable(HEAP_SIZE);
    unsafe {
        ALLOCATOR.lock().init(start as *mut u8, HEAP_SIZE);
    }
}
