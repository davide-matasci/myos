#![no_std]
#![no_main]

use myos_user::{close, exit, open, read, write};

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

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    write(b"user ok\n");
    do_msg();
    write(b"fat ok\n");
    exit();
}

fn miss(m: &[u8]) -> ! {
    write(m);
    exit();
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
