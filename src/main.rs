//! Host launcher: builds a bootable Limine image and starts QEMU.
//!
//! ```text
//! cargo run                 # x86_64 BIOS, graphical
//! cargo run -- bios
//! cargo run -- uefi         # x86_64 UEFI (fetches OVMF into target/ovmf)
//! cargo run -- aarch64      # QEMU virt + AAVMF, serial + ramfb
//! cargo run -- riscv64      # QEMU virt + RISC-V UEFI, serial + ramfb
//! cargo run -- iso          # write target/myos-x86_64.iso (no QEMU)
//! cargo run -- --ci         # headless BIOS check
//! cargo run -- uefi --ci    # headless UEFI check
//! cargo run -- aarch64 --ci # headless AArch64 check
//! cargo run -- riscv64 --ci # headless RISC-V64 check
//! ```

mod limine_image;
mod initramfs;

use limine_image::{
    DiskFile, LIMINE_VERSION, fetch_limine, write_esp_image, write_esp_image_ex,
    write_fat_data_image, write_x86_iso,
};
use ovmf_prebuilt::{Arch, FileType, Prebuilt, Source};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio, exit};
use std::time::{Duration, Instant};

const AARCH64_TARGET: &str = "aarch64-unknown-none-softfloat";
const RISCV64_TARGET: &str = "riscv64imac-unknown-none-elf";

const RISCV_LIMINE_CONF: &str = "\
serial: yes
timeout: 0
randomise_hhdm_base: no
global_dtb: boot():/boot/virt.dtb

/myos
    protocol: limine
    path: boot():/boot/kernel
    paging_mode: sv39
    module_path: boot():/boot/hello
    module_path: boot():/boot/ok
    module_path: boot():/boot/initramfs
";
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
        .find(|a| matches!(a.as_str(), "uefi" | "bios" | "aarch64" | "riscv64" | "iso"))
        .map(|s| s.as_str())
        .unwrap_or("bios");

    match (mode, ci) {
        ("iso", _) => run_iso(),
        ("riscv64", true) => run_ci_riscv64(),
        ("riscv64", false) => run_riscv64(),
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
Usage: cargo run -- [bios|uefi|aarch64|riscv64|iso] [--ci]

  bios      Boot the x86_64 Limine BIOS disk image in QEMU (default, graphical)
  uefi      Boot the x86_64 Limine UEFI disk image in QEMU (fetches OVMF on first run)
  aarch64   Boot the AArch64 kernel via Limine on QEMU virt + AAVMF (serial + ramfb)
  riscv64   Boot the RISC-V64 kernel via Limine on QEMU virt + UEFI (serial + ramfb)
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
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target = manifest.join("target");
    let initramfs_path = target.join("initramfs-x86_64.cpio");
    std::fs::write(&initramfs_path, initramfs::build_initramfs(&manifest, "x86_64"))
        .expect("write target/initramfs-x86_64.cpio");
    let limine = fetch_limine(Path::new(env!("LIMINE_DIR")));
    let dest = target.join("myos-x86_64.iso");
    let iso_root = target.join("iso_root");
    write_x86_iso(&dest, &iso_root, kernel, hello, ok, &initramfs_path, &limine);
    println!("{}", dest.display());
}

fn fat_img_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/fat.img")
}

fn empty_img_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/empty.img")
}

fn write_empty_blk_image() {
    let dest = empty_img_path();
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&dest, vec![0u8; 64 * 1024])
        .unwrap_or_else(|e| panic!("write {}: {e}", dest.display()));
}

fn nvme_img_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/nvme.img")
}

fn write_nvme_blk_image() {
    let dest = nvme_img_path();
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&dest, vec![0u8; 4 * 1024 * 1024])
        .unwrap_or_else(|e| panic!("write {}: {e}", dest.display()));
}

fn add_nvme(cmd: &mut Command) {
    write_nvme_blk_image();
    let img = nvme_img_path();
    cmd.arg("-drive").arg(format!(
        "if=none,id=nvme0,format=raw,file={}",
        img.display()
    ));
    cmd.arg("-device").arg("nvme,drive=nvme0,serial=myos");
}

