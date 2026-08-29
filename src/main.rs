//! Host launcher: builds a bootable Limine image and starts QEMU.
//!
//! ```text
//! cargo run                 # x86_64 BIOS, graphical
//! cargo run -- bios
//! cargo run -- uefi         # x86_64 UEFI (fetches OVMF into target/ovmf)
//! cargo run -- aarch64      # QEMU virt + AAVMF, serial + ramfb
//! cargo run -- iso          # write target/myos-x86_64.iso (no QEMU)
//! cargo run -- --ci         # headless BIOS check
//! cargo run -- uefi --ci    # headless UEFI check
//! cargo run -- aarch64 --ci # headless AArch64 check
//! ```

mod limine_image;

use limine_image::{
    fetch_limine, write_esp_image, write_fat_data_image, write_x86_iso, LIMINE_VERSION,
};
use ovmf_prebuilt::{Arch, FileType, Prebuilt, Source};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{exit, Child, Command, Stdio};
use std::time::{Duration, Instant};

const AARCH64_TARGET: &str = "aarch64-unknown-none-softfloat";
/// Default qemu64 does not advertise x2APIC (CI #49 panicked, #50 fell back
/// to PIC and hung). Limine leaves PIC IRQs dead, so the kernel timer proof
/// needs the x2APIC MSRs.
const X86_CPU: &str = "qemu64,+x2apic";

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
        .find(|a| matches!(a.as_str(), "uefi" | "bios" | "aarch64" | "iso"))
        .map(|s| s.as_str())
        .unwrap_or("bios");

    match (mode, ci) {
        ("iso", _) => run_iso(),
        ("aarch64", true) => run_ci_aarch64(),
        ("aarch64", false) => run_aarch64(),
        ("uefi", true) => run_ci_uefi(uefi_path),
        ("uefi", false) => run_uefi(uefi_path),
        (_, true) => run_ci_bios(bios_path),
        (_, false) => run_bios(bios_path),
    }
}

fn print_usage() {
    eprintln!(
        "\
Usage: cargo run -- [bios|uefi|aarch64|iso] [--ci]

  bios      Boot the x86_64 Limine BIOS disk image in QEMU (default, graphical)
  uefi      Boot the x86_64 Limine UEFI disk image in QEMU (fetches OVMF on first run)
  aarch64   Boot the AArch64 kernel via Limine on QEMU virt + AAVMF (serial + ramfb)
  iso       Write target/myos-x86_64.iso (Limine BIOS+UEFI hybrid) and exit; needs xorriso
  --ci      Headless boot; require serial hello/heap/int/mod and a clean QEMU exit",
    );
}

fn run_iso() {
    // Artifact-dep kernel lives at CARGO_BIN_FILE_KERNEL_kernel, not
    // target/<triple>/debug/kernel (ISO #1 panicked on that missing path).
    let kernel = Path::new(env!("KERNEL_PATH"));
    let hello = Path::new(env!("HELLO_PATH"));
    let ok = Path::new(env!("OK_PATH"));
    if !kernel.is_file() {
        panic!("kernel ELF missing at {}", kernel.display());
    }
    if !hello.is_file() {
        panic!("hello ELF missing at {}", hello.display());
    }
    if !ok.is_file() {
        panic!("ok ELF missing at {}", ok.display());
    }
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let limine = fetch_limine(Path::new(env!("LIMINE_DIR")));
    let dest = target.join("myos-x86_64.iso");
    let iso_root = target.join("iso_root");
    write_x86_iso(&dest, &iso_root, kernel, hello, ok, &limine);
    println!("{}", dest.display());
}

fn fat_img_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/fat.img")
}

fn add_virtio_blk_x86(cmd: &mut Command) {
    let fat = fat_img_path();
    cmd.arg("-drive").arg(format!(
        "if=none,id=vd0,format=raw,file={}",
        fat.display()
    ));
    // Legacy I/O-BAR virtio-blk (kernel talks to BAR0 ports, device 0x1001).
    cmd.arg("-device")
        .arg("virtio-blk-pci,drive=vd0,disable-modern=on");
}

fn add_virtio_blk_aarch64(cmd: &mut Command) {
    let fat = fat_img_path();
    cmd.arg("-drive").arg(format!(
        "if=none,id=vd0,format=raw,file={}",
        fat.display()
    ));
    cmd.arg("-device").arg("virtio-blk-device,drive=vd0");
}

