//! Hello module. Speaks only through [`myos_abi::KernelApi`] — no kernel internals.

#![no_std]
#![no_main]

use myos_abi::{KernelApi, ABI_VERSION};

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api: *const KernelApi) -> i32 {
    unsafe {
        if api.is_null() {
            return -1;
        }
        let api = &*api;
        if api.abi_version != ABI_VERSION {
            return -2;
        }
        let msg = b"mod ok\n";
        (api.write_str)(msg.as_ptr(), msg.len());
        0
    }
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
