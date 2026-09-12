mod limine_image {
    include!("src/limine_image.rs");
}

mod initramfs {
    include!("src/initramfs.rs");
}

use limine_image::{bios_install, fetch_limine, write_esp_image, write_fat_data_image, LIMINE_VERSION};
use std::path::PathBuf;

/// Build a feature-gated port when its artifact is missing. Port build
/// scripts are stamped and early-exit when current, so this is cheap for
/// up-to-date trees. Ports whose feature is disabled are never built.
fn ensure_feature_port(manifest: &PathBuf, feature: &str, artifact: &str, script: &str) {
    if !initramfs::feature_enabled(feature) {
        return;
    }
    println!("cargo:rerun-if-changed={}", manifest.join(script).display());
    let artifact_path = manifest.join(artifact);
    if artifact_path.is_file() {
        return;
    }
    eprintln!("==> cargo: {feature} artifact missing ({artifact}); running {script}");
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
    println!("cargo:rerun-if-changed=src/limine_gpt.rs");
    println!("cargo:rerun-if-changed=src/limine_fat.rs");
    println!("cargo:rerun-if-changed=src/limine_dir.rs");
    println!("cargo:rerun-if-changed=src/initramfs.rs");
    println!("cargo:rerun-if-changed=ports/termcap/termcap");
    println!("cargo:rerun-if-changed=ports/lynx/lynx.cfg");
    println!("cargo:rerun-if-changed=kbd/ch.map");
    println!("cargo:rerun-if-changed=kbd/us.map");
    println!("cargo:rerun-if-changed={}", kernel_path.display());

    // Ensure Phase-1 git (+zlib) ELFs exist before packing initramfs. CI hooks
    // the same scripts from ci-build-kernels.sh; ISO/workflow may not list them
    // when the OAuth token lacks `workflow` scope to edit ci.yml/iso.yml.
    // Gated on the port_git feature: build --no-default-features excludes it.
    if initramfs::feature_enabled("port_git") {
        let git_elf = manifest.join("target/git-x86_64-unknown-none");
        if !git_elf.is_file() {
            let sh = manifest.join("ports/git/build.sh");
            let status = std::process::Command::new("bash")
                .arg(&sh)
                .env("MYOS_GIT_ARCHES", "x86_64")
                .status()
                .unwrap_or_else(|e| panic!("run {}: {e}", sh.display()));
            if !status.success() {
                panic!("{} failed", sh.display());
            }
        }
        println!("cargo:rerun-if-changed={}", git_elf.display());
        println!("cargo:rerun-if-changed=ports/git/build.sh");
        println!("cargo:rerun-if-changed=ports/zlib/build.sh");
    }

    // Auto-build feature-gated ports when their artifacts are missing. Ports
    // whose feature is disabled are skipped entirely (no build, no error);
    // each port build.sh handles its own deps and early-exits when current.
    // (std / c_hello / oksh are ensured in kernel/build.rs — the kernel embeds
    // them via env!(), and the kernel package has no feature knowledge.)
    ensure_feature_port(&manifest, "port_sbase", "target/sbase-manifest-x86_64.txt", "ports/sbase/build.sh");
    ensure_feature_port(&manifest, "port_coreutils", "target/coreutils-manifest-x86_64.txt", "ports/coreutils/build.sh");
    ensure_feature_port(&manifest, "port_tcc", "target/tcc-x86_64-unknown-myos", "ports/tcc/build.sh");
    ensure_feature_port(&manifest, "port_ripgrep", "target/rg-x86_64-unknown-myos", "ports/ripgrep/build.sh");
    ensure_feature_port(&manifest, "port_vim", "target/vim-x86_64-unknown-none", "ports/vim/build.sh");
    ensure_feature_port(&manifest, "port_make", "target/make-x86_64-unknown-none", "ports/make/build.sh");
    ensure_feature_port(&manifest, "port_lynx", "target/lynx-x86_64-unknown-none", "ports/lynx/build.sh");

    // Userspace ships as a newc cpio module. The kernel rebuilds whenever any
    // user ELF changes (its build.rs rerun-if-changed on every stable copy), so
    // the image (and thus the cpio) is rebuilt transitively here.
    let initramfs_bytes = initramfs::build_initramfs(&manifest, "x86_64");
    let initramfs_path = manifest.join("target/initramfs-x86_64.cpio");
    std::fs::write(&initramfs_path, &initramfs_bytes)
        .expect("write target/initramfs-x86_64.cpio");
    println!("cargo:rerun-if-changed={}", initramfs_path.display());

    let hello_path = manifest.join("target").join("hello-x86_64-unknown-none");
    println!("cargo:rerun-if-changed={}", hello_path.display());
    let hello = std::fs::read(&hello_path).unwrap_or_else(|_| {
        panic!("hello ELF missing at {}", hello_path.display())
    });

    let ok_path = manifest.join("target").join("ok-x86_64-unknown-none");
    println!("cargo:rerun-if-changed={}", ok_path.display());
    let ok = std::fs::read(&ok_path).unwrap_or_else(|_| {
        panic!("ok ELF missing at {}", ok_path.display())
    });

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
        &hello,
        &ok,
        &initramfs_bytes,
    );
    bios_install(&limine.tool(), &bios_path);

    let uefi_path = out_dir.join("uefi.img");
    std::fs::copy(&bios_path, &uefi_path).expect("copy hybrid image to uefi.img");

    let target_dir = manifest.join("target");
    let _ = std::fs::create_dir_all(&target_dir);
    // Stable paths for CI prebuilt boots: GHCR `kernels` restores these, but not
    // cargo's hashed OUT_DIR. Baking OUT_DIR into BIOS_PATH made master boot
    // jobs fail with "Could not open .../build/myos-*/out/bios.img" on a cache hit.
    let bios_stable = target_dir.join("bios.img");
    let uefi_stable = target_dir.join("uefi.img");
    std::fs::copy(&bios_path, &bios_stable).expect("copy bios.img to target/");
    std::fs::copy(&uefi_path, &uefi_stable).expect("copy uefi.img to target/");

    let fat_path = target_dir.join("fat.img");
    write_fat_data_image(&fat_path);

    println!("cargo:rustc-env=BIOS_PATH={}", bios_stable.display());
    println!("cargo:rustc-env=UEFI_PATH={}", uefi_stable.display());
    println!("cargo:rustc-env=LIMINE_DIR={}", limine_dir.display());
    // Artifact-dep kernel is not at target/<triple>/debug/kernel.
    println!("cargo:rustc-env=KERNEL_PATH={}", kernel_path.display());
    println!("cargo:rustc-env=HELLO_PATH={}", hello_path.display());
    println!("cargo:rustc-env=OK_PATH={}", ok_path.display());
    // Hand the active feature set to the host binary so wait_ci.rs can gate the
    // smoke-test needles at runtime (build scripts can't use #[cfg] on a
    // separate binary; the crate can, but this keeps one source of truth).
    println!("cargo:rustc-env=MYOS_FEATURES={}", initramfs::active_features().join(","));
}
