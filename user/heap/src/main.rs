#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use myos_user::{exec, exit, fork, heap_init, wait, write, Heap};

#[global_allocator]
static GLOBAL: Heap = Heap;

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

fn run_prog(path: &[u8], args: &[&[u8]]) {
    match fork() {
        None => write(b"fork fail\n"),
        Some(0) => exec(path, args),
        Some(_) => {
            let _ = wait();
        }
    }
}

fn main() -> ! {
    heap_init();
    let mut v = Vec::new();
    v.extend_from_slice(b"alloc ok\n");
    write(&v);
    // Empty argv keeps AArch64 exec working (multi-arg pack can fail today).
    run_prog(b"/stdhello", &[]);
    run_prog(b"/stdcat", &[]);
    run_prog(b"/stdecho", &[]);
    exit();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
