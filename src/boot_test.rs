//! The boot test (docs/testing.md): boot an image headless, log in, start
//! the guest-side test runner and watch the serial console for its result.
//!
//! The tests themselves live in the image (`/lib/myos-tests`, from
//! `user/tests` and the ports' `PORT_TEST` scripts). The runner prints one
//! line per test, `TEST <name> PASS` or `TEST <name> FAIL`, asks for the
//! host's help with `HOST <port> <args>` (the port's own host-side script,
//! `PORT_HOST`: a peer connecting to a forwarded port, an SSH client), and
//! ends with `TESTS DONE <passed>/<total>`. This side types the login and
//! the one command, runs the requested scripts, bounds the run (an overall
//! budget and a stall watchdog: the console must keep moving), checks the
//! kernel's own boot markers, and exits 0 only when every test passed.

use std::io::{Read, Write};
use std::path::PathBuf;
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
    "[ OK ] tty",
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

/// A host request from the guest, `HOST <port> <args>`: the port's
/// host-side script (`PORT_HOST`, docs/testing.md) runs on the host with the
/// arguments, in the background, its messages on our stderr. The guest test
/// decides the outcome from what the script did (a file it touched, a reply
/// it sent); the exit status only makes the log say when it failed.
fn handle_host_request(req: &str, ports: &[crate::ports::Port]) {
    let mut words = req.split_whitespace();
    let Some(name) = words.next() else {
        eprintln!("boot test: empty host request");
        return;
    };
    let args: Vec<String> = words.map(str::to_string).collect();
    let Some(script) = ports.iter().find(|p| p.name == name).and_then(|p| p.host.clone()) else {
        eprintln!("boot test: host request {req:?}: no port {name:?} with a PORT_HOST script");
        return;
    };
    let req = req.to_string();
    std::thread::spawn(move || {
        let status = Command::new("bash").arg(&script).args(&args).current_dir(repo_root()).status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => eprintln!("boot test: HOST {req}: {script} exited with {s}"),
            Err(e) => eprintln!("boot test: HOST {req}: {script} did not start: {e}"),
        }
    });
}

/// The checkout the launcher was built from: the descriptors and the host
/// scripts are read from it.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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
    // The descriptors: a `HOST <port> ...` line runs that port's host script.
    let ports = crate::ports::load_all(&repo_root());
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
                    handle_host_request(req, &ports);
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
