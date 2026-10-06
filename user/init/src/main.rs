#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![no_main]

use myos_user::{
    close, exit, fork, open_flags, status_fail, status_ok, wait_status, exec, write_fd, O_WRONLY,
};

/// Default keymap path in the initramfs (libfs nested tree). Switch to US with:
/// `b"/lib/kbd/us.map"`.
const DEFAULT_KEYMAP: &[u8] = b"/lib/kbd/ch.map";
const FALLBACK_KEYMAP: &[u8] = b"/lib/kbd/us.map";

#[derive(Clone, Copy)]
enum KeymapErr {
    /// The console's control file could not be opened.
    Ctl,
    /// The kernel refused the line: no such file, or the map does not parse
    /// (the console module says why).
    Load,
}

/// Load the keyboard map in the file at `path`: `keymap PATH` written to the
/// console's control file (docs/keymap.md); the kernel reads the file and the
/// console module installs it.
fn load_keymap(path: &[u8]) -> Result<(), KeymapErr> {
    let Some(fd) = open_flags(b"/dev/console/ctl", O_WRONLY) else {
        return Err(KeymapErr::Ctl);
    };
    let mut line = [0u8; 64];
    let head = b"keymap ";
    let n = head.len() + path.len() + 1;
    if n > line.len() {
        close(fd);
        return Err(KeymapErr::Load);
    }
    line[..head.len()].copy_from_slice(head);
    line[head.len()..head.len() + path.len()].copy_from_slice(path);
    line[n - 1] = b'\n';
    let ok = write_fd(fd, &line[..n]) == n;
    close(fd);
    if ok {
        Ok(())
    } else {
        Err(KeymapErr::Load)
    }
}

fn fail_keymap(which: &str, err: KeymapErr) {
    // Static labels only (no_std init has no formatting helpers here).
    match (which, err) {
        ("ch", KeymapErr::Ctl) => status_fail("keymap ctl ch"),
        ("ch", KeymapErr::Load) => status_fail("keymap load ch"),
        (_, KeymapErr::Ctl) => status_fail("keymap ctl us"),
        (_, KeymapErr::Load) => status_fail("keymap load us"),
    }
}

/// Prefer Swiss German; fall back to US so the PS/2 keyboard is never bricked.
fn load_default_keymap() {
    match load_keymap(DEFAULT_KEYMAP) {
        Ok(()) => {
            status_ok("keymap ch");
            return;
        }
        Err(e) => fail_keymap("ch", e),
    }
    match load_keymap(FALLBACK_KEYMAP) {
        Ok(()) => status_ok("keymap us"),
        Err(e) => fail_keymap("us", e),
    }
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
                    &[b"getty", b"/dev/console/data", b"linux"],
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
    load_default_keymap();
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
