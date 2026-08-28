use std::io::Write;
use std::process::ChildStdin;

struct CiExpect {
    timeout: Duration,
    qemu_debug_exit: bool,
    /// After boot smokes, inject `\n` at the interactive `$` prompt via `-serial stdio`.
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

/// Boot smokes cover unknown-command handling and `echo` with argv; keyboard sends enter.
const CI_SHELL_NEEDLES: [&str; 2] = ["sh: command not found", "SHELLCI"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellStage {
    WaitPrompt,
    SentEnter,
    Done,
}

fn serial_has_all_needles(serial: &str, extra: &[&str]) -> bool {
    CI_NEEDLES.iter().chain(extra.iter()).all(|n| serial.contains(n))
}

fn serial_has_shell_needles(serial: &str) -> bool {
    CI_SHELL_NEEDLES.iter().all(|n| serial.contains(n))
        && serial_has_keyboard_ack(serial)
}

/// After an empty interactive command, the shell prints another `$` prompt.
fn serial_has_keyboard_ack(serial: &str) -> bool {
    serial
        .rsplit("SHELLCI")
        .next()
        .is_some_and(|tail| tail.matches("$").count() >= 2)
}

fn at_interactive_prompt(serial: &str) -> bool {
    if !serial.contains("SHELLCI") {
        return false;
    }
    let Some(tail) = serial.rsplit("SHELLCI").next() else {
        return false;
    };
    let t = tail.trim();
    t == "$" || t.ends_with("$")
}

fn shell_ready(serial: &str, extra: &[&str]) -> bool {
    serial_has_all_needles(serial, extra)
        && serial.contains("sh: command not found")
        && serial.contains("SHELLCI")
        && at_interactive_prompt(serial)
}

fn send_shell_enter(stdin: &mut ChildStdin) {
    std::thread::sleep(Duration::from_millis(200));
    stdin
        .write_all(b"\n")
        .expect("write shell newline to qemu stdin");
    stdin.flush().ok();
}

fn advance_shell_ci(
    stdin: &mut Option<ChildStdin>,
    stage: &mut ShellStage,
    acc: &str,
    extra: &[&str],
) {
    let Some(stdin) = stdin.as_mut() else {
        return;
    };
    match *stage {
        ShellStage::WaitPrompt if shell_ready(acc, extra) => {
            send_shell_enter(stdin);
            *stage = ShellStage::SentEnter;
        }
        ShellStage::SentEnter if serial_has_keyboard_ack(acc) => {
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
        serial_has_shell_needles(acc) && stage == ShellStage::Done
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
    let status = loop {
        {
            let acc = serial_acc.lock().unwrap().clone();
            if expect.shell_ci {
                advance_shell_ci(&mut shell_stdin, &mut shell_stage, &acc, extra_needles);
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
        exit(1);
    }
    if expect.shell_ci && !serial_has_shell_needles(&serial) {
        if timed_out {
            eprintln!("error: QEMU timed out after {:?}", expect.timeout);
        }
        eprintln!("error: shell CI stage was {shell_stage:?}");
        for needle in CI_SHELL_NEEDLES {
            if !serial.contains(needle) {
                eprintln!("error: serial output did not contain shell needle {needle:?}");
            }
        }
        if !serial_has_keyboard_ack(&serial) {
            eprintln!("error: serial output did not show a second `$` prompt after keyboard enter");
        }
        exit(1);
    }
    if expect.qemu_debug_exit && !timed_out && !killed_for_needles {
        if status.code() != Some(QEMU_SUCCESS_STATUS) {
            eprintln!(
                "error: unexpected QEMU exit status {status:?} (want {QEMU_SUCCESS_STATUS} from isa-debug-exit)"
            );
            exit(1);
        }
    }
}
