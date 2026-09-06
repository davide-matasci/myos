fn main() {
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=max-page-size=4096");
    println!("cargo:rustc-link-arg=-u");
    println!("cargo:rustc-link-arg=_start");

    if arch == "x86_64" {
        println!("cargo:rustc-link-arg=-pie");
        println!("cargo:rustc-link-arg=-nostdlib");
    } else if arch == "aarch64" || arch == "riscv64" {
        println!("cargo:warning=http: --image-base=0x40000000 for {arch}");
        println!("cargo:rustc-link-arg=--image-base");
        println!("cargo:rustc-link-arg=0x40000000");
    }
}
