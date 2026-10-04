//! Kernel heap: `linked_list_allocator`, backed by Limine HHDM.
//!
//! Besides runtime modules, VFS registration logging and kernel stacks, it
//! holds every tmpfs file's data (the os-test copy is ~30 MB; a Linux
//! package root such as Alpine's python3 ~45 MB). So it is sized from the
//! machine's memory at boot ([`size_for`]) instead of a fixed size.

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicU64, Ordering};

use linked_list_allocator::LockedHeap;

use crate::limine_boot;

/// Bounds of the heap size: what the kernel needs at the least, and a cap
/// that leaves most of a large machine to user memory.
const MIN_SIZE: u64 = 64 * 1024 * 1024;
const MAX_SIZE: u64 = 1024 * 1024 * 1024;

#[global_allocator]
static ALLOCATOR: IrqSafeHeap = IrqSafeHeap(LockedHeap::empty());

/// The heap with interrupts off while its lock is held. Otherwise a task
/// preempted inside `alloc` keeps the lock, and the next one on its CPU to
/// allocate with interrupts off (under `TASKS`, in `with_process_mut`)
/// spins on it forever.
struct IrqSafeHeap(LockedHeap);

unsafe impl GlobalAlloc for IrqSafeHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let flags = crate::arch::irq_save();
        crate::arch::irq_off();
        let p = unsafe { self.0.alloc(layout) };
        crate::arch::irq_restore(flags);
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let flags = crate::arch::irq_save();
        crate::arch::irq_off();
        unsafe { self.0.dealloc(ptr, layout) };
        crate::arch::irq_restore(flags);
    }
}

/// Physical end of the heap (the frame allocator starts after it).
static PHYS_END: AtomicU64 = AtomicU64::new(0);

/// A quarter of the largest usable memory region, within
/// [`MIN_SIZE`, `MAX_SIZE`], in whole 2 MiB: 256 MiB with the 1 GiB the CI
/// guests get.
fn size_for(largest_region: u64) -> u64 {
    (largest_region / 4).clamp(MIN_SIZE, MAX_SIZE) & !(2 * 1024 * 1024 - 1)
}

pub fn init() {
    let size = size_for(limine_boot::largest_usable());
    let start = limine_boot::alloc_usable(size as usize);
    PHYS_END.store(start as u64 - limine_boot::hhdm_offset() + size, Ordering::SeqCst);
    unsafe {
        ALLOCATOR.0.lock().init(start as *mut u8, size as usize);
    }
}

/// Physical address just past the heap.
pub fn phys_end() -> u64 {
    PHYS_END.load(Ordering::SeqCst)
}
