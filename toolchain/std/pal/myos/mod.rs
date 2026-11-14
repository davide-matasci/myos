//! Platform abstraction layer for `target_os = "myos"`.

#![deny(unsafe_op_in_unsafe_fn)]
#![allow(missing_docs, nonstandard_style)]

pub mod futex;
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
    // Before anything uses a thread-local. On the mlibc flavour the C
    // library's startup has set the thread pointer up already.
    #[cfg(not(target_env = "mlibc"))]
    crate::sys::thread_local::key::init_main();
    unsafe {
        crate::sys::args::init(argc, argv);
    }
}

#[cfg(not(any(test, target_env = "mlibc")))]
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    // Linux/SysV process entry: %rsp ≡ 0 (mod 16) at _start (iret, no return
    // address). A bare `call` then yields callee %rsp ≡ 8 as the ABI requires.
    // Do NOT `push` a dummy pad here: that only matched fe50282's inverted
    // entry (%rsp ≡ 8), which broke oksh SSE frames; with correct ≡0 entry the
    // pad leaves callees at ≡0 and large std/uutils ELFs fault in the kernel.
    core::arch::naked_asm!(
        "mov rdi, [rsp]",
        "lea rsi, [rsp + 8]",
        "call {init}",
        "call {start_main}",
        init = sym init,
        start_main = sym start_main,
    );
}

#[cfg(not(any(test, target_env = "mlibc")))]
#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start(argc: isize, argv: *const *const u8) -> ! {
    unsafe { init(argc, argv, 0) };
    start_main()
}

#[cfg(not(any(test, target_env = "mlibc")))]
#[cfg(target_arch = "riscv64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start(argc: isize, argv: *const *const u8) -> ! {
    unsafe { init(argc, argv, 0) };
    start_main()
}

// The mlibc flavour (`sys::myos::libc`): the program starts as a C one
// does. The entry hands the C library's startup `main` (what `rustc` emits
// for the program: it runs the runtime's `lang_start`, and so `init`
// above, with the arguments) and the entry stack, as mlibc's own crt0
// does; what `main` returns goes through the libc's `exit`.
#[cfg(not(test))]
#[cfg(all(target_env = "mlibc", target_arch = "x86_64"))]
core::arch::global_asm!(
    ".text",
    ".globl _start",
    ".type _start, @function",
    "_start:",
    "lea rdi, [rip + main]",
    "mov rsi, rsp",
    "call {entry}",
    "ud2",
    entry = sym crate::sys::myos::libc::__mlibc_entry,
);

#[cfg(not(test))]
#[cfg(all(target_env = "mlibc", target_arch = "aarch64"))]
core::arch::global_asm!(
    ".text",
    ".globl _start",
    ".type _start, %function",
    "_start:",
    "adrp x0, main",
    "add x0, x0, :lo12:main",
    "mov x1, sp",
    "bl {entry}",
    "brk #0",
    entry = sym crate::sys::myos::libc::__mlibc_entry,
);

#[cfg(not(test))]
#[cfg(all(target_env = "mlibc", target_arch = "riscv64"))]
core::arch::global_asm!(
    ".text",
    ".globl _start",
    ".type _start, @function",
    "_start:",
    ".option push",
    ".option norelax",
    "lla gp, __global_pointer$",
    ".option pop",
    "lla a0, main",
    "mv a1, sp",
    "call {entry}",
    "unimp",
    entry = sym crate::sys::myos::libc::__mlibc_entry,
);

#[cfg(not(any(test, target_env = "mlibc")))]
#[inline(never)]
unsafe fn start_main() -> ! {
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
