#![no_std]
#![no_main]

use myos_user::{close, exit, mount, open, read, write, write_fd};

myos_user::x86_start!(main);

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: usize, argv: *const usize) -> ! {
    unsafe { myos_user::args::init_from_regs(argc, argv) };
    main()
}

fn main() -> ! {
    match myos_user::argc() {
        1 => print_mounts(),
        4 => do_mount(),
        _ => {
            write(b"usage: mount [<source> <target> <fstype>]\n");
            myos_user::exit_code(1);
        }
    }
}

fn print_mounts() -> ! {
    let Some(fd) = open(b"/proc/mounts") else {
        write(b"mount: open /proc/mounts failed\n");
        myos_user::exit_code(1);
    };
    let mut buf = [0u8; 256];
    loop {
        let n = read(fd, &mut buf);
        if n == usize::MAX {
            write(b"mount: read /proc/mounts failed\n");
            close(fd);
            myos_user::exit_code(1);
        }
        if n == 0 {
            break;
        }
        write_fd(1, &buf[..n]);
    }
    close(fd);
    exit();
}

fn do_mount() -> ! {
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
