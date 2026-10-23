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
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Which tests the guest runs (docs/testing.md): every test carries tags,
/// what it needs (`net`: the host's network and the package mirror, `pkg`:
/// the packages installed as apps, `heavy`: minutes long, `install`: the
/// boot disk upgraded or installed, checked by the boots that follow,
/// `alpine-disk`: the Alpine roots on the disk the host prepares) and
/// what it covers (a module's or a port's name), and its own name is one
/// too. The runner takes the tests with any of `tags` (all of them with
/// none), minus those with any of `skip`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Selection {
    pub tags: Vec<String>,
    pub skip: Vec<String>,
    /// `--budget MINUTES`: the whole run's, instead of the one the tags
    /// imply.
    pub budget: Option<Duration>,
}

/// What the quick list of old skips: `test-mini` is `test --skip` this.
const MINI_SKIPS: &str = "net,pkg,heavy,install,alpine-disk";

impl Selection {
    /// The `test` command's options from the launcher's arguments:
    /// `--tags TAG,...`, `--skip TAG,...`, `--budget MINUTES`;
    /// `test-mini` and `test-full` stand for the two lists of
    /// old. `None` without a test command.
    pub fn parse(args: &[String]) -> Option<Selection> {
        let mut sel = Selection::default();
        let mut found = false;
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let list = |s: Option<&String>| -> Vec<String> {
                s.map(|s| s.split(',').filter(|t| !t.is_empty()).map(str::to_string).collect()).unwrap_or_default()
            };
            match arg.as_str() {
                "test" | "test-full" => found = true,
                "test-mini" => {
                    found = true;
                    sel.skip = list(Some(&MINI_SKIPS.to_string()));
                }
                "--tags" => sel.tags = list(it.next()),
                "--skip" => sel.skip = list(it.next()),
                "--budget" => {
                    let minutes: u64 = it.next().and_then(|m| m.parse().ok()).unwrap_or_else(|| {
                        eprintln!("error: --budget takes the minutes");
                        std::process::exit(2);
                    });
                    sel.budget = Some(Duration::from_secs(minutes * 60));
                }
                _ => {}
            }
        }
        found.then_some(sel)
    }

    fn skips(&self, tag: &str) -> bool {
        self.skip.iter().any(|t| t == tag)
    }

    /// Whether a test needing the host's network may run: the package
    /// mirror is served then.
    pub fn needs_network(&self) -> bool {
        !self.skips("net")
    }

    /// Whether a test needing the Alpine disk may run: it is prepared
    /// (fetched once, minutes) and attached then.
    pub fn needs_alpine_disk(&self) -> bool {
        !self.skips("alpine-disk")
    }

    /// The runner's arguments.
    fn args(&self) -> String {
        let mut s = String::new();
        for (opt, list) in [("--tags", &self.tags), ("--skip", &self.skip)] {
            if !list.is_empty() {
                s.push_str(&format!(" {opt} {}", list.join(",")));
            }
        }
        s
    }

    /// Whether the run is a quick one: nothing heavy, nothing from the
    /// network (the budgets of the mini list of old).
    fn quick(&self) -> bool {
        self.skips("heavy") && self.skips("net")
    }
}

/// A boot of the test harness: the tests, or one of the boots that follow
/// a run that changed the boot disk.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Mode {
    Tests(Selection),
    /// The second boot, of the disk the first one upgraded: it comes up
    /// from the other slot (`run.sh reboot`, docs/install.md), with the app
    /// the first boot's `apps_data` installed on the data partition when
    /// that test ran.
    Reboot { app: bool },
    /// The third, of the disk `get-myos --install` made on the scratch
    /// disk: it comes up from slot a (`run.sh installed`).
    Installed,
}

impl Mode {
    /// The runner's arguments, after `sh /lib/myos-tests/run.sh`.
    fn args(&self) -> String {
        match self {
            Mode::Tests(sel) => sel.args(),
            Mode::Reboot { app: true } => " reboot".into(),
            Mode::Reboot { app: false } => " reboot --skip app_kept".into(),
            Mode::Installed => " installed".into(),
        }
    }

