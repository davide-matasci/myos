#![no_std]
#![no_main]

use myos_user::{exit, listdir, write};

myos_user::x86_start!(main);

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: usize, argv: *const usize) -> ! {
    unsafe { myos_user::args::init_from_regs(argc, argv) };
    main()
}

fn main() -> ! {
    let mut buf = [0u8; myos_user::LISTDIR_BUF];
    let n = listdir(b".", &mut buf);
    if n != usize::MAX && n > 0 {
        write(&buf[..n]);
    }
    exit();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
