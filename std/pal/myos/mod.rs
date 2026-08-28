//! Platform abstraction layer for `target_os = "myos"`.

#![deny(unsafe_op_in_unsafe_fn)]
#![allow(missing_docs, nonstandard_style)]

#[path = "../unsupported/common.rs"]
mod unsupported_common;
pub use unsupported_common::{cleanup, unsupported, unsupported_err};

use crate::io;
use crate::sys::backtrace;
use crate::sys::myos::abi;

pub fn abort_internal() -> ! {
    abi::exit(101)
}

// SAFETY: must be called only once during runtime initialization.
pub unsafe fn init(argc: isize, argv: *const *const u8, _sigpipe: u8) {
    unsafe {
        crate::sys::args::init(argc, argv);
    }
}

#[cfg(not(test))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    let sp: usize;
    core::arch::asm!("mov {}, rsp", out(reg) sp, options(nomem, nostack));

    let argc = *(sp as *const isize);
    let argv = sp.wrapping_add(core::mem::size_of::<isize>()) as *const *const u8;

    unsafe { init(argc, argv, 0) };

    unsafe extern "C" {
        fn main();
    }

    unsafe { main() };
    abi::exit(0);
}

#[doc(hidden)]
pub trait IsNegative {
    fn is_negative(&self) -> bool;
    fn negate(&self) -> i32;
}

macro_rules! impl_is_negative {
    ($($t:ident)*) => ($(impl IsNegative for $t {
        fn is_negative(&self) -> bool {
            *self < 0
        }

        fn negate(&self) -> i32 {
            i32::try_from(-(*self)).unwrap()
        }
    })*)
}

impl IsNegative for i32 {
    fn is_negative(&self) -> bool {
        *self < 0
    }

    fn negate(&self) -> i32 {
        -(*self)
    }
}
impl_is_negative! { i8 i16 i64 isize }

pub fn cvt<T: IsNegative>(t: T) -> io::Result<T> {
    if t.is_negative() { Err(io::Error::from_raw_os_error(t.negate())) } else { Ok(t) }
}

pub fn cvt_r<T, F>(mut f: F) -> io::Result<T>
where
    T: IsNegative,
    F: FnMut() -> T,
{
    loop {
        match cvt(f()) {
            Err(ref e) if e.is_interrupted() => {}
            other => return other,
        }
    }
}