    /// The whole run's budget. The full list runs the curated os-test set
    /// (1000 tests at a few hundred milliseconds each under TCG) after
    /// installing the packages; a quick list must fail well inside the
    /// 5-minute CI job.
    /// riscv64 (`slow`), the slowest under TCG, gets half as much again,
    /// as in CI (`scripts/ci-select.py`): its quick list took 232 s of the
    /// 240 on a 4-core host with nothing else running.
    pub fn budget(&self, linux_compat: bool, slow: bool) -> Duration {
        let scale = |s: u64| Duration::from_secs(if slow { s * 3 / 2 } else { s });
        match self {
            Mode::Tests(sel) if sel.budget.is_some() => sel.budget.unwrap(),
            Mode::Tests(sel) if sel.quick() => scale(if linux_compat { 300 } else { 240 }),
            Mode::Tests(_) => Duration::from_secs(3000),
            Mode::Reboot { .. } | Mode::Installed => Duration::from_secs(300),
        }
    }

    /// How long the console may stay silent. The long steps (installing
    /// the packages, an Alpine package's unpack) print little.
    fn stall(&self) -> Duration {
        match self {
            Mode::Tests(sel) if !sel.quick() => Duration::from_secs(600),
            _ => Duration::from_secs(180),
        }
    }
}

/// What a boot of the harness found: whether every test passed (and the
/// boot's own checks held), and the names of the tests that passed, for
/// the boots that may follow.
pub struct Outcome {
    pub ok: bool,
    pub passed: Vec<String>,
}

/// What the kernel and init print before the login prompt: the boot's own
/// self-checks (memory, interrupts, the scheduler, the drivers, the VFS
/// through `/bin/custom/ok`). Required in every mode; a missing one fails
/// the run even when every test passed.
const BOOT_MARKERS: [&str; 38] = [
    "Hello from myos",
    "[ OK ] heap",
    "[ OK ] dma pages",
    "[ OK ] platform",
    "[ OK ] interrupts",
    "task a",
    "task b",
    "[ OK ] scheduler",
    "[ OK ] urandom",
    "[ OK ] hello",
    "[ OK ] boot modules",
    "[ OK ] fork",
    "[ OK ] fork exec",
    "[ OK ] alloc",
    "[ OK ] user",
    "[ OK ] msg",
    "[ OK ] fat",
    "[ OK ] vda",
    "[ OK ] net0",
    "[ OK ] tap",
    "[ OK ] netmac",
    "[ OK ] nvme",
    "[ OK ] sdmmc",
    "[ OK ] xhci",
    "[ OK ] usb_hub",
    "[ OK ] usb_storage",
    "[ OK ] usb_hid",
    "[ OK ] ext2",
    "[ OK ] ext2 rw",
    "[ OK ] fat ls",
    "[ OK ] fat read",
    "[ OK ] devnull",
    "[ OK ] tmp",
    "[ OK ] tmpops",
    "[ OK ] proc",
    "[ OK ] tty",
    "[ OK ] signal",
    "[ OK ] rc started",
];

/// The board a test boot runs on: what it prints before the login prompt,
/// and whether it has the test disks the host checks afterwards.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Board {
    /// QEMU's PCs and `virt` boards: every device of [`BOOT_MARKERS`], the
    /// scratch and FAT disks.
    Virt,
    /// QEMU's Raspberry Pi 3 (`raspi3b`, `cargo run -- rpi3 test`): no
    /// virtio, PCI or USB devices, no disk but the SD card it boots from
    /// ([`RPI3_MARKERS`]).
    Rpi3,
}

impl Board {
    fn markers(self) -> &'static [&'static str] {
        match self {
            Board::Virt => &BOOT_MARKERS,
            Board::Rpi3 => &RPI3_MARKERS,
        }
    }
}

