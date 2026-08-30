use std::io::Write;
use std::process::ChildStdin;

struct CiExpect {
    timeout: Duration,
    qemu_debug_exit: bool,
    /// Type commands at the interactive `$` prompt via `-serial stdio`.
    shell_ci: bool,
}

const CI_NEEDLES: [&str; 14] = [
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
];

/// Extra serial markers for patched `std` examples and C smoke ELFs.
const CI_NEEDLES_STD: [&str; 4] = ["std ok", "std cat ok", "std echo ok", "c ok"];

/// Interactive shell commands typed at the `$` prompt (serial stdin).
const CI_SHELL_COMMANDS: [&[u8]; 4] = [
    b"nosuchcmd\n",
    b"ok\n",
    b"echo test\n",
    b"echo pipe | cat\n",
];

/// Printed by the interactive shell when `open(path)` fails.
const CI_SHELL_UNKNOWN_CMD: &str = "sh: command not found";

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
    command_echoed(serial, "ok") && !tail_has_hex_received(serial)
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

fn shell_cmd_result_ok(serial: &str, cmd_index: usize) -> bool {
    match cmd_index {
        0 => interactive_unknown_cmd_ok(serial),
        1 => interactive_ok_cmd_ok(serial),
        2 => interactive_echo_cmd_ok(serial),
        3 => interactive_pipe_cmd_ok(serial),
        _ => false,
    }
}

fn shell_ready(serial: &str, extra: &[&str]) -> bool {
    serial_has_all_needles(serial, extra) && at_interactive_prompt(serial)
}

const SHELL_TYPE_DELAY: Duration = Duration::from_millis(25);

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
        ShellStage::WaitPrompt if shell_ready(acc, extra) => {
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
        ShellStage::WaitResult if shell_cmd_result_ok(acc, *cmd_index) => {
            *cmd_index += 1;
            if *cmd_index >= CI_SHELL_COMMANDS.len() {
                *stage = ShellStage::Done;
            } else {
                *typing = 0;
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
        if shell_cmd_index >= 1 && shell_cmd_index < 2 && !interactive_ok_cmd_ok(&serial) {
            if !command_echoed(&serial, "ok") {
                eprintln!("error: serial did not echo `$ ok` at the interactive prompt");
            }
            if tail_has_hex_received(&serial) {
                eprintln!(
                    "error: shell reported non-printable input (hex escapes in received:)"
                );
            }
        }
        if shell_cmd_index >= 2 && shell_cmd_index < 3 && !interactive_echo_cmd_ok(&serial) {
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
        if shell_cmd_index >= 3 && !interactive_pipe_cmd_ok(&serial) {
            eprintln!(
                "error: interactive `echo pipe | cat` failed (want `$ echo pipe | cat` then `pipe`)"
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
