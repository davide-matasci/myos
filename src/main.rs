//! Host launcher: builds a bootable disk image (via `build.rs`) and starts QEMU.
//!
//! ```text
//! cargo run              # BIOS, graphical
//! cargo run -- bios
//! cargo run -- uefi      # UEFI (fetches OVMF into target/ovmf)
//! cargo run -- --ci      # headless BIOS check
//! cargo run -- uefi --ci # headless UEFI check
//! ```

use ovmf_prebuilt::{Arch, FileType, Prebuilt, Source};
use std::io::Read;
use std::process::{exit, Child, Command, Stdio};
use std::time::{Duration, Instant};

fn main() {
    let bios_path = env!("BIOS_PATH");
    let uefi_path = env!("UEFI_PATH");

    let args: Vec<String> = std::env::args().skip(1).map(|s| s.to_lowercase()).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_usage();
        return;
    }

    let ci = args.iter().any(|a| a == "--ci");
    let mode = args
        .iter()
        .find(|a| a.as_str() == "uefi" || a.as_str() == "bios")
        .map(|s| s.as_str())
        .unwrap_or("bios");

    if ci {
        if mode == "uefi" {
            run_ci_uefi(uefi_path);
        } else {
            run_ci_bios(bios_path);
        }
    } else if mode == "uefi" {
        run_uefi(uefi_path);
    } else {
        run_bios(bios_path);
    }
}

fn print_usage() {
    eprintln!(
        "\
Usage: cargo run -- [bios|uefi] [--ci]

  bios   Boot the BIOS disk image in QEMU (default, graphical)
  uefi   Boot the UEFI disk image in QEMU (fetches OVMF on first run)
  --ci   Headless boot; require serial hello + isa-debug-exit",
    );
}

fn run_bios(bios_path: &str) {
    let status = Command::new("qemu-system-x86_64")
        .arg("-drive")
        .arg(format!("format=raw,file={bios_path}"))
        .arg("-serial")
        .arg("stdio")
        .status()
        .expect("failed to start qemu-system-x86_64");
    exit(status.code().unwrap_or(1));
}

fn run_uefi(uefi_path: &str) {
    let (code, vars) = ovmf_files();
    let status = Command::new("qemu-system-x86_64")
        .arg("-m")
        .arg("256")
        .arg("-drive")
        .arg(format!("format=raw,file={uefi_path}"))
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,unit=0,file={},readonly=on",
            code.display()
        ))
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,unit=1,file={},snapshot=on",
            vars.display()
        ))
        .arg("-serial")
        .arg("stdio")
        .status()
        .expect("failed to start qemu-system-x86_64");
    exit(status.code().unwrap_or(1));
}

fn ovmf_files() -> (std::path::PathBuf, std::path::PathBuf) {
    let prebuilt =
        Prebuilt::fetch(Source::LATEST, "target/ovmf").expect("failed to fetch OVMF prebuilt");
    (
        prebuilt.get_file(Arch::X64, FileType::Code),
        prebuilt.get_file(Arch::X64, FileType::Vars),
    )
}

/// QEMU `isa-debug-exit` turns a 32-bit write of `value` into process status `(value << 1) | 1`.
/// The kernel writes `0x10` on success, so QEMU should exit 33.
const QEMU_SUCCESS_STATUS: i32 = (0x10 << 1) | 1;

fn run_ci_bios(bios_path: &str) {
    let child = Command::new("qemu-system-x86_64")
        .arg("-drive")
        .arg(format!("format=raw,file={bios_path}"))
        .arg("-serial")
        .arg("stdio")
        .arg("-display")
        .arg("none")
        .arg("-device")
        .arg("isa-debug-exit,iobase=0xf4,iosize=0x04")
        .arg("-no-reboot")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start qemu-system-x86_64");
    wait_ci(child, Duration::from_secs(20));
}

fn run_ci_uefi(uefi_path: &str) {
    let (code, vars) = ovmf_files();
    let child = Command::new("qemu-system-x86_64")
        .arg("-m")
        .arg("256")
        .arg("-drive")
        .arg(format!("format=raw,file={uefi_path}"))
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,unit=0,file={},readonly=on",
            code.display()
        ))
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,unit=1,file={},snapshot=on",
            vars.display()
        ))
        .arg("-serial")
        .arg("stdio")
        .arg("-display")
        .arg("none")
        .arg("-device")
        .arg("isa-debug-exit,iobase=0xf4,iosize=0x04")
        .arg("-no-reboot")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start qemu-system-x86_64");
    wait_ci(child, Duration::from_secs(60));
}

fn wait_ci(mut child: Child, timeout: Duration) {
    let mut stdout = child.stdout.take().expect("qemu stdout");
    let mut stderr = child.stderr.take().expect("qemu stderr");
    let stdout_handle = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait().expect("failed to wait on qemu") {
            Some(status) => break status,
            None if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                eprintln!("error: QEMU timed out after {:?}", timeout);
                exit(1);
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };

    let serial = stdout_handle.join().expect("stdout thread");
    let err = stderr_handle.join().expect("stderr thread");
    eprint!("{err}");
    print!("{serial}");

    if !serial.contains("Hello from myos") {
        eprintln!("error: serial output did not contain \"Hello from myos\"");
        exit(1);
    }
    if status.code() != Some(QEMU_SUCCESS_STATUS) {
        eprintln!(
            "error: unexpected QEMU exit status {status:?} (want {QEMU_SUCCESS_STATUS} from isa-debug-exit)"
        );
        exit(1);
    }
}
