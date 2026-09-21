use std::io::Write;
use std::process::ChildStdin;

struct CiExpect {
    timeout: Duration,
    qemu_debug_exit: bool,
    /// Type commands at the interactive `$` prompt via `-serial stdio`.
    shell_ci: bool,
}

const CI_NEEDLES: [&str; 33] = [
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
    // Slim always-on `/ok` VFS markers (pre-prompt readiness).
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

/// Heavy markers from CI-only `/heap` (typed at `$` on every arch).
/// Pre-prompt readiness stays slim; these are required after interactive `heap`.
///
/// Note: `[ OK ] dns` / `[ OK ] https` are intentionally NOT here. They are
/// separate interactive commands (indices 11/12) run *after* `heap`; they never
/// print during `/heap`, so including them here would abort before those
/// commands are typed. Verified by `interactive_dns_cmd_ok` /
/// `interactive_https_cmd_ok`.
const CI_NEEDLES_STD: [&str; 22] = [
    "[ OK ] std",
    "[ OK ] std cat",
    "[ OK ] std echo",
    "[ OK ] bigalloc",
    "[ OK ] c",
    "[ OK ] sbase",
    "[ OK ] sls",
    "[ OK ] uutils echo",
    "[ OK ] uutils true",
    "[ OK ] uutils false",
    "[ OK ] find",
    "/tmp/findnest/a/b/c",
    "[ OK ] uutils cat",
    "[ OK ] uutils ls",
    "[ OK ] ripgrep",
    "[ OK ] sbase argv",
    "[ OK ] tcc",
    "[ OK ] tcc std",
    "[ OK ] git",
    "[ OK ] git commit",
    "[ OK ] ping",
    "[ OK ] socket",
];

/// Interactive shell commands typed at the `$` prompt (serial stdin).
///
/// Full mode: base commands + HTTPS GET + curl + interrupt/arrow tests.
/// Mini mode (`MYOS_CI_MINI=1`): base commands only (no http, no curl), for
/// the fast boot-mini CI jobs; interrupt/seed/arrow still run.
const CMD_NOSUCH: &[u8] = b"nosuchcmd\n";
const CMD_HEAP: &[u8] = b"heap\n";
const CMD_HEAP_MINI: &[u8] = b"heap mini\n";
const CMD_OK: &[u8] = b"ok\n";
// boot-mini passes `heap mini` so the heavy git porcelain stage stays in the
// full boot jobs; full mode runs plain `heap`.
const CMD_ECHO: &[u8] = b"echo test\n";
const CMD_PIPE: &[u8] = b"echo pipe | cat\n";
const CMD_TRUE: &[u8] = b"/bin/coreutils/true\n";
const CMD_SBASE_ECHO: &[u8] = b"/bin/sbase/echo hi\n";
const CMD_SBASE_LS: &[u8] = b"/bin/sbase/ls\n";
// Typo then backspaces: canonical stdin must deliver `/s/ls`, not `x/s/ls` or raw BS.
const CMD_BS_LS: &[u8] = b"x\x08/bin/sbase/ls\n";
// oksh redirect uses newlib O_CREAT; must create on tmpfs (not only `/ok`).
const CMD_TMP_REDIR: &[u8] = b"echo test > /tmp/aaa; cat /tmp/aaa\n";
// `which` walks $PATH via fstatat(dirfd, name); must print a real PATH hit.
const CMD_WHICH: &[u8] = b"which ls\n";
// DNS resolution test (requires network).
const CMD_DNS: &[u8] = b"dns www.google.com\n";
// pty boot-CI smoke (openpty/forkpty, echo round-trip, EIO on session end).
const CMD_PTY: &[u8] = b"/bin/etc/pty_smoke 2\n";
// urandom boot-CI smoke (kernel CSPRNG via /dev/urandom: non-zero, distinct,
// successive reads differ).
const CMD_URANDOM: &[u8] = b"/bin/etc/urandom_smoke\n";
// netd listen/accept smoke: the guest announces TCP 2323 in the background;
// the harness connects back through slirp hostfwd (see add_virtio_net in
// src/main.rs), sends "ping", and expects "pong". Then `cat` must show
// `[ OK ] listen` from the redirected output.
const CMD_LISTEN_BG: &[u8] =
    b"/bin/etc/tcp_listen_smoke > /tmp/listen.out 2>&1 & echo $! > /tmp/listen.pid\n";
const CMD_LISTEN_CAT: &[u8] = b"cat /tmp/listen.out\n";
/// Tear down listen smoke before `cat | cat`. Leaving tcp_listen_smoke (+ #166
/// netd reclaim/drain) live across the interrupt stage raced UEFI SMP fork/
/// aspace for the pipeline (user page fault cr2 in HHDM, code=0x5, tip
/// 42b7f2c boot-mini). Same discipline as CMD_DROPBEAR_STOP before ostest.
const CMD_LISTEN_STOP: &[u8] =
    b"kill $(cat /tmp/listen.pid) 2>/dev/null; echo LISTEN-STOP\n";
/// Harness-side flag: the ping/pong exchange through hostfwd succeeded.
static LISTEN_PONGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// When the listen smoke stage first began (bounds the ping/pong retry loop).
static LISTEN_STAGE_START: std::sync::OnceLock<std::time::Instant> =
    std::sync::OnceLock::new();
const LISTEN_STAGE_BOUND: Duration = Duration::from_secs(180);
/// Shared serial accumulator, so failure paths can dump fresh output.
static SERIAL_ACC: std::sync::OnceLock<std::sync::Arc<std::sync::Mutex<String>>> =
    std::sync::OnceLock::new();

/// Runner-side half of the listen/accept smoke: connect to the guest listener
/// through hostfwd, send "ping", expect "pong". One attempt per call; the
/// stage machine retries while the guest listener is announcing.
fn poke_listener() -> bool {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    let addr = SocketAddr::from(([127, 0, 0, 1], 2323));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(5))
    else {
        std::thread::sleep(std::time::Duration::from_millis(250));
        return false;
    };
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(15)));
    if stream.write_all(b"ping").is_err() {
        return false;
    }
    let mut buf = [0u8; 8];
    let mut got = 0usize;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while got < 5 && std::time::Instant::now() < deadline {
        match stream.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(_) => break,
        }
    }
    got >= 5 && &buf[..5] == b"pong\n"
}
// Dropbear SSH smoke (full boot only): start sshd in the guest; the harness
// then opens two sequential OpenSSH clients through slirp hostfwd
// localhost:2222 → guest:22 (see add_virtio_net in src/main.rs).
// netd keeps a parked accept + on-listen hold (backlog 2); pump_sockets skips
// listeners so a held backlog SYN cannot mark the listen conv connected/hungup.
const CMD_DROPBEAR_BG: &[u8] =
    b"/bin/custom/dropbear -F -E -p 22 -r /etc/dropbear/ed25519_hostkey > /tmp/dropbear.out 2>&1 & echo $! > /tmp/dropbear.pid\n";
/// Stop dropbear before listen smoke so netd convs / slirp stay free for :2323.
const CMD_DROPBEAR_STOP: &[u8] =
    b"kill $(cat /tmp/dropbear.pid) 2>/dev/null; echo DROPBEAR-STOP\n";
/// Harness-side flag: both sequential SSH clients completed with exit 0.
static SSH_SMOKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Background SSH attempt started for the current dropbear stage.
static SSH_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Background attempt finished (ok or err); see SSH_SMOKED / SSH_ERR.
static SSH_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static SSH_ERR: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
static SSH_STAGE_START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
const SSH_STAGE_BOUND: Duration = Duration::from_secs(300);
const SSH_HOST_PORT: &str = "2222";

fn dropbear_testkey_src() -> PathBuf {
    let from_manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ports/dropbear/testkey");
    if from_manifest.is_file() {
        return from_manifest;
    }
    PathBuf::from("ports/dropbear/testkey")
}

/// OpenSSH refuses world-readable private keys; copy the committed test key
/// to a 0600 tempfile for the duration of the smoke.
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

/// Ensure a host `ssh` client exists. The myos-ci image should ship
/// openssh-client; fall back to apt-get when running as root on an older image.
fn ensure_host_ssh() -> Result<(), String> {
    if Command::new("ssh").arg("-V").output().is_ok() {
        return Ok(());
    }
    let apt = Command::new("apt-get")
        .args(["update", "-qq"])
        .status()
        .map_err(|e| format!("apt-get update: {e}"))?;
    if !apt.success() {
        return Err("apt-get update failed; install openssh-client for SSH smoke".into());
    }
    let inst = Command::new("apt-get")
        .args([
            "install",
            "-y",
            "-qq",
            "--no-install-recommends",
            "openssh-client",
        ])
        .status()
        .map_err(|e| format!("apt-get install openssh-client: {e}"))?;
    if !inst.success() {
        return Err("failed to install openssh-client".into());
    }
    if Command::new("ssh").arg("-V").output().is_err() {
        return Err("`ssh` still missing after apt install".into());
    }
    Ok(())
}

