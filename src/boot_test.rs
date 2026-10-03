//! The boot test (docs/testing.md): boot an image headless, log in, start
//! the guest-side test runner and watch the serial console for its result.
//!
//! The tests themselves live in the image (`/lib/myos-tests`, from
//! `user/tests` and the ports' `PORT_TEST` scripts). The runner prints one
//! line per test, `TEST <name> PASS` or `TEST <name> FAIL`, asks for the
//! host's help with `HOST <what> <args>` (a connection to a forwarded port),
//! and ends with `TESTS DONE <passed>/<total>`. This side types the login
//! and the one command, performs the requests, bounds the run (an overall
//! budget and a stall watchdog: the console must keep moving), checks the
//! kernel's own boot markers, and exits 0 only when every test passed.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Which test list the guest runs: `test-mini` is what every pull request
/// boots (no network beyond QEMU's own, a few minutes), `test-full` adds the
/// packages, HTTPS, SSH, the Linux layer's Alpine packages and the curated
/// os-test list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Mini,
    Full,
}

impl Mode {
    pub fn parse(arg: &str) -> Option<Mode> {
        match arg {
            "test-mini" => Some(Mode::Mini),
            "test-full" => Some(Mode::Full),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Mode::Mini => "mini",
            Mode::Full => "full",
        }
    }

    /// The whole run's budget. The full list runs the curated os-test set
    /// (300 tests at a few seconds each under TCG) after installing the
    /// packages; the mini list must fail well inside the 5-minute CI job.
    pub fn budget(self, linux_compat: bool) -> Duration {
        match self {
            Mode::Mini => Duration::from_secs(if linux_compat { 300 } else { 240 }),
            Mode::Full => Duration::from_secs(3000),
        }
    }

    /// How long the console may stay silent. The long full-mode steps
    /// (installing the packages, an Alpine package's unpack) print little.
    fn stall(self) -> Duration {
        match self {
            Mode::Mini => Duration::from_secs(180),
            Mode::Full => Duration::from_secs(600),
        }
    }
}

/// What the kernel and init print before the login prompt: the boot's own
/// self-checks (memory, interrupts, the scheduler, the drivers, the VFS
/// through `/bin/custom/ok`). Required in every mode; a missing one fails
/// the run even when every test passed.
const BOOT_MARKERS: [&str; 33] = [
    "Hello from myos",
    "[ OK ] heap",
    "[ OK ] interrupts",
    "task a",
    "task b",
    "[ OK ] scheduler",
    "[ OK ] urandom",
    "[ OK ] hello",
    "[ OK ] stubfs",
    "[ OK ] limine module",
    "[ OK ] sh",
    "[ OK ] fork",
    "[ OK ] fork exec",
    "[ OK ] alloc",
    "[ OK ] user",
    "[ OK ] msg",
    "[ OK ] fat",
    "[ OK ] vda",
    "[ OK ] net0",
    "[ OK ] netmac",
    "[ OK ] nvme",
    "[ OK ] ext2",
    "[ OK ] ext2 rw",
    "[ OK ] disk",
    "[ OK ] disk ls",
    "[ OK ] fat ls",
    "[ OK ] fat read",
    "[ OK ] devnull",
    "[ OK ] tmp",
    "[ OK ] tmpops",
    "[ OK ] proc",
    "[ OK ] ioctl",
    "[ OK ] signal",
];

/// The kernel reporting a crash: a CPU exception, a user fault it printed,
/// a panic. Ends the run at once instead of waiting out the budget.
const FATAL_MARKERS: [&str; 3] = ["exception:", "user panic", "[ WARN ] user fault"];

/// Login never takes this long once init is up (OVMF alone can take 40 s
/// before that, so the clock starts at `[ OK ] fork exec`).
const LOGIN_BOUND: Duration = Duration::from_secs(60);
/// Pause between typed bytes, and the echo-sync wait per byte.
const TYPE_DELAY: Duration = Duration::from_millis(40);
const ECHO_WAIT: Duration = Duration::from_secs(5);

/// Everything QEMU wrote to the serial console so far (CR normalized to LF)
/// and when it last grew.
struct Serial {
    text: String,
    last_change: Instant,
}

