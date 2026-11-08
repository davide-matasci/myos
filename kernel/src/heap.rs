//! Kernel heap: `linked_list_allocator`, backed by Limine HHDM.
//!
//! Besides runtime modules, VFS registration logging and kernel stacks, it
//! holds every tmpfs file's data (the os-test copy is ~30 MB; a Linux
//! package root such as Alpine's python3 ~45 MB). So it is sized from the
//! machine's memory at boot ([`size_for`]) instead of a fixed size.
//!
//! Blocks of up to [`CLASS_MAX`] bytes are kept, once freed, on a free list
//! per size class and handed out again from there. `linked_list_allocator`
//! walks its list of holes on every allocation and free, and that walk
//! grows with the heap's use: the small blocks (paths, map nodes, boxes)
//! left most of its holes (installing the 3,500 files of os-test spent most
//! of its kernel time in it), and after an hour of builds every block above
//! 512 bytes (a Linux `pread`'s buffer, an exec's copies) cost 0.1 to 0.4 ms
//! each way, with the heap locked and interrupts off: every CPU queued on
//! it, and a build crawled. A class above [`SMALL_MAX`] keeps at most
//! [`CLASS_CACHE_MAX`] bytes, the rest going back to the heap, and an
//! allocation the heap has no room for gets the free lists back first
//! ([`Classed::drain`]).

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
const CLASS_MAX: usize = 1024 * 1024;
const CLASSES: usize = (CLASS_MAX / 16).trailing_zeros() as usize + 1;
/// The classes up to this size keep every block freed (most of the heap's
/// blocks, few of its bytes); a larger one keeps [`CLASS_CACHE_MAX`] bytes.
const SMALL_MAX: usize = 512;
const CLASS_CACHE_MAX: usize = 4 * 1024 * 1024;
/// The blocks of the classes from a page up are aligned to a page, so a
/// page-aligned allocation (a kernel stack, a table) takes one too; the
/// smaller ones to 16 bytes, as every allocation is.
const PAGE: usize = 4096;

/// The heap and the free lists of its size classes.
struct Classed {
    heap: Heap,
    /// Each class's free blocks, linked through their first word.
    free: [*mut u8; CLASSES],
    /// The bytes in each class's free list.
    class_cached: [usize; CLASSES],
    /// The bytes in the free lists: free, for [`usage`].
    cached: usize,
}

// The free lists' pointers are the heap's own memory.
unsafe impl Send for Classed {}

impl Classed {
    const fn new() -> Self {
        Classed { heap: Heap::empty(), free: [core::ptr::null_mut(); CLASSES], class_cached: [0; CLASSES], cached: 0 }
    }

    /// The class of a block of `layout`, or `None` for a large one (or one
    /// aligned beyond what its class's blocks are).
    fn class(layout: Layout) -> Option<usize> {
        let size = layout.size().max(16).next_power_of_two();
        let c = (size / 16).trailing_zeros() as usize;
        (size <= CLASS_MAX && layout.align() <= Self::class_layout(c).align()).then_some(c)
    }

    /// The layout of class `c`'s blocks in the heap.
    fn class_layout(c: usize) -> Layout {
        let size = 16usize << c;
        Layout::from_size_align(size, if size >= PAGE { PAGE } else { 16 }).unwrap()
    }

    fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let Some(c) = Self::class(layout) else {
            return self.heap_alloc(layout);
        };
        let block = self.free[c];
        if block.is_null() {
            return self.heap_alloc(Self::class_layout(c));
        }
        self.free[c] = unsafe { *(block as *mut *mut u8) };
        self.class_cached[c] -= 16 << c;
        self.cached -= 16 << c;
        block
    }

    /// A block from the heap itself; when there is no room for it, the free
    /// lists go back to the heap first.
    fn heap_alloc(&mut self, layout: Layout) -> *mut u8 {
        if let Ok(p) = self.heap.allocate_first_fit(layout) {
            return p.as_ptr();
        }
        if self.cached == 0 {
            return core::ptr::null_mut();
        }
        self.drain();
        self.heap.allocate_first_fit(layout).map_or(core::ptr::null_mut(), |p| p.as_ptr())
    }

    /// Give every block of the free lists back to the heap.
    fn drain(&mut self) {
        for c in 0..CLASSES {
            let layout = Self::class_layout(c);
            let mut block = core::mem::replace(&mut self.free[c], core::ptr::null_mut());
            while !block.is_null() {
                let next = unsafe { *(block as *mut *mut u8) };
                unsafe { self.heap.deallocate(core::ptr::NonNull::new_unchecked(block), layout) };
                block = next;
            }
            self.class_cached[c] = 0;
        }
        self.cached = 0;
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        let Some(c) = Self::class(layout) else {
            return unsafe { self.heap.deallocate(core::ptr::NonNull::new_unchecked(ptr), layout) };
        };
        let size = 16usize << c;
        if size > SMALL_MAX && self.class_cached[c] + size > CLASS_CACHE_MAX {
            return unsafe { self.heap.deallocate(core::ptr::NonNull::new_unchecked(ptr), Self::class_layout(c)) };
        }
        unsafe { *(ptr as *mut *mut u8) = self.free[c] };
        self.free[c] = ptr;
        self.class_cached[c] += size;
        self.cached += size;
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

/// Whether the heap is too full for another task: less free than
/// [`task_reserve`]. A task's kernel stack, its tables and what it does
/// next are allocated by paths that cannot fail (a panic on a laptop whose
/// heap a build had filled, `docs/scheduler.md`); refused here, a fork or
/// a thread fails with `EAGAIN` and the system stays up. Reported once per
/// episode (the non-blocking warning: this runs with interrupts off).
pub fn low_for_tasks() -> bool {
    let (size, free) = usage();
    let reserve = task_reserve(size);
    if free >= reserve {
        if free >= 2 * reserve {
            LOW_REPORTED.store(false, Ordering::Relaxed);
        }
        return false;
    }
    if !LOW_REPORTED.swap(true, Ordering::Relaxed) {
        let mut line = crate::task::Line::new();
        let _ = core::fmt::Write::write_fmt(&mut line, format_args!("heap: {free} of {size} bytes free: no new tasks until {reserve} are"));
        if !crate::console::status_warn_now(line.as_str()) {
            LOW_REPORTED.store(false, Ordering::Relaxed);
        }
    }
    true
}

/// What [`low_for_tasks`] keeps free: an eighth of the heap, 32 MiB at
/// most (a 1 GiB heap keeps 32 MiB, the 64 MiB minimum 8 MiB).
fn task_reserve(size: usize) -> usize {
    (size / 8).min(32 << 20)
}

static LOW_REPORTED: AtomicBool = AtomicBool::new(false);

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
