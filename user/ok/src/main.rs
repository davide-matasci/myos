#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use myos_user::{close, exit, heap_init, listdir, open, read, write, Heap};

#[global_allocator]
static GLOBAL: Heap = Heap;

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

fn buf_has(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn smoke_vfs() {
    let mut buf = [0u8; myos_user::LISTDIR_BUF];
    let n = listdir(b"/disk", &mut buf);
    if n != usize::MAX && n > 0 && buf_has(&buf[..n], b"ping") {
        write(b"disk ls ok\n");
    }
    let n = listdir(b"/fat", &mut buf);
    if n != usize::MAX && n > 0 && buf_has(&buf[..n], b"msg") {
        write(b"fat ls ok\n");
    }
    let Some(fd) = open(b"/fat/msg") else {
        write(b"fat open fail\n");
        return;
    };
    let mut msg = [0u8; 8];
    let nr = read(fd, &mut msg);
    close(fd);
    if nr >= 7 && &msg[..7] == b"fat ok\n" {
        write(b"fat read ok\n");
    }
}

fn smoke_disk() {
    let Some(fd) = open(b"/disk/ping") else {
        write(b"disk open fail\n");
        return;
    };
    let mut buf = [0u8; 16];
    let n = read(fd, &mut buf);
    close(fd);
    if n >= 8 && &buf[..8] == b"disk ok\n" {
        write(b"disk ok\n");
    }
}

fn main() -> ! {
    heap_init();
    let mut v = Vec::new();
    v.extend_from_slice(b"alloc ok\n");
    write(&v);

    write(b"user ok\n");
    do_msg();
    write(b"fat ok\n");

    smoke_disk();
    smoke_vfs();
    exit();
}

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

fn miss(m: &[u8]) -> ! {
    write(m);
    exit();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
