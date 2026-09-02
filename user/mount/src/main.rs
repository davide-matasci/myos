#![no_std]
#![no_main]

use myos_user::{exit, mount, write};

myos_user::x86_start!(main);

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: usize, argv: *const usize) -> ! {
    unsafe { myos_user::args::init_from_regs(argc, argv) };
    main()
}

fn main() -> ! {
    if myos_user::argc() != 4 {
        write(b"usage: mount <source> <target> <fstype>\n");
        myos_user::exit_code(1);
    }
    let src = myos_user::arg(1).unwrap_or(b"");
    let tgt = myos_user::arg(2).unwrap_or(b"");
    let fs = myos_user::arg(3).unwrap_or(b"");
    if !mount(src, tgt, fs) {
        write(b"mount failed\n");
        myos_user::exit_code(1);
    }
    exit();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
