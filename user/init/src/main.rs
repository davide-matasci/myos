#![no_std]
#![no_main]

#[cfg(target_arch = "x86_64")]
use myos_user::{close, exec, exit, fork, open, read, wait};

#[cfg(target_arch = "aarch64")]
use myos_user::{close, exec, open, read};

const PATH: &[u8] = b"/ok";
const ELF_MAG: [u8; 4] = [0x7f, b'E', b'L', b'F'];

#[cfg(target_arch = "x86_64")]
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

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    // AArch64 fork-child re-enter via `eret` still aborts after an EL0
    // round-trip (CI #121). Keep the pre-fork PID1 path here until that
    // resume path is fixed; x86 uses fork+wait.
    child()
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
