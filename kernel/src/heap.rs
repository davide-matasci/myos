//! Kernel heap. 128 KiB, `linked_list_allocator`.

use linked_list_allocator::LockedHeap;

pub const HEAP_SIZE: usize = 128 * 1024;

/// x86_64: a virtual window we map with the frame allocator (not identity).
#[cfg(target_arch = "x86_64")]
pub const HEAP_START: usize = 0x_4444_4444_0000;

#[cfg(target_arch = "aarch64")]
#[repr(align(4096))]
struct HeapSpace(#[allow(dead_code)] [u8; HEAP_SIZE]);

#[cfg(target_arch = "aarch64")]
static mut HEAP_SPACE: HeapSpace = HeapSpace([0; HEAP_SIZE]);

#[cfg(target_arch = "aarch64")]
fn heap_start() -> usize {
    core::ptr::addr_of_mut!(HEAP_SPACE) as usize
}

#[cfg(target_arch = "x86_64")]
fn heap_start() -> usize {
    HEAP_START
}

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init() {
    let start = heap_start();
    crate::arch::map_writable(start, HEAP_SIZE);
    unsafe {
        ALLOCATOR.lock().init(start as *mut u8, HEAP_SIZE);
    }
}
