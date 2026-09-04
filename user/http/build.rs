fn main() {
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    // 4 KiB PT_LOAD spacing so the in-memory image stays small enough
    // for the kernel heap (AArch64 lld otherwise uses 64 KiB pages).
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=max-page-size=4096");

    // Keep `_start` from `--gc-sections`. Do not use `--export-dynamic`:
    // that exports all of libcore and the image jumps to hundreds of KiB.
    println!("cargo:rustc-link-arg=-u");
    println!("cargo:rustc-link-arg=_start");

    if arch == "x86_64" {
        // x86_64-unknown-none already prefers PIE; be explicit.
        println!("cargo:rustc-link-arg=-pie");
        println!("cargo:rustc-link-arg=-nostdlib");
    } else if arch == "aarch64" || arch == "riscv64" {
        // ET_EXEC (libcore is not PIC, so -pie fails). Kernel slides PT_LOAD
        // to USER_BASE without applying abs relocs. Link at USER_BASE so
        // absolute pointers match the load address.
        println!("cargo:warning=http: --image-base=0x40000000 for {arch}");
        println!("cargo:rustc-link-arg=--image-base");
        println!("cargo:rustc-link-arg=0x40000000");
    }
}