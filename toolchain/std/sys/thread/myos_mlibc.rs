//! Threads on the mlibc flavour (`sys::myos::libc`): the C library's, so
//! that the program's C runs on them as on its own (its `errno`, its
//! thread-local variables, its locks, all at the libc's thread pointer).
//! The libc maps and frees the stack, with a guard page under it.

use crate::num::NonZero;
use crate::sys::myos::{abi, libc};
use crate::thread::ThreadInit;
use crate::time::{Duration, SystemTime};
use crate::{io, mem, ptr};

/// `std::thread::spawn`'s stack size, as on the other flavour.
pub const DEFAULT_MIN_STACK_SIZE: usize = 1 << 20;

pub struct Thread {
    id: libc::pthread_t,
}

unsafe impl Send for Thread {}
unsafe impl Sync for Thread {}

impl Thread {
    // unsafe: see thread::Builder::spawn_unchecked for safety requirements
    pub unsafe fn new(stack: usize, init: Box<ThreadInit>) -> io::Result<Thread> {
        let init = Box::into_raw(init);
        let mut attr = libc::pthread_attr_t::zeroed();
        let mut id = ptr::null_mut();
        let r = unsafe {
            if libc::pthread_attr_init(&mut attr) != 0 {
                drop(Box::from_raw(init));
                return Err(io::const_error!(io::ErrorKind::OutOfMemory, "no thread attributes"));
            }
            libc::pthread_attr_setstacksize(&mut attr, stack.max(16 * 1024).next_multiple_of(4096));
            let r = libc::pthread_create(&mut id, &attr, start, init.cast());
            libc::pthread_attr_destroy(&mut attr);
            r
        };
        if r != 0 {
            // The thread never took its closure.
            unsafe { drop(Box::from_raw(init)) };
            return Err(io::const_error!(io::ErrorKind::WouldBlock, "no task for the thread"));
        }
        Ok(Thread { id })
    }

    pub fn join(self) {
        let id = self.id;
        mem::forget(self);
        let r = unsafe { libc::pthread_join(id, ptr::null_mut()) };
        assert!(r == 0, "failed to join thread");
    }
}

impl Drop for Thread {
    fn drop(&mut self) {
        let r = unsafe { libc::pthread_detach(self.id) };
        debug_assert_eq!(r, 0);
    }
}

/// A new thread's entry. Its thread-local destructors are the keys': the
/// libc runs them once this returns.
extern "C" fn start(init: *mut crate::ffi::c_void) -> *mut crate::ffi::c_void {
    let init = unsafe { Box::from_raw(init.cast::<ThreadInit>()) };
    let rust_start = init.init();
    rust_start();
    ptr::null_mut()
}

/// The CPUs: the `cpuN` lines of `/proc/cpu`.
pub fn available_parallelism() -> io::Result<NonZero<usize>> {
    let text = crate::fs::read("/proc/cpu")?;
    let n = text.split(|&b| b == b'\n').filter(|l| l.starts_with(b"cpu")).count();
    NonZero::new(n).ok_or(io::Error::UNKNOWN_THREAD_COUNT)
}

pub fn current_os_id() -> Option<u64> {
    Some(abi::gettid() as u64)
}

pub fn yield_now() {
    abi::yield_now();
}

pub fn sleep(dur: Duration) {
    let start = SystemTime::now();
    let mut left = dur;
    loop {
        if abi::nanosleep(left) {
            return;
        }
        // Interrupted: sleep what is left, by the wall clock.
        let slept = SystemTime::now().duration_since(start).unwrap_or(Duration::ZERO);
        match dur.checked_sub(slept) {
            Some(l) if !l.is_zero() => left = l,
            _ => return,
        }
    }
}
