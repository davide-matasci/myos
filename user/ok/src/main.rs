#![no_std]
#![no_main]

use myos_user::{close, exec, exit, open, read, write};

#[inline(never)]
fn do_msg() {
    let Some(fd) = open(b"/msg") else {
        miss(b"fat nofd\n");
    };
    let mut buf = [0u8; 8];
    let n = read(fd, &mut buf);
    close(fd);
    if n == usize::MAX {
        miss(b"fat nread\n");
    }
    if n == 0 {
        miss(b"fat empty\n");
    }
    write(&buf[..n]);
}

fn main() -> ! {
    write(b"user ok\n");
    do_msg();
    write(b"fat ok\n");
    exec(b"/heap", &[]);
    exit();
}

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

fn miss(m: &[u8]) -> ! {
    write(m);
    exit();
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
