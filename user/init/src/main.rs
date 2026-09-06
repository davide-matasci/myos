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
            exec(b"/bin/custom/ok", &[b"ok"]);
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
            exec(b"/bin/custom/netd", &[b"netd"]);
            write(b"netd exec failed\n");
            exit();
        }
        Some(_) => {}
        None => write(b"netd fork failed\n"),
    }
}

/// Stay PID1: fork getty, wait for *that* child, respawn.
///
/// Other children (notably netd) must not trigger another getty: a second
/// getty on the same `/dev/console` reprints `login: ` on the same line and
/// steals stdin bytes so login is unusable.
fn spawn_getty_loop() -> ! {
    loop {
        match fork() {
            Some(0) => {
                exec(
                    b"/bin/ubase/getty",
                    &[b"getty", b"/dev/console", b"linux"],
                );
                write(b"getty exec failed\n");
                exit();
            }
            Some(getty_pid) => {
                loop {
                    match wait_status() {
                        Some((pid, _)) if pid == getty_pid => break,
                        Some(_) => {}
                        None => break,
                    }
                }
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