fn ssh_one_client(key: &Path, tag: &str) -> Result<(), String> {
    let remote = format!("echo {tag}; /bin/coreutils/true");
    // Wrap with `timeout` so a hung KEX (ConnectTimeout only covers TCP
    // connect) cannot burn the whole SSH stage budget — seen on riscv64
    // where one Child + I/O error left ssh blocked until recv_timeout(60).
    let output = Command::new("timeout")
        .args([
            "20",
            "ssh",
            "-4",
            "-i",
            key.to_str().ok_or("testkey path not utf-8")?,
            "-p",
            SSH_HOST_PORT,
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
        return Err(format!(
            "ssh {tag} failed (exit {:?}): stderr={stderr} stdout={stdout}",
            output.status.code()
        ));
    }
    if !stdout.contains(tag) {
        return Err(format!(
            "ssh {tag}: remote echo missing (stdout={stdout:?} stderr={stderr:?})"
        ));
    }
    Ok(())
}

/// Open two sequential SSH sessions into the guest (pubkey auth). Both must
/// exit 0. Keep sequential host clients here: concurrent dual-SYN through
/// QEMU slirp hostfwd still flakes on riscv64/aarch64 even with netd's
/// two-slot accept queue (#161) — one Child+pubkey then banner-timeout /
/// bad-packet for the peer. netd backlog remains for real concurrent use;
/// this smoke still covers accept + pubkey + exit-status twice.
fn poke_ssh_two_clients(key: &Path) -> Result<(), String> {
    ssh_one_client(key, "ssh-ci-a")?;
    // Gap so netd finishes teardown of the first Child before the next SYN.
    // Back-to-back clients on riscv64 TCG produced dropbear
    // "Integrity error (bad packet size …)" / hung KEX for ssh-ci-b (#164).
    std::thread::sleep(Duration::from_secs(2));
    ssh_one_client(key, "ssh-ci-b")?;
    Ok(())
}

/// Background worker: retry two sequential SSH clients until success or bound.
fn start_ssh_smoke_worker() {
    if SSH_STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        // Install openssh-client + stage the test key ONCE before any connect.
        // apt-get inside the retry loop raced the first SYNs on older images.
        let key = match ensure_host_ssh().and_then(|_| prepare_dropbear_testkey()) {
            Ok(k) => k,
            Err(e) => {
                if let Ok(mut g) = SSH_ERR.lock() {
                    *g = e;
                }
                SSH_DONE.store(true, std::sync::atomic::Ordering::SeqCst);
                return;
            }
        };
        // Stage clock starts here (post-apt) so the outer WaitResult bound
        // matches the worker's retry window.
        let start = Instant::now();
        let _ = SSH_STAGE_START.set(start);
        // Longer settle on slow arches (riscv64): early SYNs before dropbear
        // has armed accept hang in slirp or produce Child+I/O-error and wedge
        // the listener for the rest of the stage. Do NOT TCP-probe :2222 —
        // a connect+close is itself a half-open session that can wedge.
        std::thread::sleep(Duration::from_secs(12));
        let mut last_err = String::from("ssh smoke never attempted");
        while start.elapsed() < SSH_STAGE_BOUND {
            match poke_ssh_two_clients(&key) {
                Ok(()) => {
                    SSH_SMOKED.store(true, std::sync::atomic::Ordering::SeqCst);
                    SSH_DONE.store(true, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
                Err(e) => {
                    last_err = e;
                    // Back off harder than 1s: each failed attempt can leave a
                    // SynReceived orphan in netd until handshake-age reclaim
                    // (~10s). Flooding SYNs every second filled MAX_CONV on
                    // riscv64 before dropbear could accept a live session.
                    std::thread::sleep(Duration::from_secs(3));
                }
            }
        }
        if let Ok(mut g) = SSH_ERR.lock() {
            *g = last_err;
        }
        SSH_DONE.store(true, std::sync::atomic::Ordering::SeqCst);
    });
}

// HTTPS GET (requires network + wall clock + mbedtls).
const CMD_HTTP: &[u8] = b"http https://example.com/\n";
// curl over userspace sockets + mbedtls (same URL as https smoke).
const CMD_CURL: &[u8] = b"curl -fsS --connect-timeout 30 --max-time 90 -o /tmp/curl-ex.html https://example.com/; cat /tmp/curl-ex.html\n";

/// os-test basic smoke (full boot only). Thin writable copy via
/// `misc/ci-smoke-copy.sh` (Makefile + misc/ + basic.h + TESTLIST sources
/// only — not `cp -r` of the whole suite) then
/// `make SUITES=basic TESTLIST=misc/ci-basic-smoke.tests report` (~22 tests).
/// NOT the full ~1187 basic suite (#860 timed out; #866 still burned 90m on
/// full-tree copy + SMP). Report prints `pass_rate=NN% (P/T)`; CI asserts the
/// harness finished but does NOT fail on pass_rate<80. Follow-up short
/// quote-free commands still require setpwent success. Commands stay
/// quote-free for oksh redraw.
const CMD_OS_TEST_PREP: &[u8] =
    b"sh /lib/os-test/misc/ci-smoke-copy.sh /tmp/o && cd /tmp/o && make SUITES=basic TESTLIST=misc/ci-basic-smoke.tests report; echo PREP-RC=$?\n";
/// After the suite report: cat setpwent .err/.out (success leaves .out empty).
const CMD_OS_TEST_CAT: &[u8] =
    b"cat out/basic/pwd/setpwent.err out/basic/pwd/setpwent.out\n";
/// setpwent gate: non-empty .out means compile_error / exit:N → SETPWENT-FAIL.
const CMD_OS_TEST_RESULT: &[u8] =
    b"test -s out/basic/pwd/setpwent.out && echo SETPWENT-FAIL || echo SETPWENT-OK\n";
// ^C interrupt test: run a foreground `cat | cat` (the right cat blocks on
// a kernel pipe read, the left one on the console) and interrupt it. The shell
// must survive (ignore SIGINT) while both children die, returning to `$`
// (no getty/login respawn. The right cat only dies weil the kernel's pipe-read
// wait wakes on a pending fatal signal; see `interactive_interrupt_cmd_ok`
// and the interrupt handling in `advance_shell_ci`.
const CMD_INTERRUPT: &[u8] = b"cat | cat\n";
// Seed a distinct history entry for the arrow-key test that follows.
const CMD_HIST_SEED: &[u8] = b"echo histrecall_zz\n";
// Arrow-key editing: at the emacs-raw prompt, Up (ESC [ A) must recall the
// previous `echo histrecall_zz`, Left (ESC [ D) moves the cursor off the end,
// `3` inserts, and Enter runs the edited line -> `echo histrecall_z3z` prints
// `histrecall_z3z`. If raw mode / the editor are broken the kernels cooked
// gate swallows the CSI bytes and the recalled+edited command never runs;
// `histrecall_z3z` then never appears.
const CMD_ARROW: &[u8] = b"\x1b[A\x1b[D3\n";

/// True when the harness runs in boot-mini mode: skip the HTTPS GET and curl
/// smokes (the two long network stages); everything else is unchanged.
pub fn ci_mini() -> bool {
    std::env::var_os("MYOS_CI_MINI").map(|v| v == "1").unwrap_or(false)
}

fn ci_shell_commands() -> Vec<&'static [u8]> {
    let mut cmds: Vec<&'static [u8]> = vec![
        CMD_NOSUCH,
        // CI-only heavy smoke (std/C/sbase/uutils/bigalloc); slim `/ok` already ran at boot.
        // boot-mini passes `heap mini` so the heavy git stage stays in the full boot jobs.
        if ci_mini() { CMD_HEAP_MINI } else { CMD_HEAP },
        CMD_OK,
        CMD_ECHO,
        CMD_PIPE,
        CMD_TRUE,
        CMD_SBASE_ECHO,
        CMD_SBASE_LS,
        CMD_BS_LS,
        CMD_TMP_REDIR,
        CMD_WHICH,
        CMD_DNS,
    ];
    if !ci_mini() {
        cmds.push(CMD_HTTP);
        cmds.push(CMD_CURL);
        // Dropbear SSH smoke: full boot only, early after outbound HTTPS so we
        // still reach it if later ostest/pty stages burn the QEMU budget.
        // Host opens two sequential clients via slirp hostfwd (:2222→:22).
        if port_enabled("port_dropbear") {
            cmds.push(CMD_DROPBEAR_BG);
            // Tear down sshd before ostest/pty/listen. Leaving dropbear up
            // through ostest raced netd on aarch64 (user fault FAR~"net/tcp",
            // PREP-RC=139) and starved listen accept on uefi.
            cmds.push(CMD_DROPBEAR_STOP);
        }
        // os-test basic smoke (TESTLIST) + setpwent gate (full boot only;
        // too slow for boot-mini). pass_rate is reported, not gated.
        cmds.push(CMD_OS_TEST_PREP);
        cmds.push(CMD_OS_TEST_CAT);
        cmds.push(CMD_OS_TEST_RESULT);
        cmds.push(CMD_PTY);
        cmds.push(CMD_URANDOM);
    }
    // netd listen/accept smoke runs in every mode (network is up; the
    // harness completes the ping/pong through slirp hostfwd).
    cmds.push(CMD_LISTEN_BG);
    cmds.push(CMD_LISTEN_CAT);
    cmds.push(CMD_LISTEN_STOP);
    cmds.push(CMD_INTERRUPT);
    cmds.push(CMD_HIST_SEED);
    cmds.push(CMD_ARROW);
    cmds
}

/// The last three commands are always the ^C interrupt test, the history
/// seed and the arrow-key editing test; their indexes depend on whether the
/// HTTPS/curl smokes were dropped in mini mode.
fn interrupt_cmd_idx(cmds: &[&[u8]]) -> usize {
    cmds.len() - 3
}
fn arrow_seed_idx(cmds: &[&[u8]]) -> usize {
    cmds.len() - 2
}
fn arrow_edit_idx(cmds: &[&[u8]]) -> usize {
    cmds.len() - 1
}

/// Interactive curl prompt echo. The full `$ curl -fsS --connect-timeout 30 …`
/// command must echo as ONE clean line: oksh's emacs editor sizes the prompt
/// from the device winsize, and the kernel reports a wide (160-col) console on
/// every boot, so the line no longer wraps with redraw artifacts on the
/// aarch64/riscv64 serial boots (the old 24x80 fallback did). Keep a strict
/// full-line needle (including the `; cat` tail) so a wrapped/redraw-corrupted
/// echo must NOT match.
const CURL_ECHO: &str =
    "$ curl -fsS --connect-timeout 30 --max-time 90 -o /tmp/curl-ex.html https://example.com/; cat /tmp/curl-ex.html";

/// Printed by the interactive shell when a command cannot be resolved.
const CI_SHELL_UNKNOWN_CMD: &str = "not found";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellStage {
    WaitLogin,
    TypingUser,
    WaitPassword,
    TypingPass,
    WaitPrompt,
    Typing,
    WaitResult,
    Done,
}

fn serial_has_all_needles(serial: &str, extra: &[&str]) -> bool {
    for n in CI_NEEDLES.iter().chain(extra.iter()) {
        if !needle_for_enabled_port(*n) {
            continue;
        }
        if !serial.contains(*n) {
            return false;
        }
    }
    true
}

fn interactive_tail(serial: &str) -> &str {
    serial
        .rsplit_once("[ OK ] fork exec")
        .map(|(_, tail)| tail)
        .unwrap_or(serial)
}

fn at_interactive_prompt(serial: &str) -> bool {
    let tail = interactive_tail(serial).trim();
    tail == "$" || tail.ends_with("$")
}

