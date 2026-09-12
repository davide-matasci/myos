use std::io::Write;
use std::process::ChildStdin;

struct CiExpect {
    timeout: Duration,
    qemu_debug_exit: bool,
    /// Type commands at the interactive `$` prompt via `-serial stdio`.
    shell_ci: bool,
}

const CI_NEEDLES: [&str; 32] = [
    "Hello from myos",
    "[ OK ] heap",
    "[ OK ] interrupts",
    "task a",
    "task b",
    "[ OK ] scheduler",
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
// HTTPS GET (requires network + wall clock + mbedtls).
const CMD_HTTP: &[u8] = b"http https://example.com/\n";
// curl over userspace sockets + mbedtls (same URL as https smoke).
const CMD_CURL: &[u8] = b"curl -fsS --connect-timeout 30 --max-time 90 -o /tmp/curl-ex.html https://example.com/; cat /tmp/curl-ex.html\n";

/// os-test regression (full boot only). `basic/pwd/setpwent` must compile AND
/// run through the REAL harness path: make invokes misc/myos-run.sh, which
/// links with tcc against the packed newlib sysroot (libc.a + libm.a, the
/// #138 fix) and runs the binary. /lib/os-test is read-only (initramfs), so
/// the make target needs a writable copy first. Kept as two commands so each
/// typed line stays a single clean echo on the 160-col console.
const CMD_OS_TEST_PREP: &[u8] =
    b"cp -r /lib/os-test /tmp/o && cd /tmp/o && make out/basic/pwd/setpwent.out; echo PREP-RC=$?\n";
/// Success shape per misc/myos-run.sh: on pass, .err is empty AND .out is
/// empty (setpwent returns 0 silently); both failure modes ("compile_error"
/// and "exit: N") leave .out non-empty, and a botched compile can leave .err
/// non-empty too. `ls` first so a missing out/ tree is visible on serial.
/// The quotes in SETPWENT-"FAIL"/SETPWENT-"OK" keep the echoed command line
/// from ever containing the plain markers the checker matches on output.
const CMD_OS_TEST_RESULT: &[u8] = b"ls out/basic/pwd; cat out/basic/pwd/setpwent.err out/basic/pwd/setpwent.out; test -s out/basic/pwd/setpwent.out -o -s out/basic/pwd/setpwent.err && echo SETPWENT-\"FAIL\" || echo SETPWENT-\"OK\"\n";
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
        // os-test setpwent regression: real make -> myos-run.sh -> tcc link,
        // then cat the .err/.out and verdict marker (full boot only; too slow
        // for the boot-mini window).
        cmds.push(CMD_OS_TEST_PREP);
        cmds.push(CMD_OS_TEST_RESULT);
    }
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
    serial.contains("[ OK ] fork exec") && interactive_tail(serial).contains("login: ")
}

fn password_prompt_ready(serial: &str) -> bool {
    interactive_tail(serial).contains("Password:")
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

/// os-test setpwent, stage 1: writable copy + targeted make must finish with
/// a clean exit (no make error, no fs failure, no missing commands). Scope
/// failure patterns to the output after the echoed command so earlier stages
/// (e.g. nosuchcmd) can't poison this check.
fn interactive_ostest_prep_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    let echoed = "$ cp -r /lib/os-test /tmp/o && cd /tmp/o && make out/basic/pwd/setpwent.out; echo PREP-RC=$?";
    if !tail.contains(echoed) || serial.contains("exception:") {
        return false;
    }
    let after = tail.rsplit_once(echoed).map(|(_, rest)| rest).unwrap_or("");
    // A make failure is NOT a prep-stage failure: PREP-RC plus the follow-up
    // cat turn the actual tcc/harness error into the stage verdict.
    !after.contains("cannot create")
        && !after.contains("not found")
        && !after.contains("Read-only file system")
        && at_interactive_prompt(serial)
}

