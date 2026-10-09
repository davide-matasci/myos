//! Allocator on syscall `brk` (nr 9): blocks up to [`MAX_CLASS`] are rounded
//! up to a power of two and a freed one goes on its size's free list for the
//! next allocation of that size; bigger (or over-aligned) blocks are bumped
//! off the break and never reused. A long-lived server (netd makes and drops
//! a socket's buffers per connection) stays bounded.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

const PAGE: usize = 4096;
const MIN_CLASS: usize = 16;
const MAX_CLASS: usize = 1 << 20;
const CLASSES: usize = (MAX_CLASS / MIN_CLASS).trailing_zeros() as usize + 1;

/// The break as seen by the allocator, and its free lists.
struct State {
    brk_ptr: usize,
    brk_end: usize,
    free: [*mut FreeBlock; CLASSES],
}

struct FreeBlock {
    next: *mut FreeBlock,
}

static LOCK: AtomicBool = AtomicBool::new(false);
static mut STATE: State = State { brk_ptr: 0, brk_end: 0, free: [ptr::null_mut(); CLASSES] };

/// The allocator state, for as long as the guard lives (threads share it).
fn lock() -> StateGuard {
    while LOCK.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    StateGuard
}

struct StateGuard;

impl StateGuard {
    fn state(&mut self) -> &mut State {
        unsafe { &mut *ptr::addr_of_mut!(STATE) }
    }
}

impl Drop for StateGuard {
    fn drop(&mut self) {
        LOCK.store(false, Ordering::Release);
    }
}

/// Query the program break and seed the allocator.
pub fn heap_init() {
    let end = crate::brk(0);
    let mut g = lock();
    let st = g.state();
    st.brk_ptr = end;
    st.brk_end = end;
}

impl State {
    /// `size` bytes aligned to `align` off the break, growing it as needed.
    fn bump(&mut self, size: usize, align: usize) -> *mut u8 {
        if self.brk_end == 0 {
            let end = crate::brk(0);
            self.brk_ptr = end;
            self.brk_end = end;
        }
        let aligned = (self.brk_ptr + align - 1) & !(align - 1);
        let Some(new_end) = aligned.checked_add(size) else {
            return ptr::null_mut();
        };
        if new_end > self.brk_end {
            let new_brk = (new_end + PAGE - 1) & !(PAGE - 1);
            let got = crate::brk(new_brk);
            if got < new_brk {
                return ptr::null_mut();
            }
            self.brk_end = got;
        }
        self.brk_ptr = new_end;
        aligned as *mut u8
    }
}

/// Page-aligned bump allocation coordinated with [`Heap`].
///
/// Used by `user/tls` on aarch64/riscv64 for the mbedtls arena so the 2 MiB
/// buffer lives in the brk heap (after the user stack) instead of ELF BSS.
/// A BSS arena was contiguous with the stack: an MPI over-read could walk
/// image→stack→heap and fault at `heap_limit` after corrupting on-stack TLS
/// state (riscv64 CI page faults after #100 restored BSS for x86).
pub fn alloc_aligned(size: usize, align: usize) -> *mut u8 {
    let align = align.max(1).next_power_of_two();
    lock().state().bump(size.max(1), align)
}

/// The free list `layout` allocates from, if any.
fn class_of(layout: Layout) -> Option<usize> {
    let size = layout.size().max(layout.align()).max(MIN_CLASS);
    (size <= MAX_CLASS && layout.align() <= PAGE)
        .then(|| (size.next_power_of_two() / MIN_CLASS).trailing_zeros() as usize)
}

pub struct Heap;

unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut g = lock();
        let st = g.state();
        let Some(c) = class_of(layout) else {
            return st.bump(layout.size().max(1), layout.align());
        };
        let block = st.free[c];
        if !block.is_null() {
            st.free[c] = unsafe { (*block).next };
            return block as *mut u8;
        }
        let size = MIN_CLASS << c;
        st.bump(size, size.min(PAGE))
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        // A block bumped off the break for good stays allocated.
        let Some(c) = class_of(layout) else {
            return;
        };
        let mut g = lock();
        let st = g.state();
        let block = p as *mut FreeBlock;
        unsafe { (*block).next = st.free[c] };
        st.free[c] = block;
    }

    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        if class_of(layout).is_some() && class_of(layout) == class_of(new_layout) {
            return p;
        }
        let q = unsafe { self.alloc(new_layout) };
        if !q.is_null() {
            unsafe {
                ptr::copy_nonoverlapping(p, q, layout.size().min(new_size));
                self.dealloc(p, layout);
            }
        }
        q
    }
}
