use std::io::Write;
use std::process::ChildStdin;

struct CiExpect {
    timeout: Duration,
    qemu_debug_exit: bool,
    /// Type an unknown command at the interactive `$` prompt via `-serial stdio`.
    shell_ci: bool,
}

const CI_NEEDLES: [&str; 14] = [
    "Hello from myos",
    "heap ok",
    "int ok",
    "task a",
    "task b",
    "sched ok",
    "mod ok",
    "limine mod ok",
    "sh ok",
    "fork ok",
    "fork exec ok",
    "alloc ok",
    "user ok",
    "fat ok",
];

/// Extra serial markers required on x86 BIOS/UEFI only (`std` hello is x86-myos today).
const CI_NEEDLES_X86: [&str; 1] = ["std ok"];

const CI_UNKNOWN_CMD: &[u8] = b"nosuchcmd";

/// Printed by the interactive shell (parent) when `open(path)` fails.
const CI_SHELL_UNKNOWN_CMD: &str = "sh: command not found";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellStage {
    WaitPrompt,
    Typing,
    SentEnter,
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

fn command_echoed(serial: &str) -> bool {
    interactive_tail(serial).contains("$ nosuchcmd")
}

fn interactive_unknown_cmd_ok(serial: &str) -> bool {
    let tail = interactive_tail(serial);
    let Some(idx) = tail.find(CI_SHELL_UNKNOWN_CMD) else {
        return false;
    };
    tail[..idx].contains("$ nosuchcmd")
}

fn shell_ready(serial: &str, extra: &[&str]) -> bool {
    serial_has_all_needles(serial, extra) && at_interactive_prompt(serial)
}

fn send_shell_byte(stdin: &mut ChildStdin, byte: u8) {
    stdin
        .write_all(&[byte])
        .expect("write shell byte to qemu stdin");
    stdin.flush().ok();
}

fn send_shell_enter(stdin: &mut ChildStdin) {
    stdin
        .write_all(b"\r\n")
        .expect("write shell newline to qemu stdin");
    stdin.flush().ok();
}

fn advance_shell_ci(
    stdin: &mut Option<ChildStdin>,
    stage: &mut ShellStage,
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
            *typing = 0;
        }
        ShellStage::Typing => {
            if *typing < CI_UNKNOWN_CMD.len() {
                send_shell_byte(stdin, CI_UNKNOWN_CMD[*typing]);
                *typing += 1;
                std::thread::sleep(Duration::from_millis(25));
            } else if command_echoed(acc) {
                std::thread::sleep(Duration::from_millis(200));
                send_shell_enter(stdin);
                *stage = ShellStage::SentEnter;
            }
        }
        ShellStage::SentEnter if interactive_unknown_cmd_ok(acc) => {
            *stage = ShellStage::Done;
        }
        ShellStage::WaitPrompt | ShellStage::SentEnter | ShellStage::Done => {}
    }
}

fn ci_complete(acc: &str, extra: &[&str], expect: &CiExpect, stage: ShellStage) -> bool {
    if !serial_has_all_needles(acc, extra) {
        return false;
    }
    if expect.shell_ci {
        interactive_unknown_cmd_ok(acc) && stage == ShellStage::Done
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
    let mut typing = 0usize;
    let status = loop {
        {
            let acc = serial_acc.lock().unwrap().clone();
            if expect.shell_ci {
                advance_shell_ci(
                    &mut shell_stdin,
                    &mut shell_stage,
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
    if expect.shell_ci && !interactive_unknown_cmd_ok(&serial) {
        if timed_out {
            eprintln!("error: QEMU timed out after {:?}", expect.timeout);
        }
        eprintln!("error: shell CI stage was {shell_stage:?}");
        if !at_interactive_prompt(&serial) && !command_echoed(&serial) {
            eprintln!("error: serial never reached interactive `$` prompt");
        }
        if !serial.contains(CI_SHELL_UNKNOWN_CMD) {
            eprintln!("error: serial output did not contain {CI_SHELL_UNKNOWN_CMD:?}");
        } else if !interactive_unknown_cmd_ok(&serial) {
            eprintln!(
                "error: {CI_SHELL_UNKNOWN_CMD:?} did not follow interactive `nosuchcmd`"
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
