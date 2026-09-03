#![no_std]
#![no_main]

use myos_user::{exec, exit, fork, wait_status, write};

fn smoke_fork_ping() {
    match fork() {
        Some(0) => exit(),
        Some(_) => {
            let _ = wait_status();
            write(b"fork ok\n");
        }
        None => write(b"fork failed\n"),
    }
}

fn smoke_fork_exec_ok() {
    match fork() {
        Some(0) => {
            exec(b"/ok", &[b"ok"]);
            exit();
        }
        Some(_) => {
            let _ = wait_status();
            write(b"fork exec ok\n");
        }
        None => write(b"fork failed\n"),
    }
}

fn spawn_netd() {
    match fork() {
        Some(0) => {
            exec(b"/netd", &[b"netd"]);
            exit();
        }
        Some(_) => {}
        None => write(b"netd fork failed\n"),
    }
}

/// Stay PID1: fork getty, wait, respawn. Getty prompts and execs `/u/login`.
fn spawn_getty_loop() -> ! {
    loop {
        match fork() {
            Some(0) => {
                exec(
                    b"/u/getty",
                    &[b"getty", b"/dev/console", b"linux"],
                );
                write(b"getty exec failed\n");
                exit();
            }
            Some(_) => {
                // Blocking wait; respawn when getty/login/sh exits.
                // Also reaps netd if it exits (do not wait for netd at spawn).
                let _ = wait_status();
            }
            None => {
                write(b"getty fork failed\n");
                exit();
            }
        }
    }
}

fn start() -> ! {
    smoke_fork_ping();
    smoke_fork_exec_ok();
    spawn_netd();
    spawn_getty_loop();
}

#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    start()
}

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_argc: usize, _argv: *const usize) -> ! {
    start()
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