fn add_virtio_blk_x86(cmd: &mut Command) {
    write_empty_blk_image();
    let fat = fat_img_path();
    let empty = empty_img_path();
    cmd.arg("-drive")
        .arg(format!("if=none,id=vd0,format=raw,file={}", fat.display()));
    // Legacy I/O-BAR virtio-blk (kernel talks to BAR0 ports, device 0x1001).
    cmd.arg("-device")
        .arg("virtio-blk-pci,drive=vd0,disable-modern=on");
    cmd.arg("-drive").arg(format!(
        "if=none,id=vd1,format=raw,file={}",
        empty.display()
    ));
    cmd.arg("-device")
        .arg("virtio-blk-pci,drive=vd1,disable-modern=on");
    add_nvme(cmd);
}

fn add_virtio_blk_aarch64(cmd: &mut Command) {
    write_empty_blk_image();
    let fat = fat_img_path();
    let empty = empty_img_path();
    cmd.arg("-drive")
        .arg(format!("if=none,id=vd0,format=raw,file={}", fat.display()));
    cmd.arg("-device").arg("virtio-blk-device,drive=vd0");
    cmd.arg("-drive").arg(format!(
        "if=none,id=vd1,format=raw,file={}",
        empty.display()
    ));
    cmd.arg("-device").arg("virtio-blk-device,drive=vd1");
    add_nvme(cmd);
}

fn add_virtio_blk_riscv64(cmd: &mut Command) {
    add_virtio_blk_aarch64(cmd);
}