/// [`BOOT_MARKERS`] that need no device a Raspberry Pi 3 lacks under QEMU.
const RPI3_MARKERS: [&str; 25] = [
    "Hello from myos",
    "[ OK ] heap",
    "[ OK ] dma pages",
    "[ OK ] platform",
    "[ OK ] interrupts",
    "task a",
    "task b",
    "[ OK ] scheduler",
    "[ OK ] smp",
    "[ OK ] urandom",
    "[ OK ] hello",
    "[ OK ] sdmmc",
    "[ OK ] boot modules",
    "[ OK ] fork",
    "[ OK ] fork exec",
    "[ OK ] alloc",
    "[ OK ] user",
    "[ OK ] msg",
    "[ OK ] devnull",
    "[ OK ] tmp",
    "[ OK ] tmpops",
    "[ OK ] proc",
    "[ OK ] tty",
    "[ OK ] signal",
    "[ OK ] rc started",
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
/// `poweroff` stops the processes (a few seconds at most), unmounts the
/// disks and powers off: QEMU exits well within this.
const POWER_OFF_BOUND: Duration = Duration::from_secs(60);

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

/// The console after init handed over to getty, without the kernel's
/// status lines: the serial port is `ttyS0`'s, but the kernel's messages
/// still go there too (`docs/tty.md`), and one can land anywhere, after a
/// prompt (`login: [ OK ] linux`, a module the boot script loads) or in
/// the middle of an echo (`roo[ OK ] linux\nt`, `P[ OK ] linux\nassword:`).
/// A status line still arriving, its tag or its text without the newline
/// yet, is left out too: it would read as typed text and garble the login
/// (riscv64-light on master, 2026-10-16).
fn after_boot(text: &str) -> String {
    let text = text.rsplit_once("[ OK ] fork exec").map(|(_, t)| t).unwrap_or(text);
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        if status_arriving(rest) {
            break;
        }
        match status_line_at(rest) {
            Some(len) => rest = &rest[len..],
            None => {
                let c = rest.chars().next().unwrap_or_default();
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    out
}

const STATUS_TAGS: [&str; 4] = ["[ OK ] ", "[ FAIL ] ", "[ WARN ] ", "[ INFO ] "];

/// `text`, the end of the console, is a kernel status line that has not
/// finished arriving: part of a tag, or a tag and its label without the
/// newline yet.
fn status_arriving(text: &str) -> bool {
    STATUS_TAGS.iter().any(|t| (t.starts_with(text) && text.len() < t.len()) || (text.starts_with(t) && !text.contains('\n')))
}

/// The length of the kernel status line (`[ OK ] label\n`, with its
/// newline) `text` starts with, if it does.
fn status_line_at(text: &str) -> Option<usize> {
    let tag = STATUS_TAGS.into_iter().find(|t| text.starts_with(t))?;
    let end = text[tag.len()..].find('\n')?;
    Some(tag.len() + end + 1)
}

fn at_prompt(text: &str) -> bool {
    after_boot(text).trim_end().ends_with('$')
}

/// Bytes echoed on the current line after `prompt` (its last occurrence).
fn typed_after(text: &str, prompt: &str) -> String {
    match after_boot(text).rsplit_once(prompt) {
        Some((_, rest)) => rest.lines().next().unwrap_or("").trim_end_matches('\r').to_string(),
        None => String::new(),
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
            let got = typed_after(&snapshot(serial), prompt);
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

/// After the tests: type `poweroff` at the prompt the runner returned to
/// and wait for QEMU to exit by itself, the kernel having stopped the
/// processes, unmounted the disks and powered the board off
/// (docs/power.md).
fn power_off(stdin: &mut ChildStdin, serial: &Shared, child: &mut Child) -> bool {
    let deadline = Instant::now() + POWER_OFF_BOUND;
    if !wait_for(serial, deadline, |t| at_prompt(t) && typed_after(t, "$ ").is_empty())
        || !type_synced(stdin, serial, "$ ", "poweroff")
    {
        eprintln!("boot test: could not type `poweroff`");
        return false;
    }
    loop {
        if let Some(status) = child.try_wait().expect("wait on qemu") {
            let via = snapshot(serial).lines().find(|l| l.contains("power off via ")).map(str::to_string);
            eprintln!("boot test: QEMU exited ({status}) after `poweroff`: {}", via.as_deref().unwrap_or("no method named"));
            return status.success() && via.is_some();
        }
        if Instant::now() > deadline {
            eprintln!("boot test: QEMU still running {POWER_OFF_BOUND:?} after `poweroff`");
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
        let status = Command::new("bash")
            .arg(&script)
            .args(&args)
            .current_dir(repo_root())
            .env("MYOS_QEMU_MONITOR", monitor_socket())
            .status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => eprintln!("boot test: HOST {req}: {script} exited with {s}"),
            Err(e) => eprintln!("boot test: HOST {req}: {script} did not start: {e}"),
        }
    });
}

/// The QEMU monitor of a test boot, a unix socket the host scripts reach as
/// `$MYOS_QEMU_MONITOR` (`sendkey` types on the guest's keyboard).
pub fn monitor_socket() -> PathBuf {
    std::env::temp_dir().join(format!("myos-monitor-{}.sock", std::process::id()))
}

/// The `-monitor` argument for [`monitor_socket`].
pub fn monitor_arg() -> String {
    let path = monitor_socket();
    let _ = std::fs::remove_file(&path);
    format!("unix:{},server=on,wait=off", path.display())
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

/// A guest that went silent is asked where its CPUs are before it is
/// killed: the QEMU monitor's `info registers -a`, every CPU's registers
/// (the PC of a spin or a halt, by `addr2line` on the kernel). `None` when
/// the monitor does not answer within a few seconds (QEMU itself stuck).
fn cpu_registers() -> Option<String> {
    let mut mon = UnixStream::connect(monitor_socket()).ok()?;
    mon.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    let mut buf = [0u8; 4096];
    // The banner and the prompt.
    let _ = mon.read(&mut buf);
    mon.write_all(b"info registers -a\n").ok()?;
    let mut out = String::new();
    loop {
        match mon.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => out.push_str(&String::from_utf8_lossy(&buf[..n])),
        }
    }
    // The first line is the monitor echoing the command (with its line
    // editor's escape codes), the last its prompt again.
    let body = out.split_once('\n').map_or("", |(_, rest)| rest);
    let body = body.trim().trim_end_matches("(qemu)").trim();
    (!body.is_empty()).then(|| body.to_string())
}

/// A stall: the console stopped, nothing else says why (a riscv64 boot
/// went silent at a USB unplug, issue #329). Dump the CPUs first, then
/// fail as usual.
fn fail_stalled(serial: &Shared, child: &mut Child, why: &str) -> ! {
    match cpu_registers() {
        Some(regs) => eprintln!("--- cpu registers (QEMU monitor) ---\n{regs}\n---"),
        None => eprintln!("boot test: the QEMU monitor did not answer: no CPU registers"),
    }
    fail(serial, child, why)
}

/// Boot test on a QEMU `child` started with `-serial stdio` and piped
/// stdio. Does not return: exits the process with the result.
/// The outcome: every test passed (a crash, a stall or the budget running out
/// ends the launcher at once instead).
pub fn run(mut child: Child, board: Board, mode: &Mode, linux_compat: bool, slow: bool) -> Outcome {
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
    let budget = mode.budget(linux_compat, slow);
    // The descriptors: a `HOST <port> ...` line runs that port's host script.
    let ports = crate::ports::load_all(&repo_root());
    let deadline = started + budget;

    // Log in on ttyS0: `root`, an empty password, then the one command.
    // Each line is typed in sync with its echo; a garbled line is abandoned
    // and typed again at the next prompt. A garbled echo of the name may
    // still have reached getty whole (the echo is what the kernel's status
    // lines disturb): the password prompt after the abandoning newline says
    // so, and the login goes on.
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
        let soon = Instant::now() + ECHO_WAIT;
        if wait_for(&serial, soon.min(login_deadline), |t| after_boot(t).contains("Password:")) {
            break;
        }
    }
    if !wait_for(&serial, login_deadline, |t| after_boot(t).contains("Password:")) {
        fail(&serial, &mut child, "no password prompt after `root`");
    }
    send(&mut stdin, b"\n");
    // The shell's line editor scrolls a line wider than the terminal
    // sideways, and its echo is no longer the line: the serial console is
    // as wide as the framebuffer's text screen (100 columns on the `virt`
    // boards' 800x600 ramfb), less than a long selection's command. A
    // COLUMNS assignment is the editor's width in ksh, so it comes first.
    let command = format!("sh /lib/myos-tests/run.sh{}", mode.args());
    for line in ["COLUMNS=1000", command.as_str()] {
        let mut attempts = 0;
        loop {
            attempts += 1;
            if attempts > 3 {
                fail(&serial, &mut child, "command line kept garbling");
            }
            if !wait_for(&serial, login_deadline, |t| at_prompt(t) && typed_after(t, "$ ").is_empty()) {
                fail(&serial, &mut child, "no shell prompt after login");
            }
            if type_synced(&mut stdin, &serial, "$ ", line) {
                break;
            }
            send(&mut stdin, b"\n");
        }
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
            fail_stalled(&serial, &mut child, &format!("no console output for {stall:?} (stall)"));
        }
        if Instant::now() > deadline {
            fail(&serial, &mut child, &format!("the {budget:?} budget ran out"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let powered_off = power_off(&mut stdin, &serial, &mut child);
    let _ = child.kill();
    let _ = child.wait();
    // The ext2 tests leave a filesystem on the scratch disk, the FAT test
    // wrote to the FAT disk: e2fsprogs and dosfstools must find them clean
    // (`src/main.rs`), the power-off having unmounted them.
    let disk_ok = board != Board::Virt || crate::fsck_scratch_disk() & crate::fsck_fat_disk();

    let text = snapshot(&serial);
    let missing: Vec<&str> = board.markers().iter().copied().filter(|m| !text.contains(m)).collect();
    let failed: Vec<&str> = results.iter().filter(|r| !r.passed).map(|r| r.name.as_str()).collect();
    let (passed, total) = done.unwrap_or((0, 0));
    eprintln!();
    eprintln!(
        "boot test (run.sh{}): {passed}/{total} passed in {:?}{}",
        mode.args(),
        started.elapsed(),
        if failed.is_empty() { String::new() } else { format!(", failed: {}", failed.join(" ")) }
    );
    for m in &missing {
        eprintln!("error: boot marker missing: {m:?}");
    }
    if results.len() as u32 != total {
        eprintln!("error: the runner reported {total} tests, {} TEST lines were seen", results.len());
    }
    if !powered_off {
        eprintln!("error: `poweroff` did not power the machine off");
    }
    let ok = failed.is_empty()
        && missing.is_empty()
        && passed == total
        && total != 0
        && results.len() as u32 == total
        && disk_ok
        && powered_off;
    Outcome { ok, passed: results.into_iter().filter(|r| r.passed).map(|r| r.name).collect() }
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
    fn selection_args() {
        let args = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        assert_eq!(Selection::parse(&args("uefi")), None);
        let sel = Selection::parse(&args("uefi test-mini")).unwrap();
        assert_eq!(sel.skip, ["net", "pkg", "heavy", "install", "alpine-disk"]);
        assert!(sel.quick() && !sel.needs_network() && !sel.needs_alpine_disk());
        assert_eq!(Mode::Tests(sel).args(), " --skip net,pkg,heavy,install,alpine-disk");
        let sel = Selection::parse(&args("test-full")).unwrap();
        assert_eq!(sel, Selection::default());
        assert!(!sel.quick() && sel.needs_network() && sel.needs_alpine_disk());
        let sel = Selection::parse(&args("test --tags smoke,netfs --skip heavy --budget 7")).unwrap();
        assert_eq!(sel.budget, Some(Duration::from_secs(420)));
        assert_eq!(Mode::Tests(sel).args(), " --tags smoke,netfs --skip heavy");
        assert_eq!(Mode::Reboot { app: true }.args(), " reboot");
        assert_eq!(Mode::Reboot { app: false }.args(), " reboot --skip app_kept");
    }

    #[test]
    fn prompt_and_echo() {
        let t = "[ OK ] fork exec\nlogin: ro";
        assert_eq!(typed_after(t, "login: "), "ro");
        let t = "[ OK ] fork exec\nlogin: root\nPassword: \n$ sh /lib";
        assert_eq!(typed_after(t, "$ "), "sh /lib");
        assert!(!at_prompt(t));
        assert!(at_prompt("[ OK ] fork exec\n$ "));
        // A kernel status line after a prompt is not what was typed.
        let t = "[ OK ] fork exec\nmyos login: [ OK ] linux\n";
        assert_eq!(typed_after(t, "login: "), "");
        assert!(at_prompt("[ OK ] fork exec\n$ [ OK ] linux\n"));
        // Nor one in the middle of an echo or a prompt, whole or still
        // arriving (riscv64-light on master: `roo[ OK ] linux` then `t`).
        assert_eq!(typed_after("[ OK ] fork exec\nmyos login: roo[ OK ] linux\nt", "login: "), "root");
        assert_eq!(typed_after("[ OK ] fork exec\nmyos login: roo[ OK ] ", "login: "), "roo");
        assert_eq!(typed_after("[ OK ] fork exec\nmyos login: roo[ O", "login: "), "roo");
        assert_eq!(typed_after("[ OK ] fork exec\nmyos login: roo[", "login: "), "roo");
        assert!(after_boot("[ OK ] fork exec\nlogin: root\nP[ OK ] linux\nassword: ").contains("Password: "));
        assert!(at_prompt("[ OK ] fork exec\n$ [ WARN ] rc: some"));
    }
}
