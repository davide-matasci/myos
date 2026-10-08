mod limine_image {
    include!("src/limine_image.rs");
}

mod initramfs {
    include!("src/initramfs.rs");
}
#[allow(dead_code)]
mod ports {
    include!("src/ports.rs");
}
mod release {
    include!("src/release.rs");
}

use limine_image::{
    all_modules, bios_install, copy_sparse, fetch_limine, write_esp_image,
    write_fat_data_image, LIMINE_VERSION,
};
use std::path::PathBuf;

/// Build a feature-gated port when its artifact is missing. Port build
/// scripts are stamped and early-exit when current, so this is cheap for
/// up-to-date trees. Ports whose feature is disabled are never built.
/// Run `script` (repo-relative) when `artifact` (repo-relative) is missing.
fn ensure_artifact(manifest: &PathBuf, artifact: &str, script: &str) {
    println!("cargo:rerun-if-changed={}", manifest.join(script).display());
    let artifact_path = manifest.join(artifact);
    if artifact_path.exists() {
        return;
    }
    eprintln!("==> cargo: artifact missing ({artifact}); running {script}");
    // Strip cargo-injected env so the nested cargo probes targets cleanly.
    let status = std::process::Command::new("bash")
        .arg(manifest.join(script))
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", std::env::var("HOME").unwrap_or_default())
        .status()
        .unwrap_or_else(|e| panic!("run {script}: {e}"));
    if !status.success() {
        panic!("{script} failed");
    }
}

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let kernel_path = PathBuf::from(std::env::var_os("CARGO_BIN_FILE_KERNEL_kernel").unwrap());
    let kernel = std::fs::read(&kernel_path).expect("read x86_64 kernel ELF");

    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let limine_dir = manifest.join("target").join(format!("limine-v{LIMINE_VERSION}"));
    // include! pulls these into the build script; cargo does not track them
    // automatically, so image-layout fixes must force a bios.img rebuild.
    println!("cargo:rerun-if-changed=src/limine_image.rs");
    // The security policy is packed as /etc/policy (src/initramfs.rs).
    println!("cargo:rerun-if-changed=etc/policy");
    println!("cargo:rerun-if-changed=src/limine_gpt.rs");
    println!("cargo:rerun-if-changed=src/limine_fat.rs");
    println!("cargo:rerun-if-changed=src/limine_dir.rs");
    println!("cargo:rerun-if-changed=src/limine_disk.rs");
    println!("cargo:rerun-if-changed=src/initramfs.rs");
    println!("cargo:rerun-if-changed=src/ports.rs");
    println!("cargo:rerun-if-changed=src/release.rs");
    // lib/myos-release names the commit: a new commit or checkout re-packs.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/logs/HEAD");
    // A new or moved port directory (ports/ <-> packages/) changes the image.
    println!("cargo:rerun-if-changed=ports");
    println!("cargo:rerun-if-changed=packages");
    println!("cargo:rerun-if-changed=modules/console/keymaps/ch.map");
    println!("cargo:rerun-if-changed=modules/console/keymaps/us.map");
    println!("cargo:rerun-if-changed={}", kernel_path.display());

    // Every port of the image (ports/, user/, toolchain/): run its build
    // script when its outputs are missing (the scripts skip themselves when
    // current), and rebuild the images when a descriptor, a build script or a
    // checked-in file a port ships changes. Packages (packages/) are not
    // built here; CI builds and publishes them.
    let all_ports = ports::load_all(&manifest);
    for port in all_ports.iter().filter(|p| p.role == ports::Role::Image) {
        println!("cargo:rerun-if-changed={}", manifest.join(&port.dir).join("port.env").display());
        for f in &port.files {
            if let ports::FileSpec::File { src, .. } = f {
                println!("cargo:rerun-if-changed={}", manifest.join(&port.dir).join(src).display());
            }
        }
        // The kernel's build script builds the user programs; the ports'
        // scripts bring the toolchains (newlib, the sysroot) themselves.
        if matches!(port.kind, ports::Kind::User | ports::Kind::Toolchain) {
            continue;
        }
        let Some(script) = &port.build else {
            continue;
        };
        let Some(ready) = port.ready_file("x86_64") else {
            continue;
        };
        ensure_artifact(&manifest, &format!("target/{ready}"), script);
    }
    // Userspace ships as a newc cpio module. The kernel rebuilds whenever any
    // user ELF changes (its build.rs rerun-if-changed on every stable copy), so
    // the image (and thus the cpio) is rebuilt transitively here.
    let initramfs_bytes = initramfs::build_initramfs(&manifest, "x86_64");
    let initramfs_path = manifest.join("target/initramfs-x86_64.cpio");
    std::fs::write(&initramfs_path, &initramfs_bytes)
        .expect("write target/initramfs-x86_64.cpio");
    println!("cargo:rerun-if-changed={}", initramfs_path.display());

    for m in all_modules() {
        println!(
            "cargo:rerun-if-changed={}",
            manifest.join("target").join(format!("{m}-x86_64-unknown-none")).display()
        );
    }

    let limine = fetch_limine(&limine_dir);
    let bootx64 = std::fs::read(limine.bootx64()).expect("BOOTX64.EFI");
    let bios_sys = std::fs::read(limine.bios_sys()).expect("limine-bios.sys");

    let bios_path = out_dir.join("bios.img");
    write_esp_image(
        &bios_path,
        &kernel,
        "BOOTX64.EFI",
        &bootx64,
        Some(&bios_sys),
        &initramfs_bytes,
    );
    bios_install(&limine.tool(), &bios_path);

    let uefi_path = out_dir.join("uefi.img");
    copy_sparse(&bios_path, &uefi_path);

    let target_dir = manifest.join("target");
    let _ = std::fs::create_dir_all(&target_dir);
    // Stable paths for CI prebuilt boots: GHCR `kernels` restores these, but not
    // cargo's hashed OUT_DIR. Baking OUT_DIR into BIOS_PATH made master boot
    // jobs fail with "Could not open .../build/myos-*/out/bios.img" on a cache hit.
    let bios_stable = target_dir.join("bios.img");
    let uefi_stable = target_dir.join("uefi.img");
    copy_sparse(&bios_path, &bios_stable);
    copy_sparse(&uefi_path, &uefi_stable);

    let fat_path = target_dir.join("fat.img");
    write_fat_data_image(&fat_path);

    println!("cargo:rustc-env=BIOS_PATH={}", bios_stable.display());
    println!("cargo:rustc-env=UEFI_PATH={}", uefi_stable.display());
    println!("cargo:rustc-env=LIMINE_DIR={}", limine_dir.display());
    // Artifact-dep kernel is not at target/<triple>/debug/kernel.
    println!("cargo:rustc-env=KERNEL_PATH={}", kernel_path.display());
    // Hand the active feature set to the host binary (the boot test's QEMU
    // budget depends on the Linux layer; build scripts can't use #[cfg] on a
    // separate binary; the crate can, but this keeps one source of truth).
    println!("cargo:rustc-env=MYOS_FEATURES={}", initramfs::active_features().join(","));
}