/// os-test setpwent, stage 2: cat the produced .err/.out and print the
/// verdict marker. Pass = plain `SETPWENT-OK` (and never plain
/// `SETPWENT-FAIL`) after the echoed command; the quoted markers inside the
/// echo cannot collide with the plain ones. Also refuse "not found" from the
/// cat (missing .err/.out => prep did not actually produce them).
fn interactive_ostest_result_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    let echoed = "$ ls out/basic/pwd; cat out/basic/pwd/setpwent.err out/basic/pwd/setpwent.out; test -s out/basic/pwd/setpwent.out -o -s out/basic/pwd/setpwent.err && echo SETPWENT-\"FAIL\" || echo SETPWENT-\"OK\"";
    if !tail.contains(echoed) || serial.contains("exception:") {
        return false;
    }
    let after = tail.rsplit_once(echoed).map(|(_, rest)| rest).unwrap_or("");
    after.contains("\nSETPWENT-OK")
        && !after.contains("\nSETPWENT-FAIL")
        && !after.contains("not found")
        && !after.contains("cannot create")
        && at_interactive_prompt(serial)
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
    // the double-newline regression shows up as a blank line (`\r\r\n`), which
    // makes `after` start with a blank line instead of `histrecall_zz`.
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
    tail.contains("$ http https://example.com/")
        && (tail.contains("tls handshake fail")
            || tail.contains("tls err ")
            || tail.contains("dns resolve fail")
            || tail.contains("tcp connect timeout"))
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
        // HTTPS/curl smokes and the os-test setpwent stage only exist in full
        // mode; in mini those slots are the interrupt/seed/arrow tail (matched
        // by position below). Full-mode length is 19, mini is 15.
        12 if cmds.len() == 19 => interactive_https_cmd_ok(serial),
        13 if cmds.len() == 19 => interactive_curl_cmd_ok(serial),
        12 if cmds.len() == 17 => interactive_ostest_prep_ok(serial), // TEMP skip-net local test
        13 if cmds.len() == 17 => interactive_ostest_result_ok(serial), // TEMP skip-net local test
        14 if cmds.len() == 19 => interactive_ostest_prep_ok(serial),
        15 if cmds.len() == 19 => interactive_ostest_result_ok(serial),
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

const SHELL_TYPE_DELAY: Duration = Duration::from_millis(25);
const SHELL_CMD_DELAY: Duration = Duration::from_millis(100);

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
                send_shell_byte(stdin, CI_LOGIN_USER[*typing]);
                *typing += 1;
                std::thread::sleep(SHELL_TYPE_DELAY);
            } else {
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
        }
        ShellStage::Typing => {
            let cmd = cmds[*cmd_index];
            if *typing < cmd.len() {
                send_shell_byte(stdin, cmd[*typing]);
                *typing += 1;
                std::thread::sleep(SHELL_TYPE_DELAY);
            } else {
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
        ShellStage::WaitResult if shell_cmd_result_ok(acc, cmds, *cmd_index, extra) => {
            *cmd_index += 1;
            if *cmd_index >= cmds.len() {
                *stage = ShellStage::Done;
            } else {
                *typing = 0;
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
    let acc_reader = serial_acc.clone();

    let mut stdout = child.stdout.take().expect("qemu stdout");
    let reader_handle = std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&buf[..n]);
                    eprint!("{chunk}");
                    // QEMU -serial stdio often delivers CRLF; status needles
                    // must not fail on CR vs LF. Normalize CR to LF.
                    let normalized = chunk.replace("\r\n", "\n").replace('\r', "\n");
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
    let mut interrupt_sent = false;
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
                // HTTPS: printed tls/dns/tcp failure — don't burn the 180s timeout.
                // Index 12 == `http https://example.com/` (full mode only; mini
                // drops the HTTPS/curl smokes and 12 is the interrupt test).
                if shell_stage == ShellStage::WaitResult
                    && !mini
                    && shell_cmd_index == 12
                    && interactive_https_cmd_failed(&acc)
                {
                    let _ = child.kill();
                    break child.wait().expect("wait after https fail-fast kill");
                }
                // curl: printed `curl: (N) …` — don't wait 180s for Example Domain.
                // Index 13 == interactive curl HTTPS smoke (full mode only).
                if shell_stage == ShellStage::WaitResult
                    && !mini
                    && shell_cmd_index == 13
                    && interactive_curl_cmd_failed(&acc)
                {
                    let _ = child.kill();
                    break child.wait().expect("wait after curl fail-fast kill");
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
            && !login_prompt_ready(&serial)
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
            eprintln!(
                "error: interactive `echo pipe | cat` failed (want `$ echo pipe | cat` then `pipe`)"
            );
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
        if cmds.len() == 17 && shell_cmd_index == 13 && !interactive_curl_cmd_ok(&serial) {
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
        if cmds.len() == 17 && shell_cmd_index == 12 && !interactive_https_cmd_ok(&serial) {
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