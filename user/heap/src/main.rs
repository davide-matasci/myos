#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use myos_user::{exec, exit, fork, heap_init, open, read, wait, write, Heap};

#[global_allocator]
static GLOBAL: Heap = Heap;

#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    main()
}

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
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

fn smoke_disk() {
    let Some(fd) = open(b"/disk/ping") else {
        write(b"disk open fail\n");
        return;
    };
    let mut buf = [0u8; 16];
    let n = read(fd, &mut buf);
    if n >= 8 && &buf[..8] == b"disk ok\n" {
        write(b"disk ok\n");
    }
}

fn main() -> ! {
    heap_init();
    let mut v = Vec::new();
    v.extend_from_slice(b"alloc ok\n");
    write(&v);
    smoke_disk();
    run_prog(b"/stdhello", &[]);
    run_prog(b"/stdcat", &[]);
    run_prog(b"/stdecho", &[]);
    run_prog(b"/chello", &[]);
    run_prog(b"/strue", &[]);
    run_prog(b"/secho", &[]);
    run_prog(b"/sls", &[]);
    run_prog(b"/spwd", &[]);
    exit();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