const CI_LOGIN_USER: &[u8] = b"root\n";
const CI_LOGIN_PASS: &[u8] = b"\n";

fn login_prompt_ready(serial: &str) -> bool {
    // Require an empty typed suffix so garble-recovery (`\n` + WaitLogin) does
    // not immediately re-enter TypingUser on the still-visible old `login: rr…`
    // line before getty reprints a fresh prompt.
    serial.contains("[ OK ] fork exec")
        && interactive_tail(serial).contains("login: ")
        && login_line_typed(serial).is_empty()
}

fn password_prompt_ready(serial: &str) -> bool {
    interactive_tail(serial).contains("Password:")
}

/// Bytes already echoed on the current getty `login: ` line (after the prompt).
fn login_line_typed(serial: &str) -> &str {
    let tail = interactive_tail(serial);
    match tail.rsplit_once("login: ") {
        Some((_, rest)) => rest.lines().next().unwrap_or("").trim_end_matches('\r'),
        None => "",
    }
}

/// Bytes already echoed on the current shell input line (after the last `$`).
fn shell_line_typed(serial: &str) -> &str {
    let tail = interactive_tail(serial);
    // Split on the `$ ` prompt token, NOT a bare '$': a typed command may
    // itself contain `$` (e.g. `echo PREP-RC=$?`), which made rsplit('$')
    // return the tail after the command's own `$` and echo-sync stall forever
    // on that command.
    let after = tail.rsplit("$ ").next().unwrap_or(tail);
    // Prompt is `$ ` — strip a single leading space from the echoed command.
    let line = after.lines().next().unwrap_or(after);
    line.strip_prefix(' ').unwrap_or(line).trim_end_matches('\r')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EchoSync {
    /// Echo matches the bytes we have already sent.
    Synced,
    /// Echo is a proper prefix — still catching up (do NOT resend).
    Waiting,
    /// Extra/wrong bytes (duplicate resend or drop garble).
    Garble,
}

fn echo_sync_state(got: &str, sent_prefix: &[u8]) -> EchoSync {
    let want = String::from_utf8_lossy(sent_prefix);
    if got.as_bytes() == sent_prefix {
        EchoSync::Synced
    } else if want.as_ref().starts_with(got) {
        EchoSync::Waiting
    } else {
        EchoSync::Garble
    }
}

fn command_echoed(serial: &str, cmd: &str) -> bool {
    interactive_tail(serial).contains(&format!("$ {cmd}"))
}

/// Regression for the real-hardware `ok` -> `/???` bug: stderr must not report hex escapes.
fn tail_has_hex_received(serial: &str) -> bool {
    interactive_tail(serial).contains("(received: \\x")
}

fn interactive_unknown_cmd_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    let Some(idx) = tail.find(CI_SHELL_UNKNOWN_CMD) else {
        return false;
    };
    tail[..idx].contains("$ nosuchcmd") && !tail_has_hex_received(serial)
}

fn interactive_ok_cmd_ok(serial: &str) -> bool {
    command_echoed(serial, "ok") && !tail_has_hex_received(serial) && at_interactive_prompt(serial)
}

fn interactive_echo_cmd_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    if !tail.contains("$ echo test") || serial.contains("exception:") {
        return false;
    }
    let after = tail
        .rsplit_once("$ echo test")
        .map(|(_, rest)| rest)
        .unwrap_or("");
    after
        .lines()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| line.trim() == "test")
        && at_interactive_prompt(serial)
}

fn interactive_pipe_cmd_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    tail.contains("echo pipe | cat")
        && tail.lines().any(|line| line.trim() == "pipe")
        && !serial.contains("exception:")
        && !serial.contains("[ WARN ] user fault")
        && !serial.contains("user panic")
        && at_interactive_prompt(serial)
}

fn interactive_uutils_true_cmd_ok(serial: &str) -> bool {
    command_echoed(serial, "/bin/coreutils/true")
        && !serial.contains("exception:")
        && at_interactive_prompt(serial)
}

fn interactive_sbase_echo_cmd_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    if !tail.contains("$ /bin/sbase/echo hi") || serial.contains("exception:") {
        return false;
    }
    let after = tail
        .rsplit_once("$ /bin/sbase/echo hi")
        .map(|(_, rest)| rest)
        .unwrap_or("");
    after
        .lines()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| line.trim() == "hi")
        && at_interactive_prompt(serial)
}

fn interactive_sbase_ls_cmd_ok(serial: &str) -> bool {
    command_echoed(serial, "/bin/sbase/ls")
        && !serial.contains("exception:")
        && !serial.contains("user panic")
        && at_interactive_prompt(serial)
}

/// `x<BS>/s/ls` must run `/s/ls` (canonical erase), not leave a bogus argv.
fn interactive_tmp_redir_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    let echoed = "$ echo test > /tmp/aaa; cat /tmp/aaa";
    if !tail.contains(echoed) || serial.contains("exception:") {
        return false;
    }
    if tail.contains("cannot create") {
        return false;
    }
    let after = tail.rsplit_once(echoed).map(|(_, rest)| rest).unwrap_or("");
    after
        .lines()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| line.trim() == "test")
        && at_interactive_prompt(serial)
}


/// `which ls` must resolve via $PATH (not cwd): print an absolute `…/ls` path.
fn interactive_which_ls_cmd_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    if !tail.contains("$ which ls") || serial.contains("exception:") {
        return false;
    }
    if tail.contains("not an external command") {
        return false;
    }
    let after = tail.rsplit_once("$ which ls").map(|(_, rest)| rest).unwrap_or("");
    let path_line = after
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('$'));
    path_line.is_some_and(|line| {
        line.starts_with('/') && (line.ends_with("/ls") || line == "/ls")
    }) && at_interactive_prompt(serial)
}

/// DNS resolution test: `dns www.google.com` should print `IP: x.x.x.x` and return to `$`.
fn interactive_dns_cmd_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    if !tail.contains("$ dns www.google.com") || serial.contains("exception:") {
        return false;
    }
    // DNS command should print `IP: x.x.x.x` followed by `[ OK ] dns` and return to prompt.
    if !tail.contains("IP: ") || !tail.contains("[ OK ] dns") {
        return false;
    }
    at_interactive_prompt(serial)
}

/// urandom smoke: `/bin/etc/urandom_smoke` prints `[ INFO ] urandom <hex>
/// <hex>` and `[ OK ] urandom`, then returns to `$`.
fn interactive_urandom_cmd_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    if !tail.contains("$ /bin/etc/urandom_smoke") || serial.contains("exception:") {
        return false;
    }
    if !tail.contains("[ OK ] urandom") || !tail.contains("[ INFO ] urandom ") {
        return false;
    }
    at_interactive_prompt(serial)
}

/// pty smoke (basic stage 2): openpty round-trip through the shared line
/// discipline — master write -> slave read, slave write -> master read with
/// OPOST/ONLCR, then `EIO` after the slave closes — and back to `$`.
fn interactive_pty_cmd_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    if !tail.contains("$ /bin/etc/pty_smoke 2") || serial.contains("exception:") {
        return false;
    }
    // Stage 2 is the openpty round-trip + EIO path; it does not print a
    // `/dev/pts/N` path (that is stage 0/1). Match the `[ OK ] s2 EIO` marker.
    if !tail.contains("[ OK ] s2 EIO") {
        return false;
    }
    at_interactive_prompt(serial)
}

/// HTTPS GET: `http https://example.com/` should include `Example Domain` and `[ OK ] https`.
/// Note: `[ OK ] https` must NOT be added to CI_NEEDLES_STD (same trap as dns).
fn interactive_https_cmd_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    if !tail.contains("$ http https://example.com/") || serial.contains("exception:") {
        return false;
    }
    if !tail.contains("Example Domain") || !tail.contains("[ OK ] https") {
        return false;
    }
    at_interactive_prompt(serial)
}


/// curl HTTPS GET via userspace sockets: file should contain Example Domain.
/// Scope failure checks to output *after* the curl echo so earlier interactive
/// `nosuchcmd: not found` does not permanently fail this stage.
fn interactive_curl_cmd_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    if !tail.contains(CURL_ECHO) || serial.contains("exception:") {
        return false;
    }
    let after = tail.rsplit_once(CURL_ECHO).map(|(_, rest)| rest).unwrap_or("");
    if interactive_curl_after_failed(after) {
        return false;
    }
    // The echoed command must be one clean line: `after` begins straight with
    // the downloaded HTML (or a curl error), never with a blank line. A blank
    // line there is the `\r\r\n` double-newline regression from the editor.
    let first = after
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty());
    let ok = match first {
        Some(l) if l.contains("Example Domain") || l.contains("curl:") => true,
        _ => false,
    };
    ok && !after.starts_with("\n\n") && at_interactive_prompt(serial)
}

/// os-test basic smoke, stage 1: writable copy + thin TESTLIST make report
/// must finish (harness printed `pass_rate=`). Scope failure patterns to the
/// output after the echoed command. Do NOT fail on pass_rate < 80 — report
/// only; the setpwent stages below remain the hard libc gate.
fn interactive_ostest_prep_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    let echoed =
        "$ sh /lib/os-test/misc/ci-smoke-copy.sh /tmp/o && cd /tmp/o && make SUITES=basic TESTLIST=misc/ci-basic-smoke.tests report; echo PREP-RC=$?";
    if !tail.contains(echoed) || serial.contains("exception:") {
        return false;
    }
    let after = tail.rsplit_once(echoed).map(|(_, rest)| rest).unwrap_or("");
    // Harness finished: myos-report.sh always emits pass_rate=NN% (P/T).
    // Low pass rates are informational only — never a CI hard-fail here.
    after.contains("pass_rate=")
        && !after.contains("cannot create")
        && !after.contains("Read-only file system")
        && at_interactive_prompt(serial)
}

