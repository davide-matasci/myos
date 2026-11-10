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
//! it, and a build crawled. The classes up to [`SMALL_MAX`] keep every
//! block (most of the heap's blocks, few of its bytes); the larger ones
//! keep [`Classed::class_max`] bytes each and [`Classed::large_max`] in
//! all, the rest going back to the heap, and an allocation the heap has no
//! room for gets their blocks back, largest class first, until it fits
//! ([`Classed::heap_alloc`]). A block's class is its allocation's size
//! rounded up: to a power of two up to a page, to a whole page above it,
//! so a large block (a tmpfs file's data, an exec's copy of a program)
//! wastes under a page, not up to half of itself.
//!
//! Each CPU keeps a few blocks of the classes up to a page for itself
//! ([`CpuCache`]) and takes and gives those without the heap's lock: with
//! every path a syscall resolves allocating its strings, the lock was what
//! three processes working on files at once waited for most (a fifth of
//! their time under TCG). A CPU's cache is its own, touched with interrupts
//! off; it is filled from the class's list and emptied into it a batch at
//! a time.

use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use linked_list_allocator::Heap;
use spin::Mutex;

use crate::limine_boot;

/// Bounds of the heap size: what the kernel needs at the least, and a cap
/// that leaves most of a large machine to user memory.
const MIN_SIZE: u64 = 64 * 1024 * 1024;
const MAX_SIZE: u64 = 1024 * 1024 * 1024;

#[global_allocator]
static ALLOCATOR: IrqSafeHeap = IrqSafeHeap(Mutex::new(Classed::new()));

/// The largest block a size class holds. The classes are the powers of two
/// from 16 bytes (`linked_list_allocator`'s smallest block) to a page, then
/// every multiple of a page up to it.
const CLASS_MAX: usize = 256 * 1024;
const PAGE: usize = crate::user::PAGE;
/// The classes up to a page: 16, 32, ..., [`PAGE`] bytes.
const POW2_CLASSES: usize = (PAGE / 16).trailing_zeros() as usize + 1;
const CLASSES: usize = POW2_CLASSES + CLASS_MAX / PAGE - 1;
/// The classes up to this size keep every block freed; a larger one keeps
/// at most [`Classed::class_max`] bytes.
const SMALL_MAX: usize = 512;
const SMALL_CLASSES: usize = (SMALL_MAX / 16).trailing_zeros() as usize + 1;

/// The heap and the free lists of its size classes.
struct Classed {
    heap: Heap,
    /// Each class's free blocks, linked through their first word.
    free: [*mut u8; CLASSES],
    /// The bytes in each class's free list.
    class_cached: [usize; CLASSES],
    /// The bytes in the free lists: free, for [`usage`].
    cached: usize,
    /// The bytes in the free lists of the classes above [`SMALL_MAX`].
    large_cached: usize,
    /// What a class above [`SMALL_MAX`] keeps at most, and what they all
    /// keep together: a 64th and an eighth of the heap, 4 MiB a class at
    /// most ([`init`]). Beyond it a freed block goes back to the heap: the
    /// lists stay a cache of what runs, not a hoard that fragments the
    /// heap for its large allocations.
    class_max: usize,
    large_max: usize,
}

// The free lists' pointers are the heap's own memory.
unsafe impl Send for Classed {}

impl Classed {
    const fn new() -> Self {
        Classed {
            heap: Heap::empty(),
            free: [core::ptr::null_mut(); CLASSES],
            class_cached: [0; CLASSES],
            cached: 0,
            large_cached: 0,
            class_max: 0,
            large_max: 0,
        }
    }

    /// The class of a block of `layout`, or `None` for a large one (or one
    /// aligned beyond the 16 bytes every class's blocks have: a module's
    /// image wants pages, and takes them from the heap itself).
    fn class(layout: Layout) -> Option<usize> {
        let size = layout.size();
        if size > CLASS_MAX || layout.align() > 16 {
            return None;
        }
        Some(if size <= PAGE {
            (size.max(16).next_power_of_two() / 16).trailing_zeros() as usize
        } else {
            POW2_CLASSES - 2 + size.div_ceil(PAGE)
        })
    }

