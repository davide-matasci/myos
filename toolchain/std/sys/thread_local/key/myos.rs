//! Thread-local storage on myos: a table of pointers per thread, one slot
//! per key, at the thread pointer (`thread_spawn`'s `tls`, `set_tp` for the
//! main thread). Word 0 points at the table itself, so x86_64 code reads the
//! table's address at `fs:0` (it cannot read the FS base).
//!
//! Keys are never reused. Destructors are kept in a list and run by
//! [`run_dtors`] when a thread ends (`sys::thread`), as on Xous; the main
//! thread's are not run.

use crate::alloc::System;
use crate::mem::ManuallyDrop;
use crate::ptr;
use crate::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use crate::sync::atomic::{Atomic, AtomicPtr, AtomicUsize};

pub type Key = usize;
pub type Dtor = unsafe extern "C" fn(*mut u8);

/// Words in a table: the self pointer and the keys.
pub const TABLE_WORDS: usize = 500;

/// A thread's table.
#[repr(C)]
pub struct Table(pub [*mut u8; TABLE_WORDS]);

/// The main thread's table.
static mut MAIN: Table = Table([ptr::null_mut(); TABLE_WORDS]);

/// Keys start at 1: word 0 is the self pointer.
static NEXT_KEY: Atomic<usize> = AtomicUsize::new(1);

static DTORS: Atomic<*mut Node> = AtomicPtr::new(ptr::null_mut());

/// Make `table` (zeroed) the calling thread's: its self pointer.
pub fn init_table(table: *mut Table) {
    unsafe { (*table).0[0] = table.cast() };
}

/// Give the main thread its table (`sys::pal::init`).
pub fn init_main() {
    let table = &raw mut MAIN;
    init_table(table);
    crate::sys::myos::abi::set_tp(table.addr());
}

#[inline]
fn table() -> *mut *mut u8 {
    let tp: *mut *mut u8;
    unsafe {
        #[cfg(target_arch = "x86_64")]
        core::arch::asm!("mov {}, qword ptr fs:[0]", out(reg) tp, options(nostack, readonly, preserves_flags));
        #[cfg(target_arch = "aarch64")]
        core::arch::asm!("mrs {}, tpidr_el0", out(reg) tp, options(nostack, nomem, preserves_flags));
        #[cfg(target_arch = "riscv64")]
        core::arch::asm!("mv {}, tp", out(reg) tp, options(nostack, nomem, preserves_flags));
    }
    tp
}

#[inline]
pub fn create(dtor: Option<Dtor>) -> Key {
    let key = NEXT_KEY.fetch_add(1, Relaxed);
    if key >= TABLE_WORDS {
        rtabort!("out of thread-local keys");
    }
    if let Some(f) = dtor {
        unsafe { register_dtor(key, f) };
    }
    key
}

#[inline]
pub unsafe fn set(key: Key, value: *mut u8) {
    debug_assert!(key >= 1 && key < TABLE_WORDS);
    unsafe { *table().add(key) = value };
}

#[inline]
pub unsafe fn get(key: Key) -> *mut u8 {
    debug_assert!(key >= 1 && key < TABLE_WORDS);
    unsafe { *table().add(key) }
}

#[inline]
pub unsafe fn destroy(_key: Key) {
    // Keys are not reused.
}

struct Node {
    dtor: Dtor,
    key: Key,
    next: *mut Node,
}

unsafe fn register_dtor(key: Key, dtor: Dtor) {
    // The System allocator: a global allocator may use thread-locals.
    let mut node =
        ManuallyDrop::new(Box::new_in(Node { key, dtor, next: ptr::null_mut() }, System));
    let mut head = DTORS.load(Acquire);
    loop {
        node.next = head;
        match DTORS.compare_exchange(head, &mut **node, Release, Acquire) {
            Ok(_) => return,
            Err(cur) => head = cur,
        }
    }
}

/// Run the calling thread's destructors, again while they set values (up
/// to 5 rounds, as Windows and Xous do), then the runtime's cleanup.
// Not inlined: no deallocation may move past it (see the Xous version).
#[inline(never)]
pub unsafe fn run_dtors() {
    for _ in 0..5 {
        let mut any_run = false;
        let mut cur = DTORS.load(Acquire);
        while !cur.is_null() {
            let key = unsafe { (*cur).key };
            let ptr = unsafe { get(key) };
            if !ptr.is_null() {
                unsafe {
                    set(key, ptr::null_mut());
                    ((*cur).dtor)(ptr);
                }
                any_run = true;
            }
            cur = unsafe { (*cur).next };
        }
        if !any_run {
            break;
        }
    }
    crate::rt::thread_cleanup();
}