fn run_bios(bios_path: &str) {
    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.arg("-cpu")
        .arg(X86_CPU)
        .arg("-m")
        .arg("256")
        .arg("-drive")
        .arg(format!("format=raw,file={bios_path}"))
        .arg("-serial")
        .arg("stdio")
        .arg("-nic")
        .arg("none")
        .arg("-boot")
        .arg("order=c,menu=off");
    add_virtio_blk_x86(&mut cmd);
    let status = cmd.status().expect("failed to start qemu-system-x86_64");
    exit(status.code().unwrap_or(1));
}

fn run_uefi(uefi_path: &str) {
    let (code, vars) = ovmf_files(Arch::X64);
    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.arg("-cpu")
        .arg(X86_CPU)
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
        .arg("-nic")
        .arg("none");
    add_virtio_blk_x86(&mut cmd);
    let status = cmd.status().expect("failed to start qemu-system-x86_64");
    exit(status.code().unwrap_or(1));
}

fn run_aarch64() {
    let image = build_aarch64_image();
    let status = qemu_aarch64(&image, false)
        .status()
        .expect("failed to start qemu-system-aarch64");
    exit(status.code().unwrap_or(1));
}

fn ovmf_files(arch: Arch) -> (PathBuf, PathBuf) {
    let prebuilt =
        Prebuilt::fetch(Source::LATEST, "target/ovmf").expect("failed to fetch OVMF prebuilt");
    (
        prebuilt.get_file(arch, FileType::Code),
        prebuilt.get_file(arch, FileType::Vars),
    )
}

fn aarch64_firmware() -> (PathBuf, PathBuf) {
    if let Ok(prebuilt) = Prebuilt::fetch(Source::LATEST, "target/ovmf") {
        let code = prebuilt.get_file(Arch::Aarch64, FileType::Code);
        let vars = prebuilt.get_file(Arch::Aarch64, FileType::Vars);
        if code.is_file() && vars.is_file() {
            return (code, vars);
        }
    }
    const CANDIDATES: &[(&str, &str)] = &[
        (
            "/usr/share/AAVMF/AAVMF_CODE.fd",
            "/usr/share/AAVMF/AAVMF_VARS.fd",
        ),
        (
            "/usr/share/qemu-efi-aarch64/QEMU_EFI.fd",
            "/usr/share/AAVMF/AAVMF_VARS.fd",
        ),
    ];
    for (c, v) in CANDIDATES {
        if Path::new(c).is_file() {
            return (PathBuf::from(c), PathBuf::from(v));
        }
    }
    panic!(
        "no AArch64 UEFI firmware found. Install qemu-efi-aarch64 (Debian/Ubuntu) \
         or allow ovmf-prebuilt to download into target/ovmf"
    );
}

/// QEMU `isa-debug-exit` turns a 32-bit write of `value` into process status `(value << 1) | 1`.
/// The kernel writes `0x10` on success, so QEMU should exit 33.
const QEMU_SUCCESS_STATUS: i32 = (0x10 << 1) | 1;

fn run_ci_bios(bios_path: &str) {
    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.arg("-cpu")
        .arg(X86_CPU)
        .arg("-m")
        .arg("256")
        .arg("-drive")
        .arg(format!("format=raw,file={bios_path}"))
        .arg("-serial")
        .arg("stdio")
        .arg("-display")
        .arg("none")
        .arg("-monitor")
        .arg("none")
        .arg("-device")
        .arg("isa-debug-exit,iobase=0xf4,iosize=0x04")
        .arg("-nic")
        .arg("none")
        .arg("-boot")
        .arg("order=c,menu=off")
        .arg("-no-reboot");
    add_virtio_blk_x86(&mut cmd);
    let child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start qemu-system-x86_64");
    wait_ci(
        child,
        CiExpect {
            timeout: Duration::from_secs(90),
            qemu_debug_exit: true,
            shell_ci: true,
        },
        &CI_NEEDLES_STD,
    );
}