fn add_virtio_net(cmd: &mut Command) {
    cmd.arg("-netdev").arg("user,id=net0");
    cmd.arg("-device").arg("virtio-net-pci,netdev=net0");
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
    add_virtio_net(&mut cmd);
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
    add_virtio_net(&mut cmd);
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
    add_virtio_net(&mut cmd);
    let child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start qemu-system-x86_64");
    wait_ci(
        child,
        CiExpect {
            timeout: Duration::from_secs(180),
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
    add_virtio_net(&mut cmd);
    let child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start qemu-system-x86_64");
    wait_ci(
        child,
        CiExpect {
            timeout: Duration::from_secs(180),
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
            timeout: Duration::from_secs(180),
            qemu_debug_exit: false,
            shell_ci: true,
        },
        // Same heavy `/heap` needles as x86 (typed at `$` after slim `/ok`).
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
        .arg("virt,gic-version=2,highmem-ecam=off,highmem-mmio=off")
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
        ));
    // Data disks first so they become /dev/vda and /dev/vdb; ESP boots via bootindex.
    add_virtio_blk_aarch64(&mut cmd);
    add_virtio_net(&mut cmd);
    cmd.arg("-drive")
        .arg(format!(
            "if=none,id=hd0,format=raw,file={}",
            image.display()
        ))
        .arg("-device")
        .arg("virtio-blk-device,drive=hd0,bootindex=1")
        .arg("-serial")
        .arg("stdio")
        .arg("-nic")
        .arg("none")
        .arg("-no-reboot");
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
    let hello = std::fs::read(&hello_path)
        .unwrap_or_else(|_| panic!("hello ELF missing at {}", hello_path.display()));
    let ok_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ok-aarch64-unknown-none-softfloat");
    let ok = std::fs::read(&ok_path)
        .unwrap_or_else(|_| panic!("ok ELF missing at {}", ok_path.display()));
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
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let initramfs = initramfs::build_initramfs(&manifest, "aarch64");
    write_esp_image(
        &image,
        &kernel_bytes,
        "BOOTAA64.EFI",
        &efi,
        None,
        &hello,
        &ok,
        &initramfs,
    );
    write_fat_data_image(&fat_img_path());
    image
}


/// rust-lld default image base (AArch64 0x200000, RISC-V 0x10000). netd (and
/// ping) are ET_EXEC; netd has absolute smoltcp vtables. The kernel slides
/// PT_LOAD to USER_BASE without fixing abs relocs, so they must be linked at
/// 0x40000000.
const USER_BASE: u64 = 0x4000_0000;

fn assert_elf_linked_at_user_base(elf: &Path, bin: &str, target: &str) {
    let bytes = std::fs::read(elf).unwrap_or_else(|e| {
        eprintln!("error: read {bin} ELF {}: {e}", elf.display());
        exit(1);
    });
    if bytes.len() < 64 || &bytes[0..4] != b"\x7fELF" {
        eprintln!("error: {bin} for {target} is not ELF64");
        exit(1);
    }
    let entry = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
    let phoff = u64::from_le_bytes(bytes[32..40].try_into().unwrap()) as usize;
    let phentsize = u16::from_le_bytes(bytes[54..56].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes(bytes[56..58].try_into().unwrap()) as usize;
    let mut min_vaddr = u64::MAX;
    for i in 0..phnum {
        let p = phoff + i * phentsize;
        if p + 40 > bytes.len() {
            break;
        }
        let p_type = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
        if p_type != 1 {
            continue;
        }
        let vaddr = u64::from_le_bytes(bytes[p + 16..p + 24].try_into().unwrap());
        min_vaddr = min_vaddr.min(vaddr);
    }
    if entry < USER_BASE || min_vaddr == u64::MAX || min_vaddr < USER_BASE {
        eprintln!(
            "error: {bin} for {target} entry {entry:#x} min PT_LOAD {min_vaddr:#x} below USER_BASE {USER_BASE:#x}"
        );
        exit(1);
    }
}

/// Force a correctly-linked `target/{bin}-$target` before the kernel build.
///
/// Boot CI does `cargo clean -p kernel --target`, but that can leave the
/// kernel build-script fingerprint/output "fresh". The reused USER_*_PATH
/// then `include_bytes!` a rust-cache ELF still linked at 0x200000/0x10000.
fn ensure_user_at_user_base(cargo: &str, target: &str, bin: &str) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target_dir = root.join("target");
    let stable = target_dir.join(format!("{bin}-{target}"));
    let _ = std::fs::remove_file(&stable);

    for profile in ["debug", "release"] {
        let base = target_dir.join(target).join(profile);
        for sub in ["build", ".fingerprint", "incremental"] {
            let dir = base.join(sub);
            if sub == "incremental" {
                let _ = std::fs::remove_dir_all(&dir);
            } else if let Ok(rd) = std::fs::read_dir(&dir) {
                for ent in rd.flatten() {
                    let name = ent.file_name();
                    if name.to_string_lossy().starts_with("kernel-") {
                        let _ = std::fs::remove_dir_all(ent.path());
                        let _ = std::fs::remove_file(ent.path());
                    }
                }
            }
        }
        let host = target_dir.join(profile);
        for sub in ["build", ".fingerprint", "incremental"] {
            let dir = host.join(sub);
            if sub == "incremental" {
                let _ = std::fs::remove_dir_all(&dir);
            } else if let Ok(rd) = std::fs::read_dir(&dir) {
                for ent in rd.flatten() {
                    let name = ent.file_name();
                    if name.to_string_lossy().starts_with("kernel-") {
                        let _ = std::fs::remove_dir_all(ent.path());
                        let _ = std::fs::remove_file(ent.path());
                    }
                }
            }
        }
    }

    let td = target_dir.join(format!("{bin}-prelink-{target}"));
    let _ = std::fs::remove_dir_all(&td);
    let mut rustflags = if target.contains("aarch64") {
        String::from("-C panic=abort -C relocation-model=static")
    } else {
        String::from("-C panic=abort -C relocation-model=static -C code-model=medium")
    };
    rustflags.push_str(" -C link-arg=--image-base -C link-arg=0x40000000");
    let status = Command::new(cargo)
        .args([
            "build",
            "--manifest-path",
            root.join(format!("user/{bin}/Cargo.toml")).to_str().unwrap(),
            "--target",
            target,
            "--bin",
            bin,
            "--target-dir",
        ])
        .arg(&td)
        .env("RUSTFLAGS", &rustflags)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .status()
        .unwrap_or_else(|e| {
            eprintln!("error: spawn {bin} prelink for {target}: {e}");
            exit(1);
        });
    if !status.success() {
        eprintln!("error: {bin} prelink failed for {target}");
        exit(1);
    }
    let elf = td.join(target).join("debug").join(bin);
    if !elf.is_file() {
        eprintln!("error: {bin} prelink ELF missing at {}", elf.display());
        exit(1);
    }
    assert_elf_linked_at_user_base(&elf, bin, target);
    std::fs::create_dir_all(&target_dir).expect("target dir");
    std::fs::copy(&elf, &stable).unwrap_or_else(|e| {
        eprintln!("error: copy {bin} to {}: {e}", stable.display());
        exit(1);
    });
    stamp_user_into_kernel_outs(&elf, target, bin);
    eprintln!(
        "{bin} prelink ok: {} linked at USER_BASE (min PT_LOAD verified)",
        stable.display()
    );
}

fn stamp_user_into_kernel_outs(good: &Path, target: &str, bin: &str) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let nested_name = format!("{bin}-target");
    for profile in ["debug", "release"] {
        for build_root in [
            root.join(target).join(profile).join("build"),
            root.join(profile).join("build"),
        ] {
            let Ok(rd) = std::fs::read_dir(&build_root) else {
                continue;
            };
            for ent in rd.flatten() {
                if !ent.file_name().to_string_lossy().starts_with("kernel-") {
                    continue;
                }
                let out = ent.path().join("out");
                let nested = out
                    .join(&nested_name)
                    .join(target)
                    .join(profile)
                    .join(bin);
                if nested.is_file() {
                    let _ = std::fs::copy(good, &nested);
                }
                if let Ok(files) = std::fs::read_dir(&out) {
                    for f in files.flatten() {
                        let n = f.file_name();
                        let ns = n.to_string_lossy();
                        if ns.starts_with(&format!("{bin}-")) && ns.ends_with(".elf") {
                            let _ = std::fs::copy(good, f.path());
                        }
                    }
                }
            }
        }
    }
}

