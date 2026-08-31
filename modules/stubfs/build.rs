fn main() {
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=max-page-size=4096");
    println!("cargo:rustc-link-arg=-u");
    println!("cargo:rustc-link-arg=module_init");
    println!("cargo:rustc-link-arg=-u");
    println!("cargo:rustc-link-arg=module_exit");

    if arch == "x86_64" {
        println!("cargo:rustc-link-arg=-pie");
        println!("cargo:rustc-link-arg=-nostdlib");
    }
}