fn run_ci_uefi(uefi_path: &str) {
    let (code, vars) = ovmf_files(Arch::X64);
    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.arg("-cpu")
        .arg(X86_CPU)
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
        .arg("-monitor")
        .arg("none")
        .arg("-device")
        .arg("isa-debug-exit,iobase=0xf4,iosize=0x04")
        .arg("-nic")
        .arg("none")
        .arg("-no-reboot");
    add_virtio_blk_x86(&mut cmd);
    let child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start qemu-system-x86_64");
    wait_ci(
        child,
        CiExpect {
            timeout: Duration::from_secs(90),
            qemu_debug_exit: true,
            shell_ci: true,
        },
        &CI_NEEDLES_STD,
    );
}

fn run_ci_aarch64() {
    let image = build_aarch64_image();
    let child = qemu_aarch64(&image, true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start qemu-system-aarch64");
    wait_ci(
        child,
        CiExpect {
            timeout: Duration::from_secs(90),
            qemu_debug_exit: false,
            shell_ci: true,
        },
        &CI_NEEDLES_STD,
    );
}

fn qemu_aarch64(image: &Path, ci: bool) -> Command {
    let (code, vars) = aarch64_firmware();
    let mut cmd = Command::new("qemu-system-aarch64");
    // gic-version=2 keeps the distributor/CPU interface at the classic MMIO
    // addresses (0x0800_0000 / 0x0801_0000) used later for IRQs.
    // virtio-mmio transports default to legacy (version 1); the driver is v2.
    cmd.arg("-global")
        .arg("virtio-mmio.force-legacy=false")
        .arg("-machine")
        .arg("virt,gic-version=2")
        .arg("-cpu")
        .arg("cortex-a72")
        .arg("-m")
        .arg("1024")
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
        .arg("-drive")
        .arg(format!("format=raw,file={}", image.display()))
        .arg("-serial")
        .arg("stdio")
        .arg("-nic")
        .arg("none")
        .arg("-no-reboot");
    add_virtio_blk_aarch64(&mut cmd);
    if ci {
        cmd.arg("-display").arg("none");
        cmd.arg("-monitor").arg("none");
    } else {
        cmd.arg("-device").arg("ramfb");
        cmd.arg("-device").arg("virtio-keyboard-device");
    }
    cmd
}

fn build_aarch64_image() -> PathBuf {
    let kernel = build_aarch64_kernel();
    let kernel_bytes = std::fs::read(&kernel).expect("read aarch64 kernel ELF");
    let hello_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/hello-aarch64-unknown-none-softfloat");
    let hello = std::fs::read(&hello_path).unwrap_or_else(|_| {
        panic!("hello ELF missing at {}", hello_path.display())
    });
    let ok_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/ok-aarch64-unknown-none-softfloat");
    let ok = std::fs::read(&ok_path).unwrap_or_else(|_| {
        panic!("ok ELF missing at {}", ok_path.display())
    });
    let limine_dir = PathBuf::from(env!("LIMINE_DIR"));
    let limine = if limine_dir.join("BOOTAA64.EFI").is_file() {
        fetch_limine(&limine_dir)
    } else {
        let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("limine-v{LIMINE_VERSION}"));
        fetch_limine(&fallback)
    };
    let efi = std::fs::read(limine.bootaa64()).expect("BOOTAA64.EFI");
    let image = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/aarch64.img");
    write_esp_image(&image, &kernel_bytes, "BOOTAA64.EFI", &efi, None, &hello, &ok);
    write_fat_data_image(&fat_img_path());
    image
}

fn build_aarch64_kernel() -> PathBuf {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let mut cmd = Command::new(cargo);
    cmd.arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("-p")
        .arg("kernel")
        .arg("--bin")
        .arg("kernel")
        .arg("--target")
        .arg(AARCH64_TARGET);
    if !cfg!(debug_assertions) {
        cmd.arg("--release");
    }
    let status = cmd
        .status()
        .expect("failed to invoke cargo for aarch64 kernel");
    if !status.success() {
        eprintln!("error: building aarch64 kernel failed");
        exit(1);
    }

    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"));
    let elf = target_dir
        .join(AARCH64_TARGET)
        .join(profile)
        .join("kernel");
    if !elf.is_file() {
        eprintln!("error: aarch64 kernel ELF missing at {}", elf.display());
        exit(1);
    }
    elf
}

include!("wait_ci.rs");