/// Hard-fail ostest prep when the guest already reported a non-zero PREP-RC
/// or a user fault (aarch64: data abort mid-suite → PREP-RC=139, no pass_rate).
fn interactive_ostest_prep_failed(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    let echoed =
        "$ sh /lib/os-test/misc/ci-smoke-copy.sh /tmp/o && cd /tmp/o && make SUITES=basic TESTLIST=misc/ci-basic-smoke.tests report; echo PREP-RC=$?";
    if !tail.contains(echoed) {
        return false;
    }
    let after = tail.rsplit_once(echoed).map(|(_, rest)| rest).unwrap_or("");
    if after.contains("[ WARN ] user fault") || after.contains("user panic") {
        return true;
    }
    // PREP-RC=0 is success; any other decoded RC means the suite aborted.
    for line in after.lines() {
        if let Some(rest) = line.strip_prefix("PREP-RC=") {
            if rest.trim() != "0" {
                return true;
            }
        }
    }
    false
}

/// os-test setpwent, stage 2: cat the produced .err/.out (after smoke subset). Pass = plain `SETPWENT-OK` (and never plain
/// `SETPWENT-FAIL`) after the echoed command; the quoted markers inside the
/// echo cannot collide with the plain ones. Also refuse "not found" from the
/// cat (missing .err/.out => prep did not actually produce them).
fn interactive_ostest_cat_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    let echoed = "$ cat out/basic/pwd/setpwent.err out/basic/pwd/setpwent.out";
    if !tail.contains(echoed) || serial.contains("exception:") {
        return false;
    }
    let after = tail.rsplit_once(echoed).map(|(_, rest)| rest).unwrap_or("");
    !after.contains("not found")
        && !after.contains("cannot create")
        && at_interactive_prompt(serial)
}

/// Verdict: plain `SETPWENT-OK` (and never plain `SETPWENT-FAIL`) after the
/// echoed command; the echoed line itself contains the marker text mid-line
/// (not newline-prefixed), so it cannot collide with the real output.
fn interactive_ostest_result_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    let echoed = "$ test -s out/basic/pwd/setpwent.out && echo SETPWENT-FAIL || echo SETPWENT-OK";
    if !tail.contains(echoed) || serial.contains("exception:") {
        return false;
    }
    let after = tail.rsplit_once(echoed).map(|(_, rest)| rest).unwrap_or("");
    after.contains("\nSETPWENT-OK") && !after.contains("\nSETPWENT-FAIL")
}

/// ^C interrupt test: a foreground `cat | cat` (right cat blocked on a kernel
/// pipe read, left cat on the console) was interrupted with VINTR (`0x03`).
/// Both children must have died (the pipe-blocked one via the signal-wakeable
/// pipe-read wait) and the shell must have survived (ignored SIGINT,, returning
/// to `$` — not respawned via getty/login.
fn interactive_interrupt_cmd_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    let echoed = "$ cat | cat";
    if !tail.contains(echoed) || serial.contains("exception:") {
        return false;
    }
    let after = tail.rsplit_once(echoed).map(|(_, rest)| rest).unwrap_or("");
    // No getty/login respawn:the shell itself survived the interrupt.
    if after.contains("login: ") {
        return false;
    }
    at_interactive_prompt(serial)
}

/// History seed for the arrow test: `echo histrecall_zz` must run and print
/// `histrecall_zz` so the following arrow command can recall it via Up.
fn interactive_arrow_seed_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    if serial.contains("exception:")
        || serial.contains("histrecall_zz: not found")
        || !tail.contains("$ echo histrecall_zz")
    {
        return false;
    }
    let after = tail
        .rsplit_once("$ echo histrecall_zz")
        .map(|(_, rest)| rest)
        .unwrap_or("");
    // The prompt's Enter must produce exactly one newline before the output:
    // a real double-CR (`\r\r\n`) or a split-CRLF normalize bug both show up
    // as a blank line, which makes `after` start with `\n` before `histrecall_zz`.
    let ok = after.starts_with("\nhistrecall_zz")
        && at_interactive_prompt(serial);
    ok
}

/// Arrow-key editing test: Up (ESC [ A) recalls the seeded history entry,
/// Left (ESC [ D) backs the cursor off the end, `3` inserts, Enter runs the
/// edited line -> `echo histrecall_z3z` prints `histrecall_z3z`. The needle
/// only appears if the emacs editor recognised the CSI bytes on the raw tty:
/// with a cooked/single-line shell the escaped sequence is swallowed (kernel
/// canonical mode) or printed as garbage and `histrecall_z3z` never runs.
fn interactive_arrow_edit_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    // The recalled `echo histrecall_zz` line is edited in place (Left + insert),
    // so the editor's echo carries redraw bytes (backspaces/`3`) rather than a
    // re-rendered contiguous `$ echo histrecall_z3z`. Only the *output* line
    // `histrecall_z3z` is a clean needle. (The no-blank-line assertion lives on
    // the seed, which is a plain typed command with a clean echo.)
    !serial.contains("exception:")
        && !tail.contains("histrecall_z3z: not found")
        && tail.contains("histrecall_z3z")
        && at_interactive_prompt(serial)
}

/// curl errored after the interactive command (e.g. `curl: (4) …`).
fn interactive_curl_after_failed(after: &str) -> bool {
    after.contains("not found")
        || after.contains("curl: (")
        || (after.contains("curl:")
            && (after.contains("error") || after.contains("failed") || after.contains("mbedtls:")))
}

/// Hard fail so we do not burn the full QEMU timeout after a printed curl error.
fn interactive_curl_cmd_failed(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    if !tail.contains(CURL_ECHO) {
        return false;
    }
    let after = tail.rsplit_once(CURL_ECHO).map(|(_, rest)| rest).unwrap_or("");
    interactive_curl_after_failed(after)
}

/// Hard fail so we do not burn the full QEMU timeout after a printed TLS error.
fn interactive_https_cmd_failed(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    if !tail.contains("$ http https://example.com/") {
        return false;
    }
    // Guest http client can page-fault after printing the body (seen on
    // riscv64: Example Domain in HTML, then instruction page fault sepc=0)
    // before `[ OK ] https` — fail fast instead of burning the QEMU budget.
    if tail.contains("[ WARN ] user fault") || tail.contains("user panic") {
        return true;
    }
    tail.contains("tls handshake fail")
        || tail.contains("tls err ")
        || tail.contains("dns resolve fail")
        || tail.contains("tcp connect timeout")
}



fn interactive_bs_ls_cmd_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    // Echo includes BS-space-BS; do not require a clean `$ /s/ls` substring.
    !serial.contains("exception:")
        && !serial.contains("user panic")
        && at_interactive_prompt(serial)
        && !tail.contains("/bin/sbase/ls: not found")
        && !tail.contains("x/bin/sbase/ls")
}

