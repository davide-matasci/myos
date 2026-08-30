#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use myos_user::{
    exec, exit, exit_code, fork, heap_init, listdir, open, read, wait_status, write, Heap,
};

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
        Some(0) => {
            exec(path, args);
            exit_code(127);
        }
        Some(_) => {
            let _ = wait_status();
        }
    }
}

fn run_prog_exit(path: &[u8], args: &[&[u8]], expect: u8, ok_msg: &[u8]) {
    match fork() {
        None => write(b"fork fail\n"),
        Some(0) => {
            exec(path, args);
            exit_code(127);
        }
        Some(_) => {
            if let Some((_, status)) = wait_status() {
                if status == expect {
                    write(ok_msg);
                }
            }
        }
    }
}

fn buf_has(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn smoke_vfs() {
    let mut buf = [0u8; 512];
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
    if n >= 8 && &buf[..8] == b"disk ok\n" {
        write(b"disk ok\n");
    }
}

fn main() -> ! {
    heap_init();
    let mut v = Vec::new();
    v.extend_from_slice(b"alloc ok\n");
    write(&v);
    #[cfg(not(target_arch = "riscv64"))]
    {
        run_prog_exit(b"/uutils-echo", &[], 0, b"uutils echo ok\n");
        run_prog_exit(b"/uutils-true", &[], 0, b"uutils true ok\n");
        run_prog_exit(b"/uutils-false", &[], 1, b"uutils false ok\n");
    }
    smoke_disk();
    smoke_vfs();
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