    /// The size of class `c`'s blocks.
    fn class_size(c: usize) -> usize {
        if c < POW2_CLASSES {
            16 << c
        } else {
            (c + 2 - POW2_CLASSES) * PAGE
        }
    }

    /// The layout of class `c`'s blocks in the heap.
    fn class_layout(c: usize) -> Layout {
        Layout::from_size_align(Self::class_size(c), 16).unwrap()
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
        self.uncache(c, Self::class_size(c));
        block
    }

    /// A block from the heap itself. When it has no room, the free lists
    /// of the classes above [`SMALL_MAX`] go back to it, largest class
    /// first (the most bytes in the fewest blocks: every block given back
    /// walks the holes), until the block fits. A request for more than the
    /// heap and those lists hold together is refused without that.
    fn heap_alloc(&mut self, layout: Layout) -> *mut u8 {
        if let Ok(p) = self.heap.allocate_first_fit(layout) {
            return p.as_ptr();
        }
        if layout.size() > self.heap.free() + self.large_cached {
            return core::ptr::null_mut();
        }
        for c in (SMALL_CLASSES..CLASSES).rev() {
            if self.free[c].is_null() {
                continue;
            }
            self.drain(c);
            if let Ok(p) = self.heap.allocate_first_fit(layout) {
                return p.as_ptr();
            }
        }
        core::ptr::null_mut()
    }

    /// Give every block of class `c`'s free list back to the heap.
    fn drain(&mut self, c: usize) {
        let layout = Self::class_layout(c);
        let mut block = core::mem::replace(&mut self.free[c], core::ptr::null_mut());
        while !block.is_null() {
            let next = unsafe { *(block as *mut *mut u8) };
            unsafe { self.heap.deallocate(core::ptr::NonNull::new_unchecked(block), layout) };
            block = next;
        }
        self.uncache(c, self.class_cached[c]);
    }

    fn uncache(&mut self, c: usize, bytes: usize) {
        self.class_cached[c] -= bytes;
        self.cached -= bytes;
        if c >= SMALL_CLASSES {
            self.large_cached -= bytes;
        }
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        let Some(c) = Self::class(layout) else {
            return unsafe { self.heap.deallocate(core::ptr::NonNull::new_unchecked(ptr), layout) };
        };
        let size = Self::class_size(c);
        if c >= SMALL_CLASSES && (self.class_cached[c] + size > self.class_max || self.large_cached + size > self.large_max) {
            return unsafe { self.heap.deallocate(core::ptr::NonNull::new_unchecked(ptr), Self::class_layout(c)) };
        }
        unsafe { *(ptr as *mut *mut u8) = self.free[c] };
        self.free[c] = ptr;
        self.class_cached[c] += size;
        self.cached += size;
        if c >= SMALL_CLASSES {
            self.large_cached += size;
        }
    }
}

/// The classes a CPU caches blocks of: the powers of two up to a page.
const CPU_CLASSES: usize = POW2_CLASSES;
/// What a CPU keeps of one class at most, in bytes, and in blocks.
const CPU_CLASS_BYTES: usize = 32 * 1024;
const CPU_CLASS_BLOCKS: usize = 64;

/// One CPU's blocks of each cached class, linked through their first word
/// like the classes' own lists.
struct CpuCache {
    free: [*mut u8; CPU_CLASSES],
    count: [usize; CPU_CLASSES],
}

/// The CPUs' caches. Each is its CPU's alone, and only with interrupts off
/// (an interrupt's handler allocates too).
struct CpuCaches([UnsafeCell<CpuCache>; crate::smp::MAX_CPUS]);

unsafe impl Sync for CpuCaches {}

static CPU_CACHES: CpuCaches = CpuCaches(
    [const { UnsafeCell::new(CpuCache { free: [core::ptr::null_mut(); CPU_CLASSES], count: [0; CPU_CLASSES] }) };
        crate::smp::MAX_CPUS],
);
/// The bytes in each CPU's cache: free, for [`usage`].
static CPU_CACHED: [AtomicUsize; crate::smp::MAX_CPUS] = [const { AtomicUsize::new(0) }; crate::smp::MAX_CPUS];

/// How many blocks of class `c` a CPU keeps at most.
fn cpu_class_max(c: usize) -> usize {
    (CPU_CLASS_BYTES / Classed::class_size(c)).clamp(2, CPU_CLASS_BLOCKS)
}