type Shared = Arc<Mutex<Serial>>;

fn snapshot(serial: &Shared) -> String {
    serial.lock().unwrap().text.clone()
}

fn tail(text: &str, n: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let from = chars.len().saturating_sub(n);
    chars[from..].iter().collect()
}

/// The console after init handed over to getty.
fn after_boot(text: &str) -> &str {
    text.rsplit_once("[ OK ] fork exec").map(|(_, t)| t).unwrap_or(text)
}

fn at_prompt(text: &str) -> bool {
    after_boot(text).trim_end().ends_with('$')
}

/// Bytes echoed on the current line after `prompt` (its last occurrence).
fn typed_after<'a>(text: &'a str, prompt: &str) -> &'a str {
    match after_boot(text).rsplit_once(prompt) {
        Some((_, rest)) => rest.lines().next().unwrap_or("").trim_end_matches('\r'),
        None => "",
    }
}

fn send(stdin: &mut ChildStdin, bytes: &[u8]) {
    stdin.write_all(bytes).expect("write to qemu stdin");
    stdin.flush().ok();
}

/// Type `line` (without its newline) one byte at a time, each only once the
/// echo of the previous ones is on the console: the guest's UART drain
/// preserves bytes but an AP's echo can lag, and typing ahead or resending
/// garbles the line. `prompt` is what the current input line follows.
/// Returns false when the echo stopped matching (garble) or stalled.
fn type_synced(stdin: &mut ChildStdin, serial: &Shared, prompt: &str, line: &str) -> bool {
    let bytes = line.as_bytes();
    for i in 0..bytes.len() {
        send(stdin, &bytes[i..=i]);
        let want = &line[..=i];
        let start = Instant::now();
        loop {
            let got = typed_after(&snapshot(serial), prompt).to_string();
            if got == want {
                break;
            }
            if !want.starts_with(&got) || start.elapsed() > ECHO_WAIT {
                eprintln!("boot test: typing {line:?}: echo {got:?} after {want:?}");
                return false;
            }
            std::thread::sleep(TYPE_DELAY);
        }
        std::thread::sleep(TYPE_DELAY);
    }
    send(stdin, b"\n");
    true
}

