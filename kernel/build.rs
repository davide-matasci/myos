use std::env;
use std::path::{Path, PathBuf};

#[allow(dead_code)]
mod ports {
    include!("../src/ports.rs");
}
use std::process::Command;

/// Kernel modules, `(directory under modules/, bin name)`, in build order.
const MODULES: &[(&str, &str)] = &[
    ("console", "console"),
    ("hello", "hello"),
    ("pci_enum", "pci_enum"),
    ("acpi", "acpi"),
    ("rpi_firmware", "rpi_firmware"),
    ("virtio_blk", "virtio_blk"),
    ("virtio_scsi", "virtio_scsi"),
    ("nvme", "nvme"),
    ("sdmmc", "sdmmc"),
    ("sound", "sound"),
    ("virtio_sound", "virtio_sound"),
    ("usb", "usb"),
    ("xhci", "xhci"),
    ("ehci", "ehci"),
    ("uhci", "uhci"),
    ("dwc2", "dwc2"),
    ("usb_hub", "usb_hub"),
    ("usb_storage", "usb_storage"),
    ("usb_hid", "usb_hid"),
    ("usb_audio", "usb_audio"),
    ("usbdev", "usbdev"),
    ("pcidev", "pcidev"),
    ("pl061", "pl061"),
    ("tap", "tap"),
    ("virtio_net", "virtio_net"),
    ("netfs", "netfs"),
    ("fat", "fat"),
    ("ext2", "ext2"),
    ("linux", "linux"),
];

fn main() {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let script = format!("{manifest_dir}/link.ld");
    if arch == "aarch64" || arch == "x86_64" || arch == "riscv64" {
        println!("cargo:rustc-link-arg-bins=-T{script}");
        println!("cargo:rerun-if-changed={script}");
        println!("cargo:rustc-link-arg-bins=-z");
        println!("cargo:rustc-link-arg-bins=max-page-size=0x1000");
    }

    // Hello, fat, init, and ok are their own tiny workspaces so nested `cargo build`
    // does not share the myos lock (and is not an artifact-dep: those panic
    // cargo's resolver when nested under the kernel artifact, and build-deps
    // cannot set panic=abort).
    let target = env::var("TARGET").expect("TARGET");
    let out = env::var("OUT_DIR").unwrap();
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let manifest = Path::new(&manifest_dir);

    // Kernel modules: built here so `target/<module>-<triple>` exists for the
    // initramfs (`/lib/modules`: the kernel loads the boot ones from there,
    // `insmod` the others). The kernel embeds none of them.
    for &(dir, bin) in MODULES {
        let _ = nested_elf(
            &cargo,
            manifest,
            &format!("../modules/{dir}"),
            bin,
            &format!("{bin}-target"),
            &target,
            &profile,
            &out,
            // `.`: crates inside the module's directory too (ext2fs, ps2-scancode).
            &[".", "../abi/src/lib.rs", "../virtq/src/lib.rs"],
        );
    }
    // The Rust userspace programs: every `user/<name>/port.env` (and a
    // `packages/<name>/port.env` of kind `user`), init among them, built for
    // this arch into `target/<bin>-<triple>`, where the initramfs packer
    // takes them. The kernel embeds none of them.
    let repo = manifest.join("..");
    println!("cargo:rerun-if-changed={}", repo.join("src/ports.rs").display());
    for port in ports::load_all(&repo) {
        if port.kind != ports::Kind::User {
            continue;
        }
        println!("cargo:rerun-if-changed={}", repo.join(&port.dir).join("port.env").display());
        let crate_rel = format!("../{}", port.dir.display());
        if let Some(script) = &port.prepare {
            prepare(&repo, script, &port.bin);
        }
        let watch: Vec<String> = port.watch.clone();
        let watch_refs: Vec<&str> = watch.iter().map(String::as_str).collect();
        nested_elf(
            &cargo,
            manifest,
            &crate_rel,
            &port.bin,
            &format!("{}-target", port.name),
            &target,
            &profile,
            &out,
            &watch_refs,
        );
    }

}

/// Run a user program's `PORT_PREPARE` script (repo-relative), unless the
/// program comes prebuilt (`MYOS_PREBUILT`, see `nested_elf`).
fn prepare(repo: &Path, script: &str, bin: &str) {
    if env::var("MYOS_PREBUILT").is_ok_and(|v| v.split(',').any(|b| b == bin)) {
        return;
    }
    let path = repo.join(script);
    println!("cargo:rerun-if-changed={}", path.display());
    let status = Command::new("bash")
        .arg(&path)
        .status()
        .unwrap_or_else(|e| panic!("failed to run {script} for {bin}: {e}"));
    if !status.success() {
        panic!("{script} failed for {bin}");
    }
}

