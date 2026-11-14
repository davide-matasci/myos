//! Thread-local storage on the mlibc flavour (`sys::myos::libc`): the C
//! library's keys. The thread pointer is the libc's, so `std` keeps no
//! table of its own there, and a key's destructor runs when a thread ends
//! whoever started it (a thread C code made with `pthread_create` too).

use crate::sys::myos::libc;

pub type Key = libc::pthread_key_t;

#[inline]
pub fn create(dtor: Option<unsafe extern "C" fn(*mut u8)>) -> Key {
    let mut key = 0;
    if unsafe { libc::pthread_key_create(&mut key, dtor) } != 0 {
        rtabort!("out of thread-local keys");
    }
    key
}

#[inline]
pub unsafe fn set(key: Key, value: *mut u8) {
    let r = unsafe { libc::pthread_setspecific(key, value.cast()) };
    debug_assert_eq!(r, 0);
}

#[inline]
pub unsafe fn get(key: Key) -> *mut u8 {
    unsafe { libc::pthread_getspecific(key).cast() }
}

#[inline]
pub unsafe fn destroy(key: Key) {
    let r = unsafe { libc::pthread_key_delete(key) };
    debug_assert_eq!(r, 0);
}
