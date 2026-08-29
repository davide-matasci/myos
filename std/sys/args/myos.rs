//! myos argv: copy syscall/exec stack args into static storage before `main`
//! runs. The unix backend only stores a pointer into the stack slot; `main`'s
//! prologue clobbers that region for multi-arg exec (std-cat, std-echo).

pub use super::common::Args;
use crate::ffi::{OsStr, OsString};
use crate::os::myos::ffi::{OsStrExt, OsStringExt};

const MAX_ARGS: usize = 16;
const MAX_ARG_LEN: usize = 128;

static mut ARGC: usize = 0;
static mut ARGV_STORAGE: [[u8; MAX_ARG_LEN]; MAX_ARGS] = [[0; MAX_ARG_LEN]; MAX_ARGS];
static mut ARGV_LENS: [usize; MAX_ARGS] = [0; MAX_ARGS];

/// One-time global initialization.
pub unsafe fn init(argc: isize, argv: *const *const u8) {
    unsafe {
        ARGC = 0;
        if argv.is_null() || argc <= 0 {
            return;
        }
        let n = (argc as usize).min(MAX_ARGS);
        for i in 0..n {
            let ptr = *argv.add(i);
            if ptr.is_null() {
                break;
            }
            let mut len = 0usize;
            while len < MAX_ARG_LEN {
                let b = *ptr.add(len);
                if b == 0 {
                    break;
                }
                len += 1;
            }
            ARGV_STORAGE[i][..len].copy_from_slice(core::slice::from_raw_parts(ptr, len));
            ARGV_LENS[i] = len;
            ARGC += 1;
        }
    }
}

/// Borrow argv entries copied during [`init`]. No heap allocation.
#[unstable(feature = "myos_ext", issue = "none")]
pub fn static_args() -> StaticArgs {
    StaticArgs { idx: 0 }
}

/// Number of arguments captured during [`init`].
#[unstable(feature = "myos_ext", issue = "none")]
pub fn count() -> usize {
    unsafe { ARGC }
}

#[unstable(feature = "myos_ext", issue = "none")]
pub struct StaticArgs {
    idx: usize,
}

#[unstable(feature = "myos_ext", issue = "none")]
impl Iterator for StaticArgs {
    type Item = &'static OsStr;

    fn next(&mut self) -> Option<&'static OsStr> {
        unsafe {
            if self.idx >= ARGC {
                return None;
            }
            let len = ARGV_LENS[self.idx];
            let bytes = &ARGV_STORAGE[self.idx][..len];
            self.idx += 1;
            Some(OsStr::from_bytes(bytes))
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        unsafe {
            let rem = ARGC.saturating_sub(self.idx);
            (rem, Some(rem))
        }
    }
}

#[unstable(feature = "myos_ext", issue = "none")]
impl ExactSizeIterator for StaticArgs {
    fn len(&self) -> usize {
        unsafe { ARGC.saturating_sub(self.idx) }
    }
}

/// Returns the command line arguments (heap-backed; prefer [`static_args`] on myos).
#[inline(never)]
pub fn args() -> Args {
    let mut vec = crate::vec::Vec::with_capacity(unsafe { ARGC });
    for arg in static_args() {
        vec.push(OsStringExt::from_vec(arg.as_bytes().to_vec()));
    }
    Args::new(vec)
}
