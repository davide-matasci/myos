//! Kernel heap: `linked_list_allocator`, backed by Limine HHDM.
//!
//! Besides runtime modules, VFS registration logging and kernel stacks, it
//! holds every tmpfs file's data (the os-test copy is ~30 MB; a Linux
//! package root such as Alpine's python3 ~45 MB). So it is sized from the
//! machine's memory at boot ([`size_for`]) instead of a fixed size.
//!
//! Blocks of up to [`SMALL_MAX`] bytes are kept, once freed, on a free list
//! per size class and handed out again from there. `linked_list_allocator`
//! walks its list of holes on every allocation and free, and the small
//! blocks (paths, map nodes, boxes) left most of its holes: installing the
//! 3,500 files of os-test spent most of its kernel time in that walk.

use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use linked_list_allocator::Heap;
use spin::Mutex;

use crate::limine_boot;

/// Bounds of the heap size: what the kernel needs at the least, and a cap
/// that leaves most of a large machine to user memory.
const MIN_SIZE: u64 = 64 * 1024 * 1024;
const MAX_SIZE: u64 = 1024 * 1024 * 1024;

#[global_allocator]
static ALLOCATOR: IrqSafeHeap = IrqSafeHeap(Mutex::new(Classed::new()));

/// The largest block a size class holds; the classes are the powers of two
/// from 16 bytes (`linked_list_allocator`'s smallest block) up to it.
const SMALL_MAX: usize = 512;
const CLASSES: usize = (SMALL_MAX / 16).trailing_zeros() as usize + 1;

/// The heap and the free lists of its small blocks.
struct Classed {
    heap: Heap,
    /// Each class's free blocks, linked through their first word.
    free: [*mut u8; CLASSES],
    /// The bytes in the free lists: free, for [`usage`].
    cached: usize,
}

// The free lists' pointers are the heap's own memory.
unsafe impl Send for Classed {}

impl Classed {
    const fn new() -> Self {
        Classed { heap: Heap::empty(), free: [core::ptr::null_mut(); CLASSES], cached: 0 }
    }

    /// The class of a block of `layout`, or `None` for a large one (or one
    /// aligned beyond the 16 bytes every class's blocks have).
    fn class(layout: Layout) -> Option<usize> {
        let size = layout.size().max(16);
        (size <= SMALL_MAX && layout.align() <= 16).then(|| (size.next_power_of_two() / 16).trailing_zeros() as usize)
    }

    fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let Some(c) = Self::class(layout) else {
            return self.heap.allocate_first_fit(layout).map_or(core::ptr::null_mut(), |p| p.as_ptr());
        };
        let block = self.free[c];
        if block.is_null() {
            let class = Layout::from_size_align(16 << c, 16).unwrap();
            return self.heap.allocate_first_fit(class).map_or(core::ptr::null_mut(), |p| p.as_ptr());
        }
        self.free[c] = unsafe { *(block as *mut *mut u8) };
        self.cached -= 16 << c;
        block
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        let Some(c) = Self::class(layout) else {
            return unsafe { self.heap.deallocate(core::ptr::NonNull::new_unchecked(ptr), layout) };
        };
        unsafe { *(ptr as *mut *mut u8) = self.free[c] };
        self.free[c] = ptr;
        self.cached += 16 << c;
    }
}

/// The heap with interrupts off while its lock is held. Otherwise a task
/// preempted inside `alloc` keeps the lock, and the next one on its CPU to
/// allocate with interrupts off (under `TASKS`, in `with_process_mut`)
/// spins on it forever.
struct IrqSafeHeap(Mutex<Classed>);

unsafe impl GlobalAlloc for IrqSafeHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !READY.load(Ordering::Relaxed) {
            before_init(layout);
        }
        let flags = crate::arch::irq_save();
        crate::arch::irq_off();
        let p = self.0.lock().alloc(layout);
        crate::arch::irq_restore(flags);
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let flags = crate::arch::irq_save();
        crate::arch::irq_off();
        unsafe { self.0.lock().dealloc(ptr, layout) };
        crate::arch::irq_restore(flags);
    }
}

/// Set once [`init`] gave the heap its memory.
static READY: AtomicBool = AtomicBool::new(false);

/// An allocation before [`init`] (a `format!` in what `kernel_main` runs
/// first): a line on the console, then the CPU stops. The empty heap would
/// refuse it, and the panic for the refusal formats its message on the heap
/// as well, so the boot went silent with the CPU at address 0.
#[cold]
fn before_init(layout: Layout) -> ! {
    crate::console::write_fmt(format_args!("fatal: a {}-byte allocation before heap::init\n", layout.size()));
    crate::arch::halt()
}

/// Physical end of the heap (the frame allocator starts after it).
static PHYS_END: AtomicU64 = AtomicU64::new(0);

/// A quarter of the usable memory, within [`MIN_SIZE`, `MAX_SIZE`], in
/// whole 2 MiB: 1 GiB with the 4 GiB the CI guests get. The heap is one
/// piece of one region, so it takes at most half the largest: a firmware
/// that splits the memory (riscv64's EDK2 gave a 4 GiB guest a largest
/// region under 2 GiB) does not halve the heap with it, as a quarter of the
/// largest region did (494 MiB, which the full boot's packages in /tmp
/// filled).
fn size_for(usable: u64, largest_region: u64) -> u64 {
    (usable / 4).clamp(MIN_SIZE, MAX_SIZE).min((largest_region / 2).max(MIN_SIZE)) & !(2 * 1024 * 1024 - 1)
}

/// The heap's size, as [`init`] chose it.
pub fn size() -> usize {
    usage().0
}

pub fn init() {
    let size = size_for(limine_boot::usable_total(), limine_boot::largest_usable());
    let start = limine_boot::alloc_usable(size as usize);
    PHYS_END.store(start as u64 - limine_boot::hhdm_offset() + size, Ordering::SeqCst);
    unsafe {
        ALLOCATOR.0.lock().heap.init(start as *mut u8, size as usize);
    }
    READY.store(true, Ordering::SeqCst);
}

/// Physical address just past the heap.
pub fn phys_end() -> u64 {
    PHYS_END.load(Ordering::SeqCst)
}

/// The heap's size and the bytes free in it: tmpfs's space (`statvfs`).
pub fn usage() -> (usize, usize) {
    let flags = crate::arch::irq_save();
    crate::arch::irq_off();
    let (size, free) = {
        let classed = ALLOCATOR.0.lock();
        (classed.heap.size(), classed.heap.free() + classed.cached)
    };
    crate::arch::irq_restore(flags);
    (size, free)
}

/// `len` zero bytes, or `None` when the heap has no room for them in one
/// piece: for buffers sized by data (a whole file read for exec), where a
/// full or fragmented heap must fail the call, not panic the kernel. The
/// refusal is reported with the heap's state.
pub fn try_zeroed(len: usize) -> Option<Vec<u8>> {
    let mut v = Vec::new();
    if v.try_reserve_exact(len).is_err() {
        let (size, free) = usage();
        crate::console::status_warn(&alloc::format!("heap: no room for {len} bytes in one piece ({free} of {size} free)"));
        return None;
    }
    v.resize(len, 0);
    Some(v)
}
