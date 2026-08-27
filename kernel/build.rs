fn main() {
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    if arch == "aarch64" {
        let script = format!("{manifest}/src/arch/aarch64/link.ld");
        println!("cargo:rustc-link-arg-bins=-T{script}");
        println!("cargo:rerun-if-changed={script}");
    }
}
