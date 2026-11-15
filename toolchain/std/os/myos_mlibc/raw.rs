//! Myos-specific raw type definitions.

#![stable(feature = "raw_ext", since = "1.1.0")]
#![deprecated(
    since = "1.8.0",
    note = "these type aliases are no longer supported by \
            the standard library, the `libc` crate on \
            crates.io should be used instead for the correct \
            definitions"
)]
#![allow(deprecated)]

use crate::os::raw::c_ulong;

#[stable(feature = "pthread_t", since = "1.8.0")]
pub type pthread_t = c_ulong;

#[stable(feature = "raw_ext", since = "1.1.0")]
pub use libc::{blkcnt_t, blksize_t, ino_t, nlink_t, off_t, time_t, dev_t, mode_t};
