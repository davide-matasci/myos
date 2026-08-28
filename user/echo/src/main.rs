#![no_std]
#![no_main]

use myos_user::{exit, write};

#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    unsafe { myos_user::args::init_from_stack() };
    main()
}

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: usize, argv: *const usize) -> ! {
    unsafe { myos_user::args::init_from_regs(argc, argv) };
    main()
}

fn main() -> ! {
    let mut first = true;
    for i in 1..myos_user::argc() {
        if let Some(a) = myos_user::arg(i) {
            if !first {
                write(b" ");
            }
            write(a);
            first = false;
        }
    }
    write(b"\n");
    exit();
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
