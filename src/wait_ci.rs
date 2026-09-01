use std::io::Write;
use std::process::ChildStdin;

struct CiExpect {
    timeout: Duration,
    qemu_debug_exit: bool,
    /// Type commands at the interactive `$` prompt via `-serial stdio`.
    shell_ci: bool,
}

const CI_NEEDLES: [&str; 21] = [
    "Hello from myos",
    "[ OK ] heap",
    "[ OK ] interrupts",
    "task a",
    "task b",
    "[ OK ] scheduler",
    "mod ok",
    "[ OK ] limine module",
    "sh ok",
    "fork ok",
    "fork exec ok",
    "alloc ok",
    "user ok",
    "fat ok",
    // Slim always-on `/ok` VFS markers (pre-prompt readiness).
    "disk ok",
    "disk ls ok",
    "fat ls ok",
    "fat read ok",
    "devnull ok",
    "tmp ok",
    "tmpops ok",
];

/// Heavy markers from CI-only `/heap` (typed at `$` on every arch).
/// Pre-prompt readiness stays slim; these are required after interactive `heap`.
const CI_NEEDLES_STD: [&str; 11] = [
    "std ok",
    "std cat ok",
    "std echo ok",
    "bigalloc ok",
    "c ok",
    "sbase ok",
    "sls ok",
    "uutils echo ok",
    "uutils true ok",
    "uutils false ok",
    "sbase argv ok",
];

/// Interactive shell commands typed at the `$` prompt (serial stdin).
const CI_SHELL_COMMANDS: [&[u8]; 9] = [
    b"nosuchcmd\n",
    // CI-only heavy smoke (std/C/sbase/uutils/bigalloc); slim `/ok` already ran at boot.
    b"heap\n",
    b"ok\n",
    b"echo test\n",
    b"echo pipe | cat\n",
    b"c/true\n",
    b"/s/echo hi\n",
    b"/s/ls\n",
    // Typo then backspaces: canonical stdin must deliver `/s/ls`, not `x/s/ls` or raw BS.
    b"x\x08/s/ls\n",
];

/// Printed by the interactive shell when a command cannot be resolved.
const CI_SHELL_UNKNOWN_CMD: &str = "not found";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellStage {
    WaitPrompt,
    Typing,
    WaitResult,
    Done,
}

fn serial_has_all_needles(serial: &str, extra: &[&str]) -> bool {
    CI_NEEDLES.iter().chain(extra.iter()).all(|n| serial.contains(n))
}

fn interactive_tail(serial: &str) -> &str {
    serial
        .rsplit_once("fork exec ok")
        .map(|(_, tail)| tail)
        .unwrap_or(serial)
}

fn at_interactive_prompt(serial: &str) -> bool {
    let tail = interactive_tail(serial).trim();
    tail == "$" || tail.ends_with("$")
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
    command_echoed(serial, "ok")
        && !tail_has_hex_received(serial)
        && at_interactive_prompt(serial)
}

fn interactive_echo_cmd_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    if !tail.contains("$ echo test") || serial.contains("exception:") {
        return false;
    }
    let after = tail.rsplit_once("$ echo test").map(|(_, rest)| rest).unwrap_or("");
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
    command_echoed(serial, "c/true")
        && !serial.contains("exception:")
        && at_interactive_prompt(serial)
}

fn interactive_sbase_echo_cmd_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    if !tail.contains("$ /s/echo hi") || serial.contains("exception:") {
        return false;
    }
    let after = tail
        .rsplit_once("$ /s/echo hi")
        .map(|(_, rest)| rest)
        .unwrap_or("");
    after
        .lines()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| line.trim() == "hi")
        && at_interactive_prompt(serial)
}

fn interactive_sbase_ls_cmd_ok(serial: &str) -> bool {
    command_echoed(serial, "/s/ls")
        && !serial.contains("exception:")
        && !serial.contains("user panic")
        && at_interactive_prompt(serial)
}

/// `x<BS>/s/ls` must run `/s/ls` (canonical erase), not leave a bogus argv.
fn interactive_bs_ls_cmd_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    // Echo includes BS-space-BS; do not require a clean `$ /s/ls` substring.
    !serial.contains("exception:")
        && !serial.contains("user panic")
        && at_interactive_prompt(serial)
        && !tail.contains("/s/ls: not found")
        && !tail.contains("x/s/ls")
}

/// CI-only `/heap` carnival. All arches pass the same `heavy` needles
/// (`CI_NEEDLES_STD`) and must return to `$` after `smoke ok`.
fn interactive_heap_cmd_ok(serial: &str, heavy: &[&str]) -> bool {
    if !command_echoed(serial, "heap") || serial.contains("exception:") {
        return false;
    }
    if !heavy.iter().all(|n| serial.contains(*n)) {
        return false;
    }
    if !serial.contains("smoke ok") {
        return false;
    }
    at_interactive_prompt(serial)
}