fn build_aarch64_kernel() -> PathBuf {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    // aarch64 kernels are not packed in ci-build.tar. rust-cache can keep a
    // pre--image-base ET_EXEC at 0x200000; cargo clean -p kernel alone may skip
    // build.rs. Prelink ping/netd at USER_BASE and invalidate fingerprints.
    ensure_user_at_user_base(&cargo, AARCH64_TARGET, "ping");
    ensure_user_at_user_base(&cargo, AARCH64_TARGET, "netd");
    let clean = Command::new(&cargo)
        .args(["clean", "-p", "kernel", "--target", AARCH64_TARGET])
        .status()
        .expect("failed to clean aarch64 kernel");
    if !clean.success() {
        eprintln!("error: cleaning aarch64 kernel failed");
        exit(1);
    }
    let mut cmd = Command::new(&cargo);
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
    let elf = target_dir.join(AARCH64_TARGET).join(profile).join("kernel");
    if !elf.is_file() {
        eprintln!("error: aarch64 kernel ELF missing at {}", elf.display());
        exit(1);
    }
    elf
}

fn run_riscv64() {
    let image = build_riscv64_image();
    let status = qemu_riscv64(&image, false)
        .status()
        .expect("failed to start qemu-system-riscv64");
    exit(status.code().unwrap_or(1));
}

fn riscv64_firmware() -> (PathBuf, PathBuf) {
    const CANDIDATES: &[(&str, &str)] = &[
        (
            "/usr/share/qemu-efi-riscv64/RISCV_VIRT_CODE.fd",
            "/usr/share/qemu-efi-riscv64/RISCV_VIRT_VARS.fd",
        ),
        (
            "/usr/share/edk2/riscv64/RISCV_VIRT_CODE.fd",
            "/usr/share/edk2/riscv64/RISCV_VIRT_VARS.fd",
        ),
    ];
    for (c, v) in CANDIDATES {
        if Path::new(c).is_file() {
            return (PathBuf::from(c), PathBuf::from(v));
        }
    }
    panic!(
        "no RISC-V UEFI firmware found. Install qemu-efi-riscv64 (Debian/Ubuntu) \
         or edk2-riscv64"
    );
}

fn run_ci_riscv64() {
    let image = build_riscv64_image();
    let child = qemu_riscv64(&image, true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start qemu-system-riscv64");
    wait_ci(
        child,
        CiExpect {
            timeout: Duration::from_secs(180),
            qemu_debug_exit: false,
            shell_ci: true,
        },
        // Same heavy `/heap` needles as x86 (typed at `$` after slim `/ok`).
        &CI_NEEDLES_STD,
    );
}

fn qemu_riscv64(image: &Path, ci: bool) -> Command {
    let (code, vars) = riscv64_firmware();
    let mut cmd = Command::new("qemu-system-riscv64");
    // 2G: initramfs + /heap find/cat/ls→rg pressure used to trip sepc=0 under 1G.
    cmd.arg("-global")
        .arg("virtio-mmio.force-legacy=false")
        .arg("-machine")
        .arg("virt")
        .arg("-cpu")
        .arg("rv64")
        .arg("-m")
        .arg("2048")
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,unit=0,file={},readonly=on",
            code.display()
        ))
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,unit=1,file={},snapshot=on",
            vars.display()
        ));
    add_virtio_blk_riscv64(&mut cmd);
    add_virtio_net(&mut cmd);
    cmd.arg("-drive")
        .arg(format!(
            "if=none,id=hd0,format=raw,file={}",
            image.display()
        ))
        .arg("-device")
        .arg("virtio-blk-device,drive=hd0,bootindex=1")
        .arg("-serial")
        .arg("stdio")
        .arg("-nic")
        .arg("none")
        .arg("-no-reboot");
    if ci {
        cmd.arg("-display").arg("none");
        cmd.arg("-monitor").arg("none");
    } else {
        cmd.arg("-device").arg("ramfb");
        cmd.arg("-device").arg("virtio-keyboard-device");
    }
    cmd
}

