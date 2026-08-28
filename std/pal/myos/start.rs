//! `_start` for `std` programs on x86_64-myos (SysV argc/argv on stack).

#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    crate::rt::lang_start(crate::main)
}

// AArch64 myos passes argc/argv in x0/x1 from the kernel — wire when PAL grows:
// #[cfg(target_arch = "aarch64")]
// #[unsafe(no_mangle)]
// pub unsafe extern "C" fn _start(_argc: isize, _argv: *const *const u8) -> ! {
//     crate::rt::lang_start(crate::main)
// }
