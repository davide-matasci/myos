//! What `std` takes from the C library on the mlibc flavour of the target
//! (`<arch>-unknown-myos-mlibc`): the program's start, the heap, the
//! threads and their keys. A program with C or C++ in it (a library that
//! calls `malloc`, keeps `errno`, starts threads) has one heap and one kind
//! of thread then, the libc's, with the thread pointer the libc's own.
//! The rest of `std` stays on the kernel's calls (`abi`), which keep no
//! state the libc would have to share.
//!
//! The types are mlibc's (`toolchain/mlibc`): `pthread_t` a pointer,
//! `pthread_key_t` a `uintptr_t`, `pthread_attr_t` one pointer.

#![allow(non_camel_case_types)]

use crate::ffi::{c_char, c_int, c_void};

pub type pthread_t = *mut c_void;
pub type pthread_key_t = usize;

#[repr(C)]
pub struct pthread_attr_t {
    heap_ptr: *mut c_void,
}

impl pthread_attr_t {
    pub const fn zeroed() -> pthread_attr_t {
        pthread_attr_t { heap_ptr: crate::ptr::null_mut() }
    }
}

/// `main`, as `rustc` emits it for the program.
pub type Main = unsafe extern "C" fn(c_int, *mut *mut c_char, *mut *mut c_char) -> c_int;

// From libc.a (or libc.so), which the target's link names with the
// compiler builtins its code calls (`late-link-args` in
// `targets/<arch>-unknown-myos-mlibc.json`); the program's build says
// where they are (`-L target/mlibc-<arch>/lib`).
unsafe extern "C" {
    /// mlibc's startup (its crt0's): the TLS, the constructors, `main`,
    /// then `exit` with what it returns. `stack` is the entry stack.
    pub fn __mlibc_entry(main: Main, stack: *mut usize) -> !;
    pub fn exit(status: c_int) -> !;

    pub fn malloc(size: usize) -> *mut c_void;
    pub fn calloc(count: usize, size: usize) -> *mut c_void;
    pub fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
    pub fn free(ptr: *mut c_void);
    pub fn posix_memalign(out: *mut *mut c_void, align: usize, size: usize) -> c_int;

    pub fn pthread_attr_init(attr: *mut pthread_attr_t) -> c_int;
    pub fn pthread_attr_destroy(attr: *mut pthread_attr_t) -> c_int;
    pub fn pthread_attr_setstacksize(attr: *mut pthread_attr_t, size: usize) -> c_int;
    pub fn pthread_create(
        thread: *mut pthread_t,
        attr: *const pthread_attr_t,
        start: extern "C" fn(*mut c_void) -> *mut c_void,
        arg: *mut c_void,
    ) -> c_int;
    pub fn pthread_join(thread: pthread_t, result: *mut *mut c_void) -> c_int;
    pub fn pthread_detach(thread: pthread_t) -> c_int;

    pub fn pthread_key_create(
        key: *mut pthread_key_t,
        dtor: Option<unsafe extern "C" fn(*mut u8)>,
    ) -> c_int;
    pub fn pthread_key_delete(key: pthread_key_t) -> c_int;
    pub fn pthread_getspecific(key: pthread_key_t) -> *mut c_void;
    pub fn pthread_setspecific(key: pthread_key_t, value: *const c_void) -> c_int;
}
