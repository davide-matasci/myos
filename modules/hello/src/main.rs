//! Hello module. Speaks only through [`myos_abi::KernelApi`] — no kernel internals.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use myos_abi::{KernelApi, ABI_VERSION};

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api: *const KernelApi) -> i32 {
    let Some(api) = (unsafe { api.as_ref() }) else {
        return -1;
    };
    if api.abi_version != ABI_VERSION {
        return -2;
    }
    0
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

// So `cargo rustc --bin hello` links. The kernel never jumps here.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