/// This CPU's cache and its index. Interrupts are off.
#[allow(clippy::mut_from_ref)]
fn cpu_cache() -> (&'static mut CpuCache, usize) {
    let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
    (unsafe { &mut *CPU_CACHES.0[cpu].get() }, cpu)
}

impl IrqSafeHeap {
    /// A block of class `c` from this CPU's cache; when that is empty, one
    /// from the class's list or the heap, with half a cache of the list's
    /// blocks for the next ones. Interrupts are off.
    fn alloc_cpu(&self, c: usize) -> *mut u8 {
        let (cache, cpu) = cpu_cache();
        let size = Classed::class_size(c);
        let block = cache.free[c];
        if !block.is_null() {
            cache.free[c] = unsafe { *(block as *mut *mut u8) };
            cache.count[c] -= 1;
            CPU_CACHED[cpu].fetch_sub(size, Ordering::Relaxed);
            return block;
        }
        let mut classed = self.0.lock();
        let block = classed.alloc(Classed::class_layout(c));
        // Only what the list holds: the heap itself is walked for one
        // block at a time, when it must be.
        let mut taken = 0;
        while taken < cpu_class_max(c) / 2 && !classed.free[c].is_null() {
            let more = classed.alloc(Classed::class_layout(c));
            unsafe { *(more as *mut *mut u8) = cache.free[c] };
            cache.free[c] = more;
            taken += 1;
        }
        cache.count[c] += taken;
        CPU_CACHED[cpu].fetch_add(taken * size, Ordering::Relaxed);
        block
    }

    /// Give a block of class `c` to this CPU's cache; a full cache gives
    /// half of itself back to the class's list first. Interrupts are off.
    fn dealloc_cpu(&self, c: usize, ptr: *mut u8) {
        let (cache, cpu) = cpu_cache();
        let size = Classed::class_size(c);
        if cache.count[c] >= cpu_class_max(c) {
            let mut classed = self.0.lock();
            let mut given = 0;
            while given < cpu_class_max(c) / 2 {
                let block = cache.free[c];
                cache.free[c] = unsafe { *(block as *mut *mut u8) };
                unsafe { classed.dealloc(block, Classed::class_layout(c)) };
                given += 1;
            }
            cache.count[c] -= given;
            CPU_CACHED[cpu].fetch_sub(given * size, Ordering::Relaxed);
        }
        unsafe { *(ptr as *mut *mut u8) = cache.free[c] };
        cache.free[c] = ptr;
        cache.count[c] += 1;
        CPU_CACHED[cpu].fetch_add(size, Ordering::Relaxed);
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
        let p = match Classed::class(layout) {
            Some(c) if c < CPU_CLASSES => self.alloc_cpu(c),
            _ => self.0.lock().alloc(layout),
        };
        crate::arch::irq_restore(flags);
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let flags = crate::arch::irq_save();
        crate::arch::irq_off();
        match Classed::class(layout) {
            Some(c) if c < CPU_CLASSES => self.dealloc_cpu(c, ptr),
            _ => unsafe { self.0.lock().dealloc(ptr, layout) },
        }
        crate::arch::irq_restore(flags);
    }

    /// A block of a size class is the whole of its class's block, so a
    /// `Vec` growing or shrinking within the class keeps it (the default
    /// would take a second block of the class and copy).
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        if let (Some(old), Some(new)) = (Classed::class(layout), Classed::class(new_layout)) {
            if old == new {
                return ptr;
            }
        }
        let new_ptr = unsafe { self.alloc(new_layout) };
        if !new_ptr.is_null() {
            unsafe {
                core::ptr::copy_nonoverlapping(ptr, new_ptr, layout.size().min(new_size));
                self.dealloc(ptr, layout);
            }
        }
        new_ptr
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
    let mut classed = ALLOCATOR.0.lock();
    unsafe { classed.heap.init(start as *mut u8, size as usize) };
    classed.class_max = (size as usize / 64).min(4 << 20);
    classed.large_max = size as usize / 8;
    drop(classed);
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
    let in_cpus: usize = CPU_CACHED.iter().map(|n| n.load(Ordering::Relaxed)).sum();
    (size, free + in_cpus)
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
