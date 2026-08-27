include!("src/limine_image.rs");

use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let kernel_path = PathBuf::from(std::env::var_os("CARGO_BIN_FILE_KERNEL_kernel").unwrap());
    let kernel = std::fs::read(&kernel_path).expect("read x86_64 kernel ELF");

    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let limine_dir = manifest.join("target").join(format!("limine-v{LIMINE_VERSION}"));
    println!("cargo:rerun-if-changed={}", kernel_path.display());

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
    );
    bios_install(&limine.tool(), &bios_path);

    let uefi_path = out_dir.join("uefi.img");
    std::fs::copy(&bios_path, &uefi_path).expect("copy hybrid image to uefi.img");

    let target_dir = manifest.join("target");
    let _ = std::fs::create_dir_all(&target_dir);
    let _ = std::fs::copy(&bios_path, target_dir.join("bios.img"));
    let _ = std::fs::copy(&uefi_path, target_dir.join("uefi.img"));

    println!("cargo:rustc-env=BIOS_PATH={}", bios_path.display());
    println!("cargo:rustc-env=UEFI_PATH={}", uefi_path.display());
    println!("cargo:rustc-env=LIMINE_DIR={}", limine_dir.display());
}
