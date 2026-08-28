//! Platform abstraction layer for `target_os = "myos"`.
//!
//! Copy into `library/std/src/sys/pal/myos/` inside a patched Rust tree.

mod alloc;
mod args;
mod os;
mod start;
mod thread_local_key;

pub use alloc::init as init_alloc;
pub use args::init as init_args;