/// Enabled Cargo features, baked at compile time by build.rs via
/// `cargo:rustc-env=MYOS_FEATURES` (comma-joined, including `default` and
/// expanded members). It is NOT a runtime process env var — `cargo:rustc-env`
/// is only readable with the `env!()` compile-time macro, so never use
/// `std::env::var` here. Empty list means "default ports" (the fallback below
/// never weakens the tests on omission).
fn active_features() -> Vec<String> {
    env!("MYOS_FEATURES")
        .split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// True when the given feature (e.g. `port_tcc`) is in the active set. Falls
/// through (returns true) when MYOS_FEATURES is unset so default builds require
/// every port, matching pre-feature behavior.
fn port_enabled(feature: &str) -> bool {
    let fs = active_features();
    fs.is_empty() || fs.iter().any(|f| *f == feature)
}

/// Some heavy needles only print when their port is packed into the initramfs
/// (tcc, git). When that port feature is disabled, the needle must not be
/// required, or `--ci` would hang waiting for a marker that never appears.
fn needle_for_enabled_port(n: &str) -> bool {
    if n == "[ OK ] tcc" || n == "[ OK ] tcc std" {
        return port_enabled("port_tcc");
    }
    if n == "[ OK ] git" || n == "[ OK ] git commit" {
        // In boot-mini the harness types `heap mini`, which skips the git
        // stage entirely; those markers must not be required there.
        return port_enabled("port_git") && !ci_mini();
    }
    true
}

/// CI-only `/heap` carnival. All arches pass the same `heavy` needles
/// (`CI_NEEDLES_STD`) and must return to `$` after `[ OK ] smoke`.
fn interactive_heap_cmd_ok(serial: &str, heavy: &[&str]) -> bool {
    if !command_echoed(serial, "heap") || serial.contains("exception:") {
        return false;
    }
    for n in heavy.iter() {
        if !needle_for_enabled_port(*n) {
            continue;
        }
        if !serial.contains(*n) {
            return false;
        }
    }
    if !serial.contains("[ OK ] smoke") {
        return false;
    }
    at_interactive_prompt(serial)
}

/// Heap printed `[ OK ] smoke` and returned to `$`. Missing heavy needles can
/// fail immediately instead of waiting out the 180s timeout.
fn interactive_heap_returned(serial: &str) -> bool {
    command_echoed(serial, "heap") && serial.contains("[ OK ] smoke") && at_interactive_prompt(serial)
}

/// Listener command echoed and the runner-side ping/pong succeeded. The
/// smoke's own output goes to /tmp/listen.out (never the serial console).
fn interactive_listen_bg_ok(serial: &str) -> bool {
    if !LISTEN_PONGED.load(std::sync::atomic::Ordering::SeqCst) {
        return false;
    }
    command_echoed(serial, "/bin/etc/tcp_listen_smoke > /tmp/listen.out 2>&1 & echo $! > /tmp/listen.pid")
        && at_interactive_prompt(serial)
}

/// Redirected smoke output shows the accepted-connection round trip.
fn interactive_listen_cat_ok(serial: &str) -> bool {
    command_echoed(serial, "cat /tmp/listen.out")
        && serial.contains("[ OK ] listen")
        && at_interactive_prompt(serial)
}

/// Listen smoke torn down before the ^C interrupt stage.
fn interactive_listen_stop_ok(serial: &str) -> bool {
    command_echoed(serial, "kill $(cat /tmp/listen.pid) 2>/dev/null; echo LISTEN-STOP")
        && serial.contains("LISTEN-STOP")
        && at_interactive_prompt(serial)
}


fn interactive_dropbear_bg_ok(serial: &str) -> bool {
    if !SSH_SMOKED.load(std::sync::atomic::Ordering::SeqCst) {
        return false;
    }
    command_echoed(
        serial,
        "/bin/custom/dropbear -F -E -p 22 -r /etc/dropbear/ed25519_hostkey > /tmp/dropbear.out 2>&1 & echo $! > /tmp/dropbear.pid",
    ) && at_interactive_prompt(serial)
}

fn shell_cmd_result_ok(serial: &str, cmds: &[&[u8]], cmd_index: usize, extra: &[&str]) -> bool {
    match cmd_index {
        0 => interactive_unknown_cmd_ok(serial),
        1 => interactive_heap_cmd_ok(serial, extra),
        2 => interactive_ok_cmd_ok(serial),
        3 => interactive_echo_cmd_ok(serial),
        4 => interactive_pipe_cmd_ok(serial),
        5 => interactive_uutils_true_cmd_ok(serial),
        6 => interactive_sbase_echo_cmd_ok(serial),
        7 => interactive_sbase_ls_cmd_ok(serial),
        8 => interactive_bs_ls_cmd_ok(serial),
        9 => interactive_tmp_redir_ok(serial),
        10 => interactive_which_ls_cmd_ok(serial),
        11 => interactive_dns_cmd_ok(serial),
        // Full-mode-only stages matched by command content (indexes shift when
        // dropbear SSH smoke is inserted after curl).
        i if cmds[i] == CMD_HTTP => interactive_https_cmd_ok(serial),
        i if cmds[i] == CMD_CURL => interactive_curl_cmd_ok(serial),
        i if cmds[i] == CMD_DROPBEAR_BG => interactive_dropbear_bg_ok(serial),
        i if cmds[i] == CMD_OS_TEST_PREP => interactive_ostest_prep_ok(serial),
        i if cmds[i] == CMD_OS_TEST_CAT => interactive_ostest_cat_ok(serial),
        i if cmds[i] == CMD_OS_TEST_RESULT => interactive_ostest_result_ok(serial),
        i if cmds[i] == CMD_PTY => interactive_pty_cmd_ok(serial),
        i if cmds[i] == CMD_URANDOM => interactive_urandom_cmd_ok(serial),
        i if cmds[i] == CMD_DROPBEAR_STOP => {
            command_echoed(serial, "kill $(cat /tmp/dropbear.pid) 2>/dev/null; echo DROPBEAR-STOP")
                && serial.contains("DROPBEAR-STOP")
                && at_interactive_prompt(serial)
        }
        i if cmds[i] == CMD_LISTEN_BG => interactive_listen_bg_ok(serial),
        i if cmds[i] == CMD_LISTEN_CAT => interactive_listen_cat_ok(serial),
        i if cmds[i] == CMD_LISTEN_STOP => interactive_listen_stop_ok(serial),
        i if i == interrupt_cmd_idx(cmds) => interactive_interrupt_cmd_ok(serial),
        i if i == arrow_seed_idx(cmds) => interactive_arrow_seed_ok(serial),
        i if i == arrow_edit_idx(cmds) => interactive_arrow_edit_ok(serial),
        _ => false,
    }
}

fn shell_ready(serial: &str) -> bool {
    // Pre-prompt: slim `/ok` markers only. Heavy CI_NEEDLES_STD come from `heap`.
    serial_has_all_needles(serial, &[]) && at_interactive_prompt(serial)
}

const SHELL_TYPE_DELAY: Duration = Duration::from_millis(40);
const SHELL_CMD_DELAY: Duration = Duration::from_millis(100);
/// Wall-time bound while CMD_ARROW is active waiting for `histrecall_z3z`.
/// riscv64 #866 hung here until the GHA 90m cancel — fail ourselves instead.
const ARROW_STAGE_BOUND: Duration = Duration::from_secs(20);
/// Same idea for the histrecall *seed* WaitResult: if `echo histrecall_zz`
/// never satisfies the seed check, do not burn the full QEMU timeout.
const ARROW_SEED_STAGE_BOUND: Duration = Duration::from_secs(20);
/// Same idea for the ^C interrupt stage (`cat | cat` + VINTR).
const INTERRUPT_STAGE_BOUND: Duration = Duration::from_secs(30);
/// `which ls` mistyped as `which s` (serial drop) used to sit until QEMU 600s.
const WHICH_STAGE_BOUND: Duration = Duration::from_secs(30);
/// `echo pipe | cat` null-deref (FAR=0) used to leave the shell wedged with no
/// `$` until the 1800s QEMU wall (CI aarch64 boot-mini #35570070681 ~24m).
const PIPE_STAGE_BOUND: Duration = Duration::from_secs(30);
/// getty login typing (incl. delayed AP echo) — fail fast vs sticky `rroooo…`.
/// Clock starts only after `[ OK ] fork exec` (see wait loop): UEFI OVMF alone
/// can burn ~40s before that marker, so counting from harness start killed
/// WaitLogin with typed="" on CI boot-mini (run 34886420580).
const LOGIN_STAGE_BOUND: Duration = Duration::from_secs(45);

fn send_shell_byte(stdin: &mut ChildStdin, byte: u8) {
    stdin
        .write_all(&[byte])
        .expect("write shell byte to qemu stdin");
    stdin.flush().ok();
}

fn advance_shell_ci(
    stdin: &mut Option<ChildStdin>,
    cmds: &[&[u8]],
    stage: &mut ShellStage,
    cmd_index: &mut usize,
    typing: &mut usize,
    echo_stalls: &mut u32,
    interrupt_sent: &mut bool,
    acc: &str,
    extra: &[&str],
) {
    let Some(stdin) = stdin.as_mut() else {
        return;
    };
    match *stage {
        ShellStage::WaitLogin if login_prompt_ready(acc) => {
            *stage = ShellStage::TypingUser;
            *typing = 0;
        }
        ShellStage::TypingUser => {
            if *typing < CI_LOGIN_USER.len() {
                if *typing > 0 {
                    // Exact suffix sync on the current login line. Never resend:
                    // BSP UART drain already preserves RX bytes; resending on a
                    // delayed AP echo produced `login: rr` then sticky
                    // `rrooooo…` / `roootttt…` under -smp 4 (PR #150 stress).
                    let prefix = &CI_LOGIN_USER[..*typing];
                    // Newline is not echoed onto the login line — only sync printable.
                    let sync_prefix = if prefix.last() == Some(&b'\n') {
                        &prefix[..prefix.len() - 1]
                    } else {
                        prefix
                    };
                    match echo_sync_state(login_line_typed(acc), sync_prefix) {
                        EchoSync::Synced => {}
                        EchoSync::Waiting => {
                            *echo_stalls = echo_stalls.saturating_add(1);
                            std::thread::sleep(SHELL_TYPE_DELAY);
                            return;
                        }
                        EchoSync::Garble => {
                            // Clear the bad attempt and wait for a fresh prompt.
                            *echo_stalls = 0;
                            *typing = 0;
                            send_shell_byte(stdin, b'\n');
                            *stage = ShellStage::WaitLogin;
                            std::thread::sleep(SHELL_TYPE_DELAY);
                            return;
                        }
                    }
                }
                *echo_stalls = 0;
                send_shell_byte(stdin, CI_LOGIN_USER[*typing]);
                *typing += 1;
                std::thread::sleep(SHELL_TYPE_DELAY);
            } else {
                *echo_stalls = 0;
                *stage = ShellStage::WaitPassword;
            }
        }
        ShellStage::WaitPassword if password_prompt_ready(acc) => {
            *stage = ShellStage::TypingPass;
            *typing = 0;
        }
        ShellStage::TypingPass => {
            if *typing < CI_LOGIN_PASS.len() {
                send_shell_byte(stdin, CI_LOGIN_PASS[*typing]);
                *typing += 1;
                std::thread::sleep(SHELL_TYPE_DELAY);
            } else {
                *stage = ShellStage::WaitPrompt;
            }
        }
        ShellStage::WaitPrompt if shell_ready(acc) => {
            *stage = ShellStage::Typing;
            *cmd_index = 0;
            *typing = 0;
            *echo_stalls = 0;
        }
        ShellStage::Typing => {
            let cmd = cmds[*cmd_index];
            if *typing < cmd.len() {
                // Echo-sync printable prefixes so we never type ahead of a
                // starved AP's ECHO. Skip CSI/controls (arrow/backspace smoke).
                // Do NOT resend on stall: IRQ UART drain preserves bytes; a
                // resend duplicates and `contains`-style sync then sticky-loops.
                if *typing > 0 {
                    let prefix = &cmd[..*typing];
                    let syncable = prefix
                        .iter()
                        .all(|b| (0x20..=0x7e).contains(b) || *b == b'\t');
                    if syncable {
                        let sync_prefix = if prefix.last() == Some(&b'\n') {
                            &prefix[..prefix.len() - 1]
                        } else {
                            prefix
                        };
                        match echo_sync_state(shell_line_typed(acc), sync_prefix) {
                            EchoSync::Synced => {}
                            EchoSync::Waiting => {
                                *echo_stalls = echo_stalls.saturating_add(1);
                                std::thread::sleep(SHELL_TYPE_DELAY);
                                return;
                            }
                            EchoSync::Garble => {
                                // Kill the line (oksh/cooked VERASE won't help for
                                // extras mid-line); submit and let WaitResult fail
                                // fast rather than spam forever.
                                *echo_stalls = 0;
                                send_shell_byte(stdin, b'\n');
                                *typing = cmd.len();
                                *stage = ShellStage::WaitResult;
                                std::thread::sleep(SHELL_TYPE_DELAY);
                                return;
                            }
                        }
                    }
                }
                *echo_stalls = 0;
                send_shell_byte(stdin, cmd[*typing]);
                *typing += 1;
                std::thread::sleep(SHELL_TYPE_DELAY);
            } else {
                *echo_stalls = 0;
                *stage = ShellStage::WaitResult;
            }
        }
        ShellStage::WaitResult if *cmd_index == interrupt_cmd_idx(cmds) && !*interrupt_sent => {
            // Give the `cat` child a beat to fork/exec and block on console
            // read before sending VINTR (^C), so the signal lands on the child
            // (via INPUT_READER's pgid) rather than racing its startup.
            std::thread::sleep(Duration::from_millis(800));
            send_shell_byte(stdin, 0x03);
            *interrupt_sent = true;
        }
        ShellStage::WaitResult if cmds[*cmd_index] == CMD_LISTEN_BG && !LISTEN_PONGED.load(std::sync::atomic::Ordering::SeqCst) => {
            // Smoke output is redirected to /tmp/listen.out (the `cat` stage
            // surfaces it), so the serial console never shows the announce.
            // Just poke the listener until it accepts (bounded below).
            if LISTEN_STAGE_START.get().is_none() {
                let _ = LISTEN_STAGE_START.set(std::time::Instant::now());
            }
            if poke_listener() {
                LISTEN_PONGED.store(true, std::sync::atomic::Ordering::SeqCst);
                return;
            }
            if LISTEN_STAGE_START.get().unwrap().elapsed() > LISTEN_STAGE_BOUND {
                // Surface the smoke's own output (redirected to
                // /tmp/listen.out) before bailing.
                for ch in "cat /tmp/listen.out\n".bytes() {
                    send_shell_byte(stdin, ch);
                }
                std::thread::sleep(Duration::from_secs(4));
                let fresh = SERIAL_ACC
                    .get()
                    .and_then(|a| a.lock().ok().map(|g| g.clone()))
                    .unwrap_or_default();
                eprintln!(
                    "error: listen smoke never completed within {LISTEN_STAGE_BOUND:?}; listen.out + last serial:\n---\n{}\n---",
                    fresh.chars().rev().take(4000).collect::<String>().chars().rev().collect::<String>()
                );
                std::process::exit(1);
            }
            return;
        }
        ShellStage::WaitResult if cmds[*cmd_index] == CMD_DROPBEAR_BG
            && !SSH_SMOKED.load(std::sync::atomic::Ordering::SeqCst) =>
        {
            // Guest sshd is backgrounded with output redirected; host clients
            // connect via hostfwd. Kick a worker once and wait for both
            // sequential sessions to exit cleanly. The stage clock starts
            // inside the worker *after* ensure_host_ssh (apt) so install
            // time does not burn the connect/retry budget.
            start_ssh_smoke_worker();
            if SSH_DONE.load(std::sync::atomic::Ordering::SeqCst) {
                if SSH_SMOKED.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                let err = SSH_ERR
                    .lock()
                    .map(|g| g.clone())
                    .unwrap_or_else(|_| "ssh smoke failed".into());
                for ch in "cat /tmp/dropbear.out\n".bytes() {
                    send_shell_byte(stdin, ch);
                }
                std::thread::sleep(Duration::from_secs(3));
                let fresh = SERIAL_ACC
                    .get()
                    .and_then(|a| a.lock().ok().map(|g| g.clone()))
                    .unwrap_or_default();
                eprintln!(
                    "error: dropbear SSH smoke failed ({err}); dropbear.out + last serial:\n---\n{}\n---",
                    fresh.chars().rev().take(4000).collect::<String>().chars().rev().collect::<String>()
                );
                std::process::exit(1);
            }
            if SSH_STAGE_START
                .get()
                .is_some_and(|t| t.elapsed() > SSH_STAGE_BOUND)
            {
                for ch in "cat /tmp/dropbear.out\n".bytes() {
                    send_shell_byte(stdin, ch);
                }
                std::thread::sleep(Duration::from_secs(3));
                let fresh = SERIAL_ACC
                    .get()
                    .and_then(|a| a.lock().ok().map(|g| g.clone()))
                    .unwrap_or_default();
                let err = SSH_ERR
                    .lock()
                    .map(|g| g.clone())
                    .unwrap_or_default();
                eprintln!(
                    "error: dropbear SSH smoke never completed within {SSH_STAGE_BOUND:?} ({err}); dropbear.out + last serial:\n---\n{}\n---",
                    fresh.chars().rev().take(4000).collect::<String>().chars().rev().collect::<String>()
                );
                std::process::exit(1);
            }
            return;
        }
        ShellStage::WaitResult if shell_cmd_result_ok(acc, cmds, *cmd_index, extra) => {
            *cmd_index += 1;
            if *cmd_index >= cmds.len() {
                *stage = ShellStage::Done;
            } else {
                *typing = 0;
                *echo_stalls = 0;
                std::thread::sleep(SHELL_CMD_DELAY);
                *stage = ShellStage::Typing;
            }
        }
        ShellStage::WaitLogin
        | ShellStage::WaitPassword
        | ShellStage::WaitPrompt
        | ShellStage::WaitResult
        | ShellStage::Done => {}
    }
}

fn ci_complete(acc: &str, extra: &[&str], expect: &CiExpect, stage: ShellStage) -> bool {
    if !serial_has_all_needles(acc, extra) {
        return false;
    }
    if expect.shell_ci {
        stage == ShellStage::Done
    } else {
        true
    }
}

fn wait_ci(mut child: Child, expect: CiExpect, extra_needles: &[&str]) {
    let mut stderr = child.stderr.take().expect("qemu stderr");
    let mut shell_stdin = if expect.shell_ci {
        child.stdin.take()
    } else {
        None
    };

    std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        loop {
            match stderr.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => eprint!("{}", String::from_utf8_lossy(&buf[..n])),
                Err(_) => break,
            }
        }
    });

    let serial_acc = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let _ = SERIAL_ACC.set(serial_acc.clone());
    let acc_reader = serial_acc.clone();

    let mut stdout = child.stdout.take().expect("qemu stdout");
    let reader_handle = std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        // Carry a trailing CR across read() chunks. Naively doing
        // replace("\r\n","\n").replace('\r','\n') per chunk turns a split
        // CRLF (`…\r` then `\n…`) into `\n\n`, which falsely trips the
        // histrecall seed blank-line check on bios (CI #34804761142).
        let mut pending_cr = false;
        loop {
            match stdout.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
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
                    // QEMU -serial stdio often delivers CRLF; status needles
                    // must not fail on CR vs LF. Normalize CR to LF.
                    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
                    acc_reader.lock().unwrap().push_str(&normalized);
                }
                Err(_) => break,
            }
        }
    });

    let cmds = ci_shell_commands();
    let mini = ci_mini();
    let started = Instant::now();
    let mut timed_out = false;
    let mut killed_for_needles = false;
    let mut shell_stage = ShellStage::WaitLogin;
    let mut shell_cmd_index = 0usize;
    let mut typing = 0usize;
    let mut echo_stalls = 0u32;
    let mut interrupt_sent = false;
    // Wall clock for fail-fast bounds on interrupt / arrow WaitResult stages.
    let mut interrupt_wait_started: Option<Instant> = None;
    let mut arrow_seed_wait_started: Option<Instant> = None;
    let mut arrow_wait_started: Option<Instant> = None;
    let mut which_wait_started: Option<Instant> = None;
    let mut pipe_wait_started: Option<Instant> = None;
    let mut login_wait_started: Option<Instant> = None;
    let status = loop {
        {
            let acc = serial_acc.lock().unwrap().clone();
            if expect.shell_ci {
                advance_shell_ci(
                    &mut shell_stdin,
                    &cmds,
                    &mut shell_stage,
                    &mut shell_cmd_index,
                    &mut typing,
                    &mut echo_stalls,
                    &mut interrupt_sent,
                    &acc,
                    extra_needles,
                );
                // `/heap` always prints `[ OK ] smoke` after tcc exits (even on
                // JIT failure). Don't sit on the 180s timeout for `[ OK ] tcc`.
                if shell_stage == ShellStage::WaitResult
                    && shell_cmd_index == 1
                    && interactive_heap_returned(&acc)
                    && !interactive_heap_cmd_ok(&acc, extra_needles)
                {
                    let _ = child.kill();
                    break child.wait().expect("wait after heap fail-fast kill");
                }
                // Heap exited back to `$` without `[ OK ] smoke` (e.g. riscv
                // ripgrep sepc=0 → `exit_code(1)`). Fail fast vs QEMU 600s.
                if shell_stage == ShellStage::WaitResult
                    && shell_cmd_index == 1
                    && command_echoed(&acc, "heap")
                    && at_interactive_prompt(&acc)
                    && !acc.contains("[ OK ] smoke")
                {
                    eprintln!(
                        "error: interactive `heap` returned to prompt without `[ OK ] smoke`"
                    );
                    let _ = child.kill();
                    break child.wait().expect("wait after heap early-exit kill");
                }
                // Any interactive WaitResult that prints a user fault / panic:
                // kill immediately. aarch64 `echo pipe | cat` FAR=0 left the
                // shell wedged with no `$` for ~24m under the old 1800s wall.
                if shell_stage == ShellStage::WaitResult
                    && (acc.contains("[ WARN ] user fault") || acc.contains("user panic"))
                {
                    eprintln!(
                        "error: user fault/panic during interactive cmd {shell_cmd_index} — fail-fast"
                    );
                    let _ = child.kill();
                    break child.wait().expect("wait after user-fault fail-fast kill");
                }
                // Pipe stage bound: even without a printed WARN, a wedged
                // pipeline must not burn the mini 240s / full 1800s budget.
                if shell_stage == ShellStage::WaitResult && cmds.get(shell_cmd_index) == Some(&CMD_PIPE)
                {
                    let started_at = pipe_wait_started.get_or_insert_with(Instant::now);
                    if started_at.elapsed() > PIPE_STAGE_BOUND && !interactive_pipe_cmd_ok(&acc)
                    {
                        eprintln!(
                            "error: pipe stage timed out after {:?} (want `$ echo pipe | cat` then `pipe`)",
                            PIPE_STAGE_BOUND
                        );
                        let _ = child.kill();
                        break child.wait().expect("wait after pipe timeout kill");
                    }
                } else {
                    pipe_wait_started = None;
                }
                // HTTPS: printed tls/dns/tcp failure — don't burn the 180s timeout.
                // Index 12 == `http https://example.com/` (full mode only; mini
                // drops the HTTPS/curl smokes and 12 is the interrupt test).
                if shell_stage == ShellStage::WaitResult
                    && !mini
                    && cmds.get(shell_cmd_index) == Some(&CMD_HTTP)
                    && interactive_https_cmd_failed(&acc)
                {
                    let _ = child.kill();
                    break child.wait().expect("wait after https fail-fast kill");
                }
                // curl: back at `$` with `curl: (N)` and no Example Domain —
                // don't wait 180s. Mid-retry errors must not kill QEMU early.
                // Index 13 == interactive curl HTTPS smoke (full mode only).
                if shell_stage == ShellStage::WaitResult
                    && !mini
                    && cmds.get(shell_cmd_index) == Some(&CMD_CURL)
                    && interactive_curl_cmd_failed(&acc)
                {
                    let _ = child.kill();
                    break child.wait().expect("wait after curl fail-fast kill");
                }
                if shell_stage == ShellStage::WaitResult
                    && !mini
                    && cmds.get(shell_cmd_index) == Some(&CMD_OS_TEST_PREP)
                    && interactive_ostest_prep_failed(&acc)
                {
                    let _ = child.kill();
                    break child.wait().expect("wait after ostest fail-fast kill");
                }
                // Login: never burn 600s on sticky UART echo (`rroooo…`).
                // Do not start the bound until late boot — OVMF + Limine on UEFI
                // often exceeds 45s before getty; the sticky-key failure mode is
                // post-prompt typing, not firmware wait (CI #34886420580).
                if matches!(
                    shell_stage,
                    ShellStage::WaitLogin
                        | ShellStage::TypingUser
                        | ShellStage::WaitPassword
                        | ShellStage::TypingPass
                ) {
                    let late_boot = acc.contains("[ OK ] fork exec") || login_prompt_ready(&acc);
                    if late_boot {
                        let started_at = login_wait_started.get_or_insert_with(Instant::now);
                        if started_at.elapsed() > LOGIN_STAGE_BOUND {
                            eprintln!(
                                "error: login stage timed out after {:?} (stage={shell_stage:?}, typed={:?})",
                                LOGIN_STAGE_BOUND,
                                login_line_typed(&acc)
                            );
                            let _ = child.kill();
                            break child.wait().expect("wait after login fail-fast kill");
                        }
                    }
                } else {
                    login_wait_started = None;
                }
                // `which ls` (cmd 10): serial drop (`which s`) or PATH miss used
                // to sit in WaitResult until the 600s QEMU timeout (CI #34824642315).
                if shell_stage == ShellStage::WaitResult && shell_cmd_index == 10 {
                    let tail = interactive_tail(&acc);
                    let which_hard_fail = tail.contains("not an external command")
                        || (tail.contains("$ which ")
                            && !tail.contains("$ which ls")
                            && at_interactive_prompt(&acc));
                    if which_hard_fail {
                        eprintln!(
                            "error: interactive `which ls` failed early (want absolute PATH hit; serial drop or PATH miss)"
                        );
                        let _ = child.kill();
                        break child.wait().expect("wait after which fail-fast kill");
                    }
                    let started_at = which_wait_started.get_or_insert_with(Instant::now);
                    if started_at.elapsed() > WHICH_STAGE_BOUND
                        && !interactive_which_ls_cmd_ok(&acc)
                    {
                        eprintln!(
                            "error: which stage timed out after {:?} (want `$ which ls` → `/…/ls`)",
                            WHICH_STAGE_BOUND
                        );
                        let _ = child.kill();
                        break child.wait().expect("wait after which timeout kill");
                    }
                } else {
                    which_wait_started = None;
                }
                // Interrupt stage: if `cat | cat` + ^C never returns to `$`, do
                // not sit until QEMU/GHA timeout (same idea as curl/TLS hard-fail).
                if shell_stage == ShellStage::WaitResult
                    && shell_cmd_index == interrupt_cmd_idx(&cmds)
                {
                    let started_at = interrupt_wait_started.get_or_insert_with(Instant::now);
                    if started_at.elapsed() > INTERRUPT_STAGE_BOUND
                        && !interactive_interrupt_cmd_ok(&acc)
                    {
                        eprintln!(
                            "error: interrupt stage timed out after {:?} (want prompt after ^C on `cat | cat`)",
                            INTERRUPT_STAGE_BOUND
                        );
                        let _ = child.kill();
                        break child.wait().expect("wait after interrupt fail-fast kill");
                    }
                } else {
                    interrupt_wait_started = None;
                }
                // Histrecall seed: do not burn the full QEMU timeout when the
                // seed check never passes (CI #34804761142 bios sat 600s).
                if shell_stage == ShellStage::WaitResult
                    && shell_cmd_index == arrow_seed_idx(&cmds)
                {
                    let started_at = arrow_seed_wait_started.get_or_insert_with(Instant::now);
                    if started_at.elapsed() > ARROW_SEED_STAGE_BOUND
                        && !interactive_arrow_seed_ok(&acc)
                    {
                        eprintln!(
                            "error: arrow history seed timed out after {:?} (want `histrecall_zz`, no blank line)",
                            ARROW_SEED_STAGE_BOUND
                        );
                        let _ = child.kill();
                        break child.wait().expect("wait after arrow-seed fail-fast kill");
                    }
                } else {
                    arrow_seed_wait_started = None;
                }
                // Arrow/histrecall: after CMD_ARROW, require `histrecall_z3z`
                // within a short bound. #866 riscv64 hung here after a good
                // smoke + SETPWENT-OK until the 90m GHA cancel.
                if shell_stage == ShellStage::WaitResult
                    && shell_cmd_index == arrow_edit_idx(&cmds)
                {
                    let started_at = arrow_wait_started.get_or_insert_with(Instant::now);
                    if started_at.elapsed() > ARROW_STAGE_BOUND
                        && !interactive_arrow_edit_ok(&acc)
                    {
                        eprintln!(
                            "error: arrow/histrecall stage timed out after {:?} (want `histrecall_z3z` after Up/Left/insert)",
                            ARROW_STAGE_BOUND
                        );
                        let _ = child.kill();
                        break child.wait().expect("wait after arrow fail-fast kill");
                    }
                } else {
                    arrow_wait_started = None;
                }
            }
            if ci_complete(&acc, extra_needles, &expect, shell_stage) {
                let _ = child.kill();
                killed_for_needles = true;
                break child.wait().expect("wait after kill");
            }
        }
        match child.try_wait().expect("failed to wait on qemu") {
            Some(status) => break status,
            None if started.elapsed() > expect.timeout => {
                let _ = child.kill();
                timed_out = true;
                break child.wait().expect("wait after kill");
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };

    reader_handle.join().expect("serial reader thread");

    let serial = serial_acc.lock().unwrap().clone();
    if !serial_has_all_needles(&serial, extra_needles) {
        if timed_out {
            eprintln!("error: QEMU timed out after {:?}", expect.timeout);
            if serial.is_empty() {
                eprintln!("error: serial was empty at timeout");
            }
        }
        for needle in CI_NEEDLES.iter().chain(extra_needles.iter()) {
            // Only report needles for ports that are actually enabled; a disabled
            // port (e.g. git/vim excluded by feature) prints no marker by design.
            if !needle_for_enabled_port(*needle) {
                continue;
            }
            if !serial.contains(*needle) {
                eprintln!("error: serial output did not contain {needle:?}");
            }
        }
        std::process::exit(1);
    }
    if expect.shell_ci && shell_stage != ShellStage::Done {
        if timed_out {
            eprintln!("error: QEMU timed out after {:?}", expect.timeout);
        }
        eprintln!("error: shell CI stage was {shell_stage:?} (cmd {shell_cmd_index})");
        {
            // Escaped byte-level dump of the serial tail at failure: needles
            // match on the exact `acc` bytes, so the raw tail (backspaces,
            // CR/LF, editor redraw bytes) is what decides pass/fail.
            let tail: String = serial
                .chars()
                .rev()
                .take(240)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            eprintln!("debug: serial tail (escaped): {tail:?}");
            eprintln!(
                "debug: at_interactive_prompt={} arrow_edit_ok={}",
                at_interactive_prompt(&serial),
                interactive_arrow_edit_ok(&serial)
            );
        }
        if matches!(shell_stage, ShellStage::WaitLogin | ShellStage::TypingUser)
            && !interactive_tail(&serial).contains("login: ")
        {
            eprintln!("error: serial never reached getty `login: ` prompt");
        }
        if matches!(
            shell_stage,
            ShellStage::WaitPassword | ShellStage::TypingPass
        ) && !password_prompt_ready(&serial)
        {
            eprintln!("error: serial never reached login `Password:` prompt after `root`");
        }
        if !at_interactive_prompt(&serial) && !command_echoed(&serial, "nosuchcmd") {
            eprintln!("error: serial never reached interactive `$` prompt");
        }
        if shell_cmd_index == 0 && !interactive_unknown_cmd_ok(&serial) {
            if !serial.contains(CI_SHELL_UNKNOWN_CMD) {
                eprintln!("error: serial output did not contain {CI_SHELL_UNKNOWN_CMD:?}");
            } else if tail_has_hex_received(&serial) {
                eprintln!("error: shell reported non-printable stdin (hex escapes in received:)");
            } else {
                eprintln!("error: {CI_SHELL_UNKNOWN_CMD:?} did not follow interactive `nosuchcmd`");
            }
        }
        if shell_cmd_index >= 1
            && shell_cmd_index < 2
            && !interactive_heap_cmd_ok(&serial, extra_needles)
        {
            if !command_echoed(&serial, "heap") {
                eprintln!("error: serial did not echo `$ heap` at the interactive prompt");
            } else if serial.contains("exception:") {
                eprintln!("error: interactive `heap` triggered a CPU exception");
            } else {
                for needle in extra_needles {
                    if !serial.contains(*needle) {
                        eprintln!("error: CI-only `/heap` did not print {needle:?}");
                    }
                }
                if !serial.contains("[ OK ] smoke") {
                    eprintln!("error: CI-only `/heap` did not print \"[ OK ] smoke\"");
                }
                if !at_interactive_prompt(&serial) {
                    eprintln!("error: shell did not return to `$` after interactive `heap`");
                }
            }
        }
        if shell_cmd_index >= 2 && shell_cmd_index < 3 && !interactive_ok_cmd_ok(&serial) {
            if !command_echoed(&serial, "ok") {
                eprintln!("error: serial did not echo `$ ok` at the interactive prompt");
            }
            if tail_has_hex_received(&serial) {
                eprintln!("error: shell reported non-printable input (hex escapes in received:)");
            }
        }
        if shell_cmd_index >= 3 && shell_cmd_index < 4 && !interactive_echo_cmd_ok(&serial) {
            if !command_echoed(&serial, "echo test") {
                eprintln!("error: serial did not echo `$ echo test` at the interactive prompt");
            } else if serial.contains("exception:") {
                eprintln!("error: interactive `echo test` triggered a CPU exception");
            } else if !at_interactive_prompt(&serial) {
                eprintln!("error: shell did not return to `$` after interactive `echo test`");
            } else {
                eprintln!("error: interactive `echo test` did not print `test`");
            }
        }
        if shell_cmd_index >= 4 && shell_cmd_index < 5 && !interactive_pipe_cmd_ok(&serial) {
            if serial.contains("[ WARN ] user fault") || serial.contains("user panic") {
                eprintln!(
                    "error: interactive `echo pipe | cat` hit user fault/panic (want `pipe` then `$`)"
                );
            } else if !at_interactive_prompt(&serial) {
                eprintln!(
                    "error: shell did not return to `$` after interactive `echo pipe | cat`"
                );
            } else {
                eprintln!(
                    "error: interactive `echo pipe | cat` failed (want `$ echo pipe | cat` then `pipe`)"
                );
            }
        }
        if shell_cmd_index >= 5 && shell_cmd_index < 6 && !interactive_uutils_true_cmd_ok(&serial) {
            eprintln!("error: interactive `/bin/coreutils/true` failed (want `$ /bin/coreutils/true` then `$` prompt)");
        }
        if shell_cmd_index >= 6 && shell_cmd_index < 7 && !interactive_sbase_echo_cmd_ok(&serial) {
            if !command_echoed(&serial, "/bin/sbase/echo hi") {
                eprintln!("error: serial did not echo `$ /bin/sbase/echo hi` at the interactive prompt");
            } else if serial.contains("exception:") {
                eprintln!("error: interactive `/bin/sbase/echo hi` triggered a CPU exception");
            } else if !at_interactive_prompt(&serial) {
                eprintln!("error: shell did not return to `$` after interactive `/bin/sbase/echo hi`");
            } else {
                eprintln!("error: interactive `/bin/sbase/echo hi` did not print `hi`");
            }
        }
        if shell_cmd_index >= 7 && shell_cmd_index < 8 && !interactive_sbase_ls_cmd_ok(&serial) {
            eprintln!("error: interactive `/bin/sbase/ls` failed (want `$ /bin/sbase/ls` then `$` prompt)");
        }
        if shell_cmd_index >= 8 && shell_cmd_index < 9 && !interactive_bs_ls_cmd_ok(&serial) {
            eprintln!(
                "error: interactive `x<BS>/s/ls` failed (canonical backspace must yield `/s/ls`)"
            );
        }
        if shell_cmd_index == 9 && !interactive_tmp_redir_ok(&serial) {
            eprintln!(
                "error: interactive `echo test > /tmp/aaa; cat /tmp/aaa` failed (want `test`, no cannot create)"
            );
        }
        if shell_cmd_index == 10 && !interactive_which_ls_cmd_ok(&serial) {
            eprintln!(
                "error: interactive `which ls` failed (want absolute PATH hit like `/bin/sbase/ls`, not `not an external command`)"
            );
        }
        if shell_cmd_index == 11 && !interactive_dns_cmd_ok(&serial) {
            if !command_echoed(&serial, "dns www.google.com") {
                eprintln!("error: serial did not echo `$ dns www.google.com` at the interactive prompt");
            } else if serial.contains("exception:") {
                eprintln!("error: interactive `dns www.google.com` triggered a CPU exception");
            } else if !at_interactive_prompt(&serial) {
                eprintln!("error: shell did not return to `$` after interactive `dns www.google.com`");
            } else {
                eprintln!("error: interactive `dns www.google.com` did not print `IP: x.x.x.x` and `[ OK ] dns`");
            }
        }
        if cmds.get(shell_cmd_index) == Some(&CMD_CURL) && !interactive_curl_cmd_ok(&serial) {
            if !serial.contains(CURL_ECHO) {
                eprintln!("error: serial did not echo the curl HTTPS command on one clean line at the interactive prompt");
            } else if serial.contains("exception:") {
                eprintln!("error: interactive curl triggered a CPU exception");
            } else if interactive_curl_cmd_failed(&serial) {
                eprintln!("error: interactive curl failed (curl printed an error)");
            } else {
                eprintln!("error: interactive curl did not print `Example Domain` and return to `$`");
            }
            std::process::exit(1);
        }
        if cmds.get(shell_cmd_index) == Some(&CMD_HTTP) && !interactive_https_cmd_ok(&serial) {
            if !command_echoed(&serial, "http https://example.com/") {
                eprintln!("error: serial did not echo `$ http https://example.com/` at the interactive prompt");
            } else if serial.contains("exception:") {
                eprintln!("error: interactive `http https://example.com/` triggered a CPU exception");
            } else if interactive_https_cmd_failed(&serial) {
                eprintln!("error: interactive HTTPS failed (tls/dns/tcp error printed)");
            } else if !at_interactive_prompt(&serial) {
                eprintln!("error: shell did not return to `$` after interactive HTTPS");
            } else {
                eprintln!("error: interactive HTTPS did not print `Example Domain` and `[ OK ] https`");
            }
            std::process::exit(1);
        }
        if cmds.get(shell_cmd_index) == Some(&CMD_OS_TEST_PREP) && !interactive_ostest_prep_ok(&serial) {
            if interactive_ostest_prep_failed(&serial) {
                eprintln!(
                    "error: os-test basic smoke aborted (user fault or PREP-RC!=0)"
                );
            } else {
                eprintln!(
                    "error: os-test basic smoke (ci-smoke-copy + TESTLIST make report) did not finish (want pass_rate= line, then `$`)"
                );
            }
            std::process::exit(1);
        }
        if cmds.get(shell_cmd_index) == Some(&CMD_OS_TEST_CAT) && !interactive_ostest_cat_ok(&serial) {
            eprintln!(
                "error: os-test setpwent cat stage failed (missing out/basic/pwd/setpwent.err/.out?)"
            );
            std::process::exit(1);
        }
        if cmds.get(shell_cmd_index) == Some(&CMD_OS_TEST_RESULT) && !interactive_ostest_result_ok(&serial) {
            eprintln!(
                "error: os-test setpwent gate failed (want SETPWENT-OK; .out must be empty on pass)"
            );
            std::process::exit(1);
        }
        if cmds.get(shell_cmd_index) == Some(&CMD_PTY) && !interactive_pty_cmd_ok(&serial) {
            if !command_echoed(&serial, "/bin/etc/pty_smoke") {
                eprintln!("error: serial did not echo `$ /bin/etc/pty_smoke` at the interactive prompt");
            } else if serial.contains("exception:") {
                eprintln!("error: interactive pty_smoke triggered a CPU exception");
            } else if !at_interactive_prompt(&serial) {
                eprintln!("error: shell did not return to `$` after interactive pty_smoke");
            } else {
                eprintln!("error: interactive pty_smoke did not print `[ OK ] s2 EIO`");
            }
            std::process::exit(1);
        }
        if cmds.get(shell_cmd_index) == Some(&CMD_URANDOM) && !interactive_urandom_cmd_ok(&serial) {
            if !command_echoed(&serial, "/bin/etc/urandom_smoke") {
                eprintln!("error: serial did not echo `$ /bin/etc/urandom_smoke` at the interactive prompt");
            } else if serial.contains("exception:") {
                eprintln!("error: interactive urandom_smoke triggered a CPU exception");
            } else if !at_interactive_prompt(&serial) {
                eprintln!("error: shell did not return to `$` after interactive urandom_smoke");
            } else {
                eprintln!("error: interactive urandom_smoke did not print `[ OK ] urandom`");
            }
            std::process::exit(1);
        }
        if cmds.get(shell_cmd_index) == Some(&CMD_DROPBEAR_BG)
            && !interactive_dropbear_bg_ok(&serial)
        {
            let err = SSH_ERR
                .lock()
                .map(|g| g.clone())
                .unwrap_or_default();
            if !SSH_SMOKED.load(std::sync::atomic::Ordering::SeqCst) {
                eprintln!(
                    "error: dropbear SSH smoke failed — need two sequential host SSH clients with clean exit-status (last error: {err})"
                );
            } else if !command_echoed(
                &serial,
                "/bin/custom/dropbear -F -E -p 22 -r /etc/dropbear/ed25519_hostkey > /tmp/dropbear.out 2>&1 &",
            ) {
                eprintln!("error: serial did not echo dropbear start command");
            } else if !at_interactive_prompt(&serial) {
                eprintln!("error: shell did not return to `$` after dropbear SSH smoke");
            } else {
                eprintln!("error: dropbear SSH smoke incomplete");
            }
            std::process::exit(1);
        }
        if shell_cmd_index == interrupt_cmd_idx(&cmds) && !interactive_interrupt_cmd_ok(&serial) {
            if !command_echoed(&serial, "cat | cat") {
                eprintln!("error: serial did not echo `$ cat | cat` at the interactive prompt");
            } else if serial.contains("exception:") {
                eprintln!("error: interactive Ctrl+C on `cat | cat` triggered a CPU exception");
            } else if serial.contains("login: ") {
                eprintln!("error: shell did not survive Ctrl+C (getty/login respawned — SIGINT killed the shell)");
            } else if !at_interactive_prompt(&serial) {
                eprintln!("error: shell did not return to `$` after Ctrl+C interrupt of `cat | cat`");
            } else {
                eprintln!("error: Ctrl+C did not interrupt the foreground `cat | cat`");
            }
            std::process::exit(1);
        }
        if shell_cmd_index == arrow_seed_idx(&cmds) && !interactive_arrow_seed_ok(&serial) {
            eprintln!(
                "error: arrow history seed `echo histrecall_zz` failed (want `histrecall_zz`, no exception)"
            );
        }
        if shell_cmd_index == arrow_edit_idx(&cmds) && !interactive_arrow_edit_ok(&serial) {
            if !serial.contains("histrecall_z3z")
                && !serial.contains("histrecall_zz3")
                && !serial.contains("histrecall_zz: not found")
            {
                eprintln!(
                    "error: arrow keys dead at the prompt — Up/Left did not recall+edit `echo histrecall_zz`"
                );
            } else {
                eprintln!(
                    "error: arrow recall/insert produced unexpected argv (want `echo histrecall_z3z`)"
                );
            }
        }
        std::process::exit(1);
    }
    if expect.qemu_debug_exit && !timed_out && !killed_for_needles {
        if status.code() != Some(QEMU_SUCCESS_STATUS) {
            eprintln!(
                "error: unexpected QEMU exit status {status:?} (want {QEMU_SUCCESS_STATUS} from isa-debug-exit)"
            );
            std::process::exit(1);
        }
    }
}