#![no_std]
#![no_main]

use myos_user::{
    close, exit, fork, ioctl, open, read, status_fail, status_ok, wait_status, exec,
};

/// Default keymap path in the initramfs. Switch to US with:
/// `b"/etc/kbd/us.map"`.
const DEFAULT_KEYMAP: &[u8] = b"/etc/kbd/ch.map";

/// `ioctl` load request — must match `kernel::keymap::KDSKMAP`.
const KDSKMAP: usize = 0x5480;

fn load_keymap(path: &[u8]) -> bool {
    let Some(kfd) = open(path) else {
        return false;
    };
    // Packet: { len: u32 NE, data: [u8; len] } — see docs/keymap.md / KDSKMAP.
    let mut packet = [0u8; 4 + 4096];
    let n = read(kfd, &mut packet[4..]);
    close(kfd);
    if n == 0 || n > 4096 {
        return false;
    }
    packet[0..4].copy_from_slice(&(n as u32).to_ne_bytes());
    // fd 1 is the console tty (FdEntry::Console) from PID1 setup.
    ioctl(1, KDSKMAP, packet.as_ptr() as usize) != usize::MAX
}

fn smoke_fork_ping() {
    match fork() {
        Some(0) => exit(),
        Some(_) => {
            let _ = wait_status();
            status_ok("fork");
        }
        None => status_fail("fork failed"),
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
            status_ok("fork exec");
        }
        None => status_fail("fork failed"),
    }
}

fn spawn_netd() {
    match fork() {
        Some(0) => {
            exec(b"/bin/custom/netd", &[b"netd"]);
            status_fail("netd exec failed");
            exit();
        }
        Some(_) => {}
        None => status_fail("netd fork failed"),
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
                status_fail("getty exec failed");
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
                status_fail("getty fork failed");
                exit();
            }
        }
    }
}

fn start() -> ! {
    if load_keymap(DEFAULT_KEYMAP) {
        status_ok("keymap ch");
    } else {
        status_fail("keymap ch");
    }
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
