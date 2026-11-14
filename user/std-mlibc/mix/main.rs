//! `std` on mlibc (`<arch>-unknown-myos-mlibc`, docs/userspace.md "Rust
//! userspace"): a Rust program with C in it, both on one C library.
//!
//! - The program starts through mlibc's startup and gets its arguments;
//!   what `main` returns is the exit status, after the libc's `atexit`
//!   functions ran (the last line is printed from one).
//! - One heap: Rust frees what C's `malloc` gave, and C what Rust's
//!   allocator did; over-aligned and zeroed blocks are what was asked for.
//! - `std::thread` threads are the libc's: C code on them has its own
//!   `errno`, stdio and thread-local variables.
//! - A thread C starts with `pthread_create` runs Rust: it has its own
//!   `thread_local!` values, whose destructors run when it ends.
//! - Threads share memory through the locks, as on the other flavour.
//!
//! Prints `[ OK ] std mlibc` (from the `atexit` function) or a
//! `[ FAIL ] std mlibc: ...` line.

use std::alloc::{Layout, alloc, alloc_zeroed, dealloc, realloc};
use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

unsafe extern "C" {
    fn mix_dup(s: *const c_char) -> *mut c_char;
    fn mix_free(p: *mut c_void);
    fn mix_thread_state(value: c_int) -> c_int;
    fn mix_on_c_thread(f: extern "C" fn(*mut c_void), arg: *mut c_void) -> c_int;
    fn mix_atexit(f: extern "C" fn()) -> c_int;
    fn free(p: *mut c_void);
}

fn check(ok: bool, what: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what.to_string()) }
}

fn heap() -> Result<(), String> {
    // C's malloc, freed by Rust (through the libc's free: the same heap).
    let c = unsafe { mix_dup(c"from malloc".as_ptr()) };
    check(!c.is_null(), "mix_dup")?;
    check(unsafe { CStr::from_ptr(c) }.to_bytes() == b"from malloc", "the C string")?;
    unsafe { free(c.cast()) };
    // Rust's allocator, freed by C.
    let s = CString::new("from rust").unwrap();
    unsafe { mix_free(s.into_raw().cast()) };
    let v: Vec<u64> = (0..1000).collect();
    let p = Box::into_raw(v.into_boxed_slice());
    unsafe { mix_free(p.cast()) };
    // Over-aligned, zeroed, and grown in place or moved.
    for align in [1usize, 8, 16, 64, 4096] {
        let layout = Layout::from_size_align(3 * align + 5, align).unwrap();
        unsafe {
            let p = alloc_zeroed(layout);
            check(!p.is_null() && p.addr() % align == 0, "alloc_zeroed's alignment")?;
            check((0..layout.size()).all(|i| *p.add(i) == 0), "alloc_zeroed's zeros")?;
            p.write_bytes(0xa5, layout.size());
            let q = realloc(p, layout, layout.size() * 4);
            check(!q.is_null() && q.addr() % align == 0, "realloc's alignment")?;
            check((0..layout.size()).all(|i| *q.add(i) == 0xa5), "realloc's bytes")?;
            dealloc(q, Layout::from_size_align(layout.size() * 4, align).unwrap());
            let r = alloc(layout);
            check(!r.is_null() && r.addr() % align == 0, "alloc's alignment")?;
            dealloc(r, layout);
        }
    }
    Ok(())
}

/// `std::thread` threads: C's per-thread state is each one's own, and
/// they share a counter under a mutex.
fn rust_threads() -> Result<(), String> {
    let total = Arc::new(Mutex::new(0usize));
    let handles: Vec<_> = (0..4)
        .map(|i| {
            let total = total.clone();
            thread::spawn(move || {
                for _ in 0..1000 {
                    *total.lock().unwrap() += 1;
                }
                unsafe { mix_thread_state(100 + i) }
            })
        })
        .collect();
    for h in handles {
        check(h.join().map_err(|_| "a thread panicked")? == 0, "C's state on a Rust thread")?;
    }
    check(*total.lock().unwrap() == 4000, "the shared counter")?;
    check(unsafe { mix_thread_state(7) } == 0, "C's state on the main thread")
}

static DROPPED: AtomicUsize = AtomicUsize::new(0);

struct Noisy(Cell<u32>);

impl Drop for Noisy {
    fn drop(&mut self) {
        DROPPED.fetch_add(1, Ordering::SeqCst);
    }
}

thread_local! {
    static LOCAL: Noisy = Noisy(Cell::new(5));
}

extern "C" fn on_c_thread(seen: *mut c_void) {
    // This thread's own value, not the main thread's 6.
    let v = LOCAL.with(|l| {
        l.0.set(l.0.get() + 10);
        l.0.get()
    });
    unsafe { *seen.cast::<u32>() = v };
    // The runtime works here: the heap, a lock, a nested Rust thread.
    let n = thread::spawn(|| vec![1u32; 100].iter().sum::<u32>()).join().unwrap_or(0);
    unsafe { *seen.cast::<u32>() += n };
}

/// A thread C made runs Rust, with its own thread-locals, destroyed when
/// it ends.
fn c_threads() -> Result<(), String> {
    LOCAL.with(|l| l.0.set(6));
    let mut seen = 0u32;
    check(unsafe { mix_on_c_thread(on_c_thread, (&raw mut seen).cast()) } == 0, "mix_on_c_thread")?;
    check(seen == 115, "the C thread's thread-local")?;
    check(LOCAL.with(|l| l.0.get()) == 6, "the main thread's thread-local")?;
    check(DROPPED.load(Ordering::SeqCst) == 1, "the C thread's thread-local was not dropped")
}

extern "C" fn at_exit() {
    println!("[ OK ] std mlibc");
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    check(args.len() == 3 && args[1] == "one" && args[2] == "two", "the arguments")?;
    heap()?;
    rust_threads()?;
    c_threads()?;
    check(unsafe { mix_atexit(at_exit) } == 0, "atexit")
}

fn main() -> ExitCode {
    match run() {
        // 42: the test checks that main's value is the exit status.
        Ok(()) => ExitCode::from(42),
        Err(e) => {
            println!("[ FAIL ] std mlibc: {e}");
            ExitCode::FAILURE
        }
    }
}
