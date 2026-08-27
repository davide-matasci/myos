use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let kernel = PathBuf::from(std::env::var_os("CARGO_BIN_FILE_KERNEL_kernel").unwrap());

    let builder = bootloader::DiskImageBuilder::new(kernel);

    let bios_path = out_dir.join("bios.img");
    builder
        .create_bios_image(&bios_path)
        .expect("failed to create BIOS disk image");

    let uefi_path = out_dir.join("uefi.img");
    builder
        .create_uefi_image(&uefi_path)
        .expect("failed to create UEFI disk image");

    let target_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("target");
    let _ = std::fs::create_dir_all(&target_dir);
    let _ = std::fs::copy(&bios_path, target_dir.join("bios.img"));
    let _ = std::fs::copy(&uefi_path, target_dir.join("uefi.img"));

    println!("cargo:rustc-env=BIOS_PATH={}", bios_path.display());
    println!("cargo:rustc-env=UEFI_PATH={}", uefi_path.display());
}