fn build_riscv64_image() -> PathBuf {
    let kernel = build_riscv64_kernel();
    let kernel_bytes = std::fs::read(&kernel).expect("read riscv64 kernel ELF");
    let hello_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/hello-riscv64imac-unknown-none-elf");
    let hello = std::fs::read(&hello_path)
        .unwrap_or_else(|_| panic!("hello ELF missing at {}", hello_path.display()));
    let ok_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ok-riscv64imac-unknown-none-elf");
    let ok = std::fs::read(&ok_path)
        .unwrap_or_else(|_| panic!("ok ELF missing at {}", ok_path.display()));
    let limine_dir = PathBuf::from(env!("LIMINE_DIR"));
    let limine = if limine_dir.join("BOOTRISCV64.EFI").is_file() {
        fetch_limine(&limine_dir)
    } else {
        let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("limine-v{LIMINE_VERSION}"));
        fetch_limine(&fallback)
    };
    let efi = std::fs::read(limine.bootriscv64()).expect("BOOTRISCV64.EFI");
    let dtb_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/virt.dtb");
    if !dtb_path.is_file() {
        let status = Command::new("qemu-system-riscv64")
            .args([
                "-machine",
                "virt,dumpdtb=target/virt.dtb",
                "-nographic",
                "-serial",
                "none",
            ])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .status()
            .expect("spawn qemu for virt.dtb");
        if !status.success() || !dtb_path.is_file() {
            panic!("failed to generate target/virt.dtb with qemu-system-riscv64");
        }
    }
    let dtb = std::fs::read(&dtb_path).expect("read virt.dtb");
    let image = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/riscv64.img");
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let initramfs = initramfs::build_initramfs(&manifest, "riscv64");
    write_esp_image_ex(
        &image,
        &kernel_bytes,
        "BOOTRISCV64.EFI",
        &efi,
        None,
        &hello,
        &ok,
        &initramfs,
        RISCV_LIMINE_CONF,
        &[DiskFile {
            path: "boot/virt.dtb".into(),
            data: dtb,
        }],
    );
    write_fat_data_image(&fat_img_path());
    image
}

fn build_riscv64_kernel() -> PathBuf {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    // Same rust-cache trap as aarch64 (fault at 0x11e6a).
    ensure_user_at_user_base(&cargo, RISCV64_TARGET, "ping");
    ensure_user_at_user_base(&cargo, RISCV64_TARGET, "netd");
    let clean = Command::new(&cargo)
        .args(["clean", "-p", "kernel", "--target", RISCV64_TARGET])
        .status()
        .expect("failed to clean riscv64 kernel");
    if !clean.success() {
        eprintln!("error: cleaning riscv64 kernel failed");
        exit(1);
    }
    let mut cmd = Command::new(&cargo);
    cmd.arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("-p")
        .arg("kernel")
        .arg("--bin")
        .arg("kernel")
        .arg("--target")
        .arg(RISCV64_TARGET);
    cmd.env(
        "RUSTFLAGS",
        "-C panic=abort -C relocation-model=static -C code-model=large",
    );
    if !cfg!(debug_assertions) {
        cmd.arg("--release");
    }
    let status = cmd
        .status()
        .expect("failed to invoke cargo for riscv64 kernel");
    if !status.success() {
        eprintln!("error: building riscv64 kernel failed");
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
    let elf = target_dir.join(RISCV64_TARGET).join(profile).join("kernel");
    if !elf.is_file() {
        eprintln!("error: riscv64 kernel ELF missing at {}", elf.display());
        exit(1);
    }
    elf
}

include!("wait_ci.rs");
