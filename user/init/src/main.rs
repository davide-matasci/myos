#![no_std]
#![no_main]

use myos_user::{close, exec, exit, fork, open, read, wait};

const PATH: &[u8] = b"/ok";
const ELF_MAG: [u8; 4] = [0x7f, b'E', b'L', b'F'];

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    match fork() {
        Some(0) => child(),
        Some(_) => {
            let _ = wait();
            exit();
        }
        None => spin(),
    }
}

fn child() -> ! {
    let Some(fd) = open(PATH) else { spin() };
    let mut mag = [0u8; 4];
    let n = read(fd, &mut mag);
    if n != 4 || mag != ELF_MAG {
        spin();
    }
    close(fd);
    exec(PATH);
    spin();
}

fn spin() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