fn shell_cmd_result_ok(serial: &str, cmd_index: usize, extra: &[&str]) -> bool {
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
    stage: &mut ShellStage,
    cmd_index: &mut usize,
    typing: &mut usize,
    acc: &str,
    extra: &[&str],
) {
    let Some(stdin) = stdin.as_mut() else {
        return;
    };
    match *stage {
        ShellStage::WaitPrompt if shell_ready(acc) => {
            *stage = ShellStage::Typing;
            *cmd_index = 0;
            *typing = 0;
        }
        ShellStage::Typing => {
            let cmd = CI_SHELL_COMMANDS[*cmd_index];
            if *typing < cmd.len() {
                send_shell_byte(stdin, cmd[*typing]);
                *typing += 1;
                std::thread::sleep(SHELL_TYPE_DELAY);
            } else {
                *stage = ShellStage::WaitResult;
            }
        }
        ShellStage::WaitResult if shell_cmd_result_ok(acc, *cmd_index, extra) => {
            *cmd_index += 1;
            if *cmd_index >= CI_SHELL_COMMANDS.len() {
                *stage = ShellStage::Done;
            } else {
                *typing = 0;
                std::thread::sleep(SHELL_CMD_DELAY);
                *stage = ShellStage::Typing;
            }
        }
        ShellStage::WaitPrompt | ShellStage::WaitResult | ShellStage::Done => {}
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
                    acc_reader.lock().unwrap().push_str(&chunk);
                }
                Err(_) => break,
            }
        }
    });

    let started = Instant::now();
    let mut timed_out = false;
    let mut killed_for_needles = false;
    let mut shell_stage = ShellStage::WaitPrompt;
    let mut shell_cmd_index = 0usize;
    let mut typing = 0usize;
    let status = loop {
        {
            let acc = serial_acc.lock().unwrap().clone();
            if expect.shell_ci {
                advance_shell_ci(
                    &mut shell_stdin,
                    &mut shell_stage,
                    &mut shell_cmd_index,
                    &mut typing,
                    &acc,
                    extra_needles,
                );
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
        if !at_interactive_prompt(&serial) && !command_echoed(&serial, "nosuchcmd") {
            eprintln!("error: serial never reached interactive `$` prompt");
        }
        if shell_cmd_index == 0 && !interactive_unknown_cmd_ok(&serial) {
            if !serial.contains(CI_SHELL_UNKNOWN_CMD) {
                eprintln!("error: serial output did not contain {CI_SHELL_UNKNOWN_CMD:?}");
            } else if tail_has_hex_received(&serial) {
                eprintln!(
                    "error: shell reported non-printable stdin (hex escapes in received:)"
                );
            } else {
                eprintln!(
                    "error: {CI_SHELL_UNKNOWN_CMD:?} did not follow interactive `nosuchcmd`"
                );
            }
        }
        if shell_cmd_index >= 1 && shell_cmd_index < 2 && !interactive_heap_cmd_ok(&serial, extra_needles) {
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
                if !serial.contains("smoke ok") {
                    eprintln!("error: CI-only `/heap` did not print \"smoke ok\"");
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
                eprintln!(
                    "error: shell reported non-printable input (hex escapes in received:)"
                );
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
            eprintln!(
                "error: interactive `c/true` failed (want `$ c/true` then `$` prompt)"
            );
        }
        if shell_cmd_index >= 6 && shell_cmd_index < 7 && !interactive_sbase_echo_cmd_ok(&serial) {
            if !command_echoed(&serial, "/s/echo hi") {
                eprintln!("error: serial did not echo `$ /s/echo hi` at the interactive prompt");
            } else if serial.contains("exception:") {
                eprintln!("error: interactive `/s/echo hi` triggered a CPU exception");
            } else if !at_interactive_prompt(&serial) {
                eprintln!("error: shell did not return to `$` after interactive `/s/echo hi`");
            } else {
                eprintln!("error: interactive `/s/echo hi` did not print `hi`");
            }
        }
        if shell_cmd_index >= 7 && shell_cmd_index < 8 && !interactive_sbase_ls_cmd_ok(&serial) {
            eprintln!(
                "error: interactive `/s/ls` failed (want `$ /s/ls` then `$` prompt)"
            );
        }
        if shell_cmd_index >= 8 && !interactive_bs_ls_cmd_ok(&serial) {
            eprintln!(
                "error: interactive `x<BS>/s/ls` failed (canonical backspace must yield `/s/ls`)"
            );
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
