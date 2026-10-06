#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![no_main]

//! `poweroff`, `reboot`, `halt`: one program, the action its name says
//! (`power off|reboot|halt` under any other name). The kernel stops the
//! other processes and unmounts the disks first (`docs/power.md`).

use myos_user::{POWER_HALT, POWER_OFF, POWER_REBOOT, write_fd};

myos_user::x86_start!(main);

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: usize, argv: *const usize) -> ! {
    unsafe { myos_user::args::init_from_regs(argc, argv) };
    main()
}

fn main() -> ! {
    let argv0 = myos_user::arg(0).unwrap_or(b"power");
    let name = argv0.rsplit(|&b| b == b'/').next().unwrap_or(argv0);
    let (name, argc) = match name {
        b"poweroff" | b"reboot" | b"halt" => (name, 1),
        _ => (myos_user::arg(1).unwrap_or(b""), 2),
    };
    let action = match name {
        b"poweroff" | b"off" => POWER_OFF,
        b"reboot" => POWER_REBOOT,
        b"halt" => POWER_HALT,
        _ => usage(),
    };
    if myos_user::argc() != argc {
        usage();
    }
    myos_user::power(action);
    write_fd(2, name);
    write_fd(2, b": not permitted (it needs write on kernel.power)\n");
    myos_user::exit_code(1);
}

fn usage() -> ! {
    write_fd(2, b"usage: poweroff | reboot | halt   (or: power off|reboot|halt)\n");
    myos_user::exit_code(2);
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
