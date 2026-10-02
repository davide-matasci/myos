fn main() {
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    // 4 KiB PT_LOAD spacing so the in-memory image stays small enough
    // for the kernel heap (AArch64 lld otherwise uses 64 KiB pages).
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=max-page-size=4096");

    // Keep `module_init` / `module_exit` from `--gc-sections`. Do not use
    // `--export-dynamic`: that exports all of libcore and the image jumps
    // to hundreds of KiB.
    println!("cargo:rustc-link-arg=-u");
    println!("cargo:rustc-link-arg=module_init");
    println!("cargo:rustc-link-arg=-u");
    println!("cargo:rustc-link-arg=module_exit");

    if arch == "x86_64" {
        // x86_64-unknown-none already prefers PIE; be explicit.
        println!("cargo:rustc-link-arg=-pie");
        println!("cargo:rustc-link-arg=-nostdlib");
    }
    // AArch64: prebuilt libcore is not PIC, so `-pie` fails to link
    // (`R_AARCH64_ABS64` in libcore). Produce ET_EXEC; the kernel slides
    // PT_LOAD as a unit. `module_init` uses PC-relative ADR.
}
