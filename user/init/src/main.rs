#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![no_main]

use myos_user::{
    close, exit, fork, open_flags, status_fail, status_ok, status_warn, wait_status, exec, write_fd, O_WRONLY,
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

/// `mount -a` (docs/install.md): the boot disk's ESP at `/boot`, then what
/// its `fstab` (`/boot/fstab`) lists (the data partition at `/data`). Waited for: the
/// programs started next may want those. A line that failed is reported by
/// mount and does not stop the boot.
fn mount_all() {
    match fork() {
        Some(0) => {
            exec(b"/bin/custom/mount", &[b"mount", b"-a"]);
            status_fail("mount exec failed");
            exit();
        }
        Some(pid) => loop {
            match wait_status() {
                Some((p, 0)) if p == pid => break status_ok("mount -a"),
                Some((p, _)) if p == pid => break status_warn("mount -a: not everything mounted"),
                Some(_) => {}
                None => break,
            }
        },
        None => status_fail("mount fork failed"),
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

/// The sound server (`docs/sound.md`): the sound cards' streams, for the
/// programs that play and record through its socket.
fn spawn_sndd() {
    match fork() {
        Some(0) => {
            exec(b"/bin/custom/sndd", &[b"sndd"]);
            status_fail("sndd exec failed");
            exit();
        }
        Some(_) => {}
        None => status_fail("sndd fork failed"),
    }
}

/// The boot script (`/etc/rc`, docs/install.md), not waited for: the login
/// prompt comes at once, and the script's last line says when it is over.
/// Its `exec` rule makes it root (etc/policy).
fn spawn_rc() {
    match fork() {
        Some(0) => {
            exec(b"/etc/rc", &[b"/etc/rc"]);
            status_fail("rc exec failed");
            exit();
        }
        Some(_) => status_ok("rc started (in the background)"),
        None => status_fail("rc fork failed"),
    }
}

/// The ttys with a login: `/data/etc/ttys` (the administrator's), else the
/// image's `/etc/ttys` (`docs/tty.md`): their number, 0 to 9, on a line of
/// its own (`#` lines are comments). 1 when neither says.
fn tty_count() -> usize {
    for path in [&b"/data/etc/ttys"[..], b"/etc/ttys"] {
        let Some(fd) = myos_user::open(path) else {
            continue;
        };
        let mut buf = [0u8; 512];
        let n = myos_user::read(fd, &mut buf);
        close(fd);
        if n == usize::MAX {
            continue;
        }
        let line = buf[..n].split(|&b| b == b'\n').map(|l| l.trim_ascii()).find(|l| !l.is_empty() && l[0] != b'#');
        match line {
            Some(&[d]) if d.is_ascii_digit() => return usize::from(d - b'0'),
            _ => status_warn("ttys: not a number from 0 to 9"),
        }
    }
    1
}

/// The terminals with a getty: `ttyS0` (the serial port), then `tty1`…
/// `ttyN`, as `/dev/<name>/data`.
const TTYS: [&[u8]; 10] = [
    b"/dev/ttyS0/data",
    b"/dev/tty1/data",
    b"/dev/tty2/data",
    b"/dev/tty3/data",
    b"/dev/tty4/data",
    b"/dev/tty5/data",
    b"/dev/tty6/data",
    b"/dev/tty7/data",
    b"/dev/tty8/data",
    b"/dev/tty9/data",
];

/// A getty on `tty`; its pid.
fn spawn_getty(tty: &[u8]) -> Option<usize> {
    match fork() {
        Some(0) => {
            exec(b"/bin/ubase/getty", &[b"getty", tty, b"linux"]);
            status_fail("getty exec failed");
            exit();
        }
        Some(pid) => Some(pid),
        None => {
            status_fail("getty fork failed");
            None
        }
    }
}

/// Stay PID1: a getty on each terminal, each started again when it exits
/// (a logout). Other children (netd, the boot script) exit without one.
/// The screen goes from the console's to tty1's, whose login is the local
/// one; Alt+0 shows the console again.
fn spawn_getty_loop() -> ! {
    let ttys = &TTYS[..1 + tty_count()];
    let mut pids = [None; TTYS.len()];
    for (i, tty) in ttys.iter().enumerate() {
        pids[i] = spawn_getty(tty);
    }
    if ttys.len() > 1 {
        if let Some(fd) = open_flags(b"/dev/console/ctl", O_WRONLY) {
            write_fd(fd, b"screen 1\n");
            close(fd);
        }
    }
    loop {
        if let Some((pid, _)) = wait_status() {
            if let Some(i) = pids.iter().position(|&p| p == Some(pid)) {
                pids[i] = None;
            }
        }
        // The one that exited, and any whose start failed.
        for (i, tty) in ttys.iter().enumerate() {
            if pids[i].is_none() {
                pids[i] = spawn_getty(tty);
            }
        }
    }
}

fn start() -> ! {
    load_default_keymap();
    smoke_fork_ping();
    smoke_fork_exec_ok();
    mount_all();
    spawn_netd();
    spawn_sndd();
    spawn_rc();
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
