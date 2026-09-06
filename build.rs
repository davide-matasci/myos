mod limine_image {
    include!("src/limine_image.rs");
}

mod initramfs {
    include!("src/initramfs.rs");
}

use limine_image::{bios_install, fetch_limine, write_esp_image, write_fat_data_image, LIMINE_VERSION};
use std::path::PathBuf;

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
    println!("cargo:rerun-if-changed={}", kernel_path.display());

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
    let _ = std::fs::copy(&bios_path, target_dir.join("bios.img"));
    let _ = std::fs::copy(&uefi_path, target_dir.join("uefi.img"));

    let fat_path = target_dir.join("fat.img");
    write_fat_data_image(&fat_path);

    println!("cargo:rustc-env=BIOS_PATH={}", bios_path.display());
    println!("cargo:rustc-env=UEFI_PATH={}", uefi_path.display());
    println!("cargo:rustc-env=LIMINE_DIR={}", limine_dir.display());
    // Artifact-dep kernel is not at target/<triple>/debug/kernel.
    println!("cargo:rustc-env=KERNEL_PATH={}", kernel_path.display());
    println!("cargo:rustc-env=HELLO_PATH={}", hello_path.display());
    println!("cargo:rustc-env=OK_PATH={}", ok_path.display());
}
