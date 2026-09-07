#![no_std]
#![no_main]

use myos_user::{
    close, exit, fork, ioctl, open, read, status_fail, status_ok, wait_status, exec,
};

/// Default keymap path in the initramfs (libfs nested tree). Switch to US with:
/// `b"/lib/kbd/us.map"`.
const DEFAULT_KEYMAP: &[u8] = b"/lib/kbd/ch.map";
const FALLBACK_KEYMAP: &[u8] = b"/lib/kbd/us.map";

/// `ioctl` load request — must match `kernel::keymap::KDSKMAP`.
const KDSKMAP: usize = 0x5480;

#[derive(Clone, Copy)]
enum KeymapErr {
    Open,
    Read,
    Ioctl,
}

fn load_keymap(path: &[u8]) -> Result<(), KeymapErr> {
    let Some(kfd) = open(path) else {
        return Err(KeymapErr::Open);
    };
    // Packet: { len: u32 NE, data: [u8; len] } — see docs/keymap.md / KDSKMAP.
    let mut packet = [0u8; 4 + 4096];
    let n = read(kfd, &mut packet[4..]);
    close(kfd);
    if n == 0 || n > 4096 {
        return Err(KeymapErr::Read);
    }
    packet[0..4].copy_from_slice(&(n as u32).to_ne_bytes());
    // Open /dev/console explicitly — do not assume fd 1 is the tty.
    let Some(cfd) = open(b"/dev/console") else {
        return Err(KeymapErr::Ioctl);
    };
    let ok = ioctl(cfd, KDSKMAP, packet.as_ptr() as usize) != usize::MAX;
    close(cfd);
    if ok {
        Ok(())
    } else {
        Err(KeymapErr::Ioctl)
    }
}

fn fail_keymap(which: &str, err: KeymapErr) {
    // Static labels only (no_std init has no formatting helpers here).
    match (which, err) {
        ("ch", KeymapErr::Open) => status_fail("keymap open ch"),
        ("ch", KeymapErr::Read) => status_fail("keymap read ch"),
        ("ch", KeymapErr::Ioctl) => status_fail("keymap ioctl ch"),
        (_, KeymapErr::Open) => status_fail("keymap open us"),
        (_, KeymapErr::Read) => status_fail("keymap read us"),
        (_, KeymapErr::Ioctl) => status_fail("keymap ioctl us"),
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
