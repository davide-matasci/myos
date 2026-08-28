#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use myos_user::{exec, exit, heap_init, write, Heap};

#[global_allocator]
static GLOBAL: Heap = Heap;

#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    main()
}

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_argc: usize, _argv: *const usize) -> ! {
    main()
}

fn main() -> ! {
    heap_init();
    let mut v = Vec::new();
    v.extend_from_slice(b"alloc ok\n");
    write(&v);
    #[cfg(target_arch = "x86_64")]
    exec(b"/stdhello", &[]);
    exit();
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