fn nested_elf(
    cargo: &str,
    manifest_dir: &Path,
    crate_rel: &str,
    bin: &str,
    td_name: &str,
    target: &str,
    profile: &str,
    out: &str,
    extra_rerun: &[&str],
) {
    let crate_dir = manifest_dir.join(crate_rel);
    // The whole source tree (a directory is watched recursively).
    println!("cargo:rerun-if-changed={}/src", crate_dir.display());
    println!("cargo:rerun-if-changed={}/build.rs", crate_dir.display());
    println!("cargo:rerun-if-changed={}/Cargo.toml", crate_dir.display());
    for rel in extra_rerun {
        println!(
            "cargo:rerun-if-changed={}",
            crate_dir.join(rel).display()
        );
    }

    // A kernel built inside myos (linux-compat/self-host.sh) takes a program
    // it cannot build there from the running system: MYOS_PREBUILT lists
    // them, each already at target/<bin>-<target>.
    println!("cargo:rerun-if-env-changed=MYOS_PREBUILT");
    if env::var("MYOS_PREBUILT").is_ok_and(|v| v.split(',').any(|b| b == bin)) {
        let stable = stable_elf(manifest_dir, bin, target);
        assert!(stable.is_file(), "MYOS_PREBUILT names {bin}, but {} is missing", stable.display());
        println!("cargo:rerun-if-changed={}", stable.display());
        return;
    }

    let is_module = MODULES.iter().any(|(_, m)| *m == bin);
    // A user program is built for its arch's hard-float bare target (it
    // links the userspace's C: http's LibreSSL), the kernel's modules for
    // the kernel's soft-float one; either lands at `target/<bin>-<target>`
    // (the kernel's triple, `{kernel}` in the port descriptors).
    let build_target = if is_module { target } else { user_target(target) };
    let td = PathBuf::from(out).join(td_name);
    // Nested target dirs reuse myos-user rlibs aggressively; drop deps when
    // the shared user library changed so fork/exec stubs stay in sync.
    let profile_dir = if profile == "release" { "release" } else { "debug" };
    let deps = td.join(build_target).join(profile_dir).join("deps");
    let _ = std::fs::remove_dir_all(deps);
    let mut cmd = Command::new(cargo);
    cmd.arg("build")
        .arg("--manifest-path")
        .arg(crate_dir.join("Cargo.toml"))
        .arg("--target")
        .arg(build_target)
        .arg("--bin")
        .arg(bin)
        .arg("--target-dir")
        .arg(&td);
    if profile == "release" {
        cmd.arg("--release");
    }
    let mut rustflags = String::from("-C panic=abort");
    // ext2's and fat's runtime-sized copies pull libcore panic fmt; x86 PIE needs PIC.
    if target.contains("x86_64")
        && matches!(
            bin,
            "ext2" | "fat" | "virtio_net" | "netfs" | "pci_enum" | "acpi" | "rpi_firmware" | "virtio_blk" | "virtio_scsi" | "nvme" | "sdmmc" | "sound" | "virtio_sound"
                | "linux" | "usb" | "xhci" | "ehci" | "uhci" | "dwc2" | "usb_hub" | "usb_storage" | "usb_hid" | "usb_audio" | "usbdev" | "pcidev" | "pl061" | "tap"
        )
    {
        rustflags = String::from("-C panic=abort -C relocation-model=pic");
    }
    if target.contains("aarch64") {
        // Match .cargo/config.toml; RUSTFLAGS replaces target rustflags entirely.
        rustflags = String::from("-C panic=abort -C relocation-model=static");
    }
    if target.contains("riscv64") {
        rustflags = String::from(
            "-C panic=abort -C relocation-model=static -C code-model=medium",
        );
    }
    // Kernel modules are PIEs on every arch, so the loader applies real
    // RELATIVE relocs (a module loads wherever the kernel heap has room,
    // which a static ET_EXEC could not follow). The prebuilt libcore
    // is not PIC on aarch64/riscv64; `-z notext` lets lld emit dynamic
    // relocs for its read-only sections (the loader relocates before use).
    if is_module && (target.contains("aarch64") || target.contains("riscv64")) {
        rustflags = String::from(
            "-C panic=abort -C relocation-model=pic -C link-arg=-pie -C link-arg=-z -C link-arg=notext",
        );
    }
    // The wifi daemon's AES (user/wifi/wpa): x86_64-unknown-none has no
    // SSE, which the aes crate's AES-NI code needs; its portable code then.
    if bin == "wifi" && target.contains("x86_64") {
        rustflags.push_str(" --cfg aes_force_soft");
    }
    cmd.env("RUSTFLAGS", rustflags);
    cmd.env_remove("CARGO_ENCODED_RUSTFLAGS");
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn cargo for {bin}: {e}"));
    if !status.success() {
        panic!("{bin} failed to build for {target}");
    }

    let elf = td
        .join(build_target)
        .join(if profile == "release" {
            "release"
        } else {
            "debug"
        })
        .join(bin);
    if !elf.is_file() {
        panic!("{bin} ELF missing at {}", elf.display());
    }
    let stable = stable_elf(manifest_dir, bin, target);
    std::fs::copy(&elf, &stable)
        .unwrap_or_else(|e| panic!("copy {bin} ELF to {}: {e}", stable.display()));
}

/// The bare target a user program is built for on the kernel's `target`:
/// the same arch with the userspace's hard float (`docs/ports.md`).
fn user_target(target: &str) -> &str {
    match target {
        "aarch64-unknown-none-softfloat" => "aarch64-unknown-none",
        "riscv64imac-unknown-none-elf" => "riscv64gc-unknown-none-elf",
        t => t,
    }
}

/// `bin`'s ELF for `target` where the image builders take it:
/// `target/<bin>-<target>`.
fn stable_elf(manifest_dir: &Path, bin: &str, target: &str) -> PathBuf {
    let ws_target = manifest_dir.join("../target");
    std::fs::create_dir_all(&ws_target).expect("workspace target dir");
    ws_target.join(format!("{bin}-{target}"))
}
