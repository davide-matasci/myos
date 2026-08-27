//! Host launcher: builds a bootable disk image (via `build.rs`) and starts QEMU.
//!
//! ```text
//! cargo run                 # BIOS, graphical (default)
//! cargo run -- bios
//! cargo run --features uefi -- uefi
//! cargo run -- --ci         # headless BIOS; used by GitHub Actions
//! ```

use std::io::Read;
use std::process::{exit, Command, Stdio};
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
        if mode != "bios" {
            eprintln!("--ci currently boots the BIOS image only");
            exit(2);
        }
        run_ci(bios_path);
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
  uefi   Boot the UEFI disk image (requires --features uefi)
  --ci   Headless BIOS boot; require serial hello + isa-debug-exit",
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
    #[cfg(not(feature = "uefi"))]
    {
        let _ = uefi_path;
        eprintln!("UEFI image is not in this build. Rebuild with:\n  cargo run --features uefi -- uefi");
        exit(2);
    }
    #[cfg(feature = "uefi")]
    {
        use ovmf_prebuilt::{Arch, FileType, Prebuilt, Source};

        let prebuilt =
            Prebuilt::fetch(Source::LATEST, "target/ovmf").expect("failed to fetch OVMF prebuilt");
        let code = prebuilt.get_file(Arch::X64, FileType::Code);
        let vars = prebuilt.get_file(Arch::X64, FileType::Vars);

        let status = Command::new("qemu-system-x86_64")
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
}

/// QEMU `isa-debug-exit` turns a 32-bit write of `value` into process status `(value << 1) | 1`.
/// The kernel writes `0x10` on success, so QEMU should exit 33.
const QEMU_SUCCESS_STATUS: i32 = (0x10 << 1) | 1;

fn run_ci(bios_path: &str) {
    let mut child = Command::new("qemu-system-x86_64")
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
            None if started.elapsed() > Duration::from_secs(20) => {
                let _ = child.kill();
                let _ = child.wait();
                eprintln!("error: QEMU timed out after 20s");
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