/// Wait until `ready(console)` holds, false on the deadline.
fn wait_for(serial: &Shared, deadline: Instant, ready: impl Fn(&str) -> bool) -> bool {
    loop {
        if ready(&snapshot(serial)) {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A host request from the guest (`HOST <what> <args>`).
fn handle_host_request(req: &str) {
    let mut words = req.split_whitespace();
    match (words.next(), words.next()) {
        (Some("tcp-ping"), Some(port)) => {
            let port: u16 = port.parse().unwrap_or(2323);
            std::thread::spawn(move || tcp_ping(port));
        }
        (Some("ssh"), Some(port)) => {
            let port = port.to_string();
            std::thread::spawn(move || ssh_two_clients(&port));
        }
        _ => eprintln!("boot test: unknown host request {req:?}"),
    }
}

/// The listen/accept smoke's peer: connect to the guest's listener through
/// QEMU's port forward, send "ping", expect "pong". Retried until the
/// listener is up; the guest test bounds its own wait.
fn tcp_ping(port: u16) {
    use std::net::{SocketAddr, TcpStream};
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline {
        let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_secs(5)) else {
            std::thread::sleep(Duration::from_millis(250));
            continue;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
        if stream.write_all(b"ping").is_err() {
            continue;
        }
        let mut buf = [0u8; 8];
        let mut got = 0usize;
        while got < 5 {
            match stream.read(&mut buf[got..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => got += n,
            }
        }
        if got >= 5 && &buf[..5] == b"pong\n" {
            eprintln!("boot test: tcp-ping {port}: pong");
            return;
        }
    }
    eprintln!("boot test: tcp-ping {port}: no pong within the bound");
}

fn dropbear_testkey_src() -> PathBuf {
    let from_manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ports/dropbear/testkey");
    if from_manifest.is_file() {
        return from_manifest;
    }
    PathBuf::from("ports/dropbear/testkey")
}

/// OpenSSH refuses world-readable private keys: a 0600 copy of the
/// committed test key.
fn prepare_dropbear_testkey() -> Result<PathBuf, String> {
    let src = dropbear_testkey_src();
    if !src.is_file() {
        return Err(format!("missing dropbear test key at {}", src.display()));
    }
    let dir = std::env::temp_dir().join("myos-dropbear-ssh-smoke");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let dst = dir.join("testkey");
    std::fs::copy(&src, &dst).map_err(|e| format!("copy testkey: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod testkey: {e}"))?;
    }
    Ok(dst)
}

/// An `ssh` client on the host: the CI image ships openssh-client; fall
/// back to apt-get when running as root on an older image.
fn ensure_host_ssh() -> Result<(), String> {
    if Command::new("ssh").arg("-V").output().is_ok() {
        return Ok(());
    }
    let ok = Command::new("apt-get").args(["update", "-qq"]).status().map_or(false, |s| s.success())
        && Command::new("apt-get")
            .args(["install", "-y", "-qq", "--no-install-recommends", "openssh-client"])
            .status()
            .map_or(false, |s| s.success());
    if !ok || Command::new("ssh").arg("-V").output().is_err() {
        return Err("no ssh client and apt-get install openssh-client failed".into());
    }
    Ok(())
}

/// One SSH session into the guest: the remote command echoes `tag` and
/// touches `/tmp/ssh-ok-<tag>`, which the guest test waits for.
fn ssh_one_client(key: &Path, port: &str, tag: &str) -> Result<(), String> {
    let remote = format!("echo {tag}; : > /tmp/ssh-ok-{tag}");
    // `timeout` so a hung key exchange cannot burn the whole bound
    // (ConnectTimeout covers the TCP connect only).
    let output = Command::new("timeout")
        .args([
            "20",
            "ssh",
            "-4",
            "-i",
            key.to_str().ok_or("testkey path not utf-8")?,
            "-p",
            port,
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "GlobalKnownHostsFile=/dev/null",
            "-o",
            "BatchMode=yes",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "PreferredAuthentications=publickey",
            // Pin the KEX to curve25519: OpenSSH 10 negotiates post-quantum
            // KEX first, which hits a dropbear interop bug on aarch64.
            "-o",
            "KexAlgorithms=curve25519-sha256",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "ConnectionAttempts=1",
            "root@127.0.0.1",
            &remote,
        ])
        .output()
        .map_err(|e| format!("spawn timeout/ssh ({tag}): {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(format!("ssh {tag} failed (exit {:?}): {stderr}", output.status.code()));
    }
    if !stdout.contains(tag) {
        return Err(format!("ssh {tag}: remote echo missing (stdout={stdout:?})"));
    }
    Ok(())
}

/// Two SSH sessions at once (pubkey auth), both must exit 0: multi-session
/// accept on dropbear and netd (a parked accept plus the listen hold). B
/// starts a second after A so both are live together while the second SYN
/// usually meets a re-armed listener.
fn ssh_two_clients(port: &str) {
    let key = match ensure_host_ssh().and_then(|_| prepare_dropbear_testkey()) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("boot test: ssh: {e}");
            return;
        }
    };
    // Early SYNs before dropbear has armed accept leave half-open sessions
    // on slow arches; give it time, and never probe the port.
    std::thread::sleep(Duration::from_secs(12));
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut last_err = String::new();
    while Instant::now() < deadline {
        let (ka, kb, pa, pb) = (key.clone(), key.clone(), port.to_string(), port.to_string());
        let a = std::thread::spawn(move || ssh_one_client(&ka, &pa, "a"));
        std::thread::sleep(Duration::from_secs(1));
        let b = std::thread::spawn(move || ssh_one_client(&kb, &pb, "b"));
        let ra = a.join().unwrap_or_else(|_| Err("ssh a panicked".into()));
        let rb = b.join().unwrap_or_else(|_| Err("ssh b panicked".into()));
        match (ra, rb) {
            (Ok(()), Ok(())) => {
                eprintln!("boot test: ssh {port}: two sessions ok");
                return;
            }
            (Err(e), _) | (_, Err(e)) => last_err = e,
        }
        // Failed attempts can leave SynReceived orphans until the
        // handshake-age reclaim (~10 s); flooding starved riscv64.
        std::thread::sleep(Duration::from_secs(5));
    }
    eprintln!("boot test: ssh {port}: gave up ({last_err})");
}

/// One `TEST` line of the runner.
#[derive(Debug, PartialEq, Eq)]
struct TestLine {
    name: String,
    passed: bool,
}

fn parse_test_line(line: &str) -> Option<TestLine> {
    let rest = line.strip_prefix("TEST ")?;
    let mut words = rest.split_whitespace();
    let name = words.next()?.to_string();
    let passed = match words.next()? {
        "PASS" => true,
        "FAIL" => false,
        _ => return None,
    };
    Some(TestLine { name, passed })
}

/// `TESTS DONE <passed>/<total>`.
fn parse_done_line(line: &str) -> Option<(u32, u32)> {
    let rest = line.strip_prefix("TESTS DONE ")?.trim();
    let (p, t) = rest.split_once('/')?;
    Some((p.trim().parse().ok()?, t.trim().parse().ok()?))
}

fn fail(serial: &Shared, child: &mut Child, why: &str) -> ! {
    let _ = child.kill();
    let _ = child.wait();
    let text = snapshot(serial);
    eprintln!("error: boot test: {why}");
    eprintln!("--- last serial output ---\n{}\n---", tail(&text, 3000));
    std::process::exit(1);
}

/// Boot test on a QEMU `child` started with `-serial stdio` and piped
/// stdio. Does not return: exits the process with the result.
pub fn run(mut child: Child, mode: Mode, linux_compat: bool) -> ! {
    let mut stdin = child.stdin.take().expect("qemu stdin");
    let mut stderr = child.stderr.take().expect("qemu stderr");
    let mut stdout = child.stdout.take().expect("qemu stdout");
    std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        while let Ok(n) = stderr.read(&mut buf) {
            if n == 0 {
                break;
            }
            eprint!("{}", String::from_utf8_lossy(&buf[..n]));
        }
    });
    let serial: Shared = Arc::new(Mutex::new(Serial { text: String::new(), last_change: Instant::now() }));
    let reader = serial.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        // A CR at the end of a chunk waits for the next one: splitting a
        // CRLF would turn it into two newlines.
        let mut pending_cr = false;
        while let Ok(n) = stdout.read(&mut buf) {
            if n == 0 {
                break;
            }
            let chunk = String::from_utf8_lossy(&buf[..n]);
            eprint!("{chunk}");
            let mut raw = String::new();
            if pending_cr {
                raw.push('\r');
                pending_cr = false;
            }
            raw.push_str(&chunk);
            if raw.ends_with('\r') {
                pending_cr = true;
                raw.pop();
            }
            let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
            let mut s = reader.lock().unwrap();
            s.text.push_str(&normalized);
            s.last_change = Instant::now();
        }
    });

    let started = Instant::now();
    let budget = mode.budget(linux_compat);
    let deadline = started + budget;

    // Log in: `root`, an empty password, then the one command. Each line is
    // typed in sync with its echo; a garbled line is abandoned and typed
    // again at the next prompt.
    let login_ready = |t: &str| {
        t.contains("[ OK ] fork exec") && after_boot(t).contains("login: ") && typed_after(t, "login: ").is_empty()
    };
    if !wait_for(&serial, deadline, login_ready) {
        fail(&serial, &mut child, "no login prompt within the budget");
    }
    let login_deadline = Instant::now() + LOGIN_BOUND;
    let mut attempts = 0;
    loop {
        attempts += 1;
        if attempts > 3 {
            fail(&serial, &mut child, "login line kept garbling");
        }
        if !wait_for(&serial, login_deadline, login_ready) {
            fail(&serial, &mut child, "login prompt did not come back");
        }
        if type_synced(&mut stdin, &serial, "login: ", "root") {
            break;
        }
        send(&mut stdin, b"\n");
    }
    if !wait_for(&serial, login_deadline, |t| after_boot(t).contains("Password:")) {
        fail(&serial, &mut child, "no password prompt after `root`");
    }
    send(&mut stdin, b"\n");
    let command = format!("sh /lib/myos-tests/run.sh {}", mode.name());
    let mut attempts = 0;
    loop {
        attempts += 1;
        if attempts > 3 {
            fail(&serial, &mut child, "command line kept garbling");
        }
        if !wait_for(&serial, login_deadline, |t| at_prompt(t) && typed_after(t, "$ ").is_empty()) {
            fail(&serial, &mut child, "no shell prompt after login");
        }
        if type_synced(&mut stdin, &serial, "$ ", &command) {
            break;
        }
        send(&mut stdin, b"\n");
    }
    let run_from = snapshot(&serial).len();

    // Watch the runner: its result lines, its requests, the kernel's
    // crash reports, the stall watchdog and the budget.
    let mut scanned = run_from;
    let mut results: Vec<TestLine> = Vec::new();
    let mut done: Option<(u32, u32)> = None;
    let stall = mode.stall();
    while done.is_none() {
        let (text, last_change) = {
            let s = serial.lock().unwrap();
            (s.text.clone(), s.last_change)
        };
        if let Some(end) = text[scanned..].rfind('\n') {
            for line in text[scanned..scanned + end].lines() {
                let line = line.trim_end();
                if let Some(t) = parse_test_line(line) {
                    results.push(t);
                } else if let Some(req) = line.strip_prefix("HOST ") {
                    handle_host_request(req);
                } else if let Some(counts) = parse_done_line(line) {
                    done = Some(counts);
                } else if FATAL_MARKERS.iter().any(|m| line.contains(m)) {
                    fail(&serial, &mut child, &format!("the kernel reported a crash: {line}"));
                }
            }
            scanned += end + 1;
        }
        if done.is_some() {
            break;
        }
        if let Some(status) = child.try_wait().expect("wait on qemu") {
            fail(&serial, &mut child, &format!("QEMU exited ({status}) before the tests were done"));
        }
        if last_change.elapsed() > stall {
            fail(&serial, &mut child, &format!("no console output for {stall:?} (stall)"));
        }
        if Instant::now() > deadline {
            fail(&serial, &mut child, &format!("the {budget:?} budget ran out"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    // The ext2 tests leave a filesystem on the scratch disk: e2fsprogs must
    // find it clean (`src/main.rs`).
    let disk_ok = crate::fsck_scratch_disk();

    let text = snapshot(&serial);
    let missing: Vec<&str> = BOOT_MARKERS.iter().copied().filter(|m| !text.contains(m)).collect();
    let failed: Vec<&str> = results.iter().filter(|r| !r.passed).map(|r| r.name.as_str()).collect();
    let (passed, total) = done.unwrap_or((0, 0));
    eprintln!();
    eprintln!(
        "boot test ({}): {passed}/{total} passed in {:?}{}",
        mode.name(),
        started.elapsed(),
        if failed.is_empty() { String::new() } else { format!(", failed: {}", failed.join(" ")) }
    );
    for m in &missing {
        eprintln!("error: boot marker missing: {m:?}");
    }
    if results.len() as u32 != total {
        eprintln!("error: the runner reported {total} tests, {} TEST lines were seen", results.len());
    }
    if !failed.is_empty() || !missing.is_empty() || passed != total || total == 0 || results.len() as u32 != total || !disk_ok {
        std::process::exit(1);
    }
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_lines() {
        assert_eq!(
            parse_test_line("TEST shell_echo PASS"),
            Some(TestLine { name: "shell_echo".into(), passed: true })
        );
        assert_eq!(
            parse_test_line("TEST heap FAIL (exit 1)"),
            Some(TestLine { name: "heap".into(), passed: false })
        );
        assert_eq!(parse_test_line("$ TEST x PASS"), None);
        assert_eq!(parse_done_line("TESTS DONE 17/18"), Some((17, 18)));
        assert_eq!(parse_done_line("TESTS DONE"), None);
    }

    #[test]
    fn prompt_and_echo() {
        let t = "[ OK ] fork exec\nlogin: ro";
        assert_eq!(typed_after(t, "login: "), "ro");
        let t = "[ OK ] fork exec\nlogin: root\nPassword: \n$ sh /lib";
        assert_eq!(typed_after(t, "$ "), "sh /lib");
        assert!(!at_prompt(t));
        assert!(at_prompt("[ OK ] fork exec\n$ "));
    }
}
