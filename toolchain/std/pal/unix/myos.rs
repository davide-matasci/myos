//! The program's entry on myos (`<arch>-unknown-myos-mlibc`): the program
//! starts as a C one does. The entry hands the C library's startup `main`
//! (what `rustc` emits for the program: it runs the runtime's
//! `lang_start`) and the entry stack, as mlibc's own crt0 does; what
//! `main` returns goes through the libc's `exit`. `main` is taken from the
//! GOT: this code is also in a shared object that holds std (a `dylib`),
//! where nothing calls it and `main` is another object's. Weak: a program
//! whose std is in such an object has no entry of its own and links
//! mlibc's crt0.o, the same code, which then stands.

use crate::ffi::{c_char, c_int};

/// `main`, as `rustc` emits it for the program.
type Main = unsafe extern "C" fn(c_int, *mut *mut c_char, *mut *mut c_char) -> c_int;

unsafe extern "C" {
    /// mlibc's startup (its crt0's): the TLS, the constructors, `main`,
    /// then `exit` with what it returns. `stack` is the entry stack.
    fn __mlibc_entry(main: Main, stack: *mut usize) -> !;
}

#[cfg(not(test))]
#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".text",
    ".weak _start",
    ".type _start, @function",
    "_start:",
    "mov rdi, qword ptr [rip + main@GOTPCREL]",
    "mov rsi, rsp",
    "call {entry}",
    "ud2",
    entry = sym __mlibc_entry,
);

#[cfg(not(test))]
#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".text",
    ".weak _start",
    ".type _start, %function",
    "_start:",
    "adrp x0, :got:main",
    "ldr x0, [x0, :got_lo12:main]",
    "mov x1, sp",
    "bl {entry}",
    "brk #0",
    entry = sym __mlibc_entry,
);

#[cfg(not(test))]
#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    ".text",
    ".weak _start",
    ".type _start, @function",
    "_start:",
    ".option push",
    ".option norelax",
    // From the GOT, as main: a shared object cannot name it PC-relative.
    "la gp, __global_pointer$",
    ".option pop",
    "la a0, main",
    "mv a1, sp",
    "call {entry}",
    "unimp",
    entry = sym __mlibc_entry,
);
