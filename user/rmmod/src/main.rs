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
    if myos_user::argc() != 2 {
        write(b"usage: rmmod <name>   (a loaded module, see /proc/modules)\n");
        myos_user::exit_code(1);
    }
    let name = myos_user::arg(1).unwrap_or(b"");
    if !myos_user::rmmod(name) {
        write(b"rmmod: unload failed\n");
        myos_user::exit_code(1);
    }
    exit();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
