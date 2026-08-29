#![no_std]
#![no_main]

use myos_user::{exit, write};

myos_user::x86_start!(main);

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
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
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
