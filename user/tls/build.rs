use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../..");
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if arch.is_empty() {
        return;
    }

    let build_sh = root.join("ports/mbedtls/build.sh");
    println!("cargo:rerun-if-changed={}", build_sh.display());
    println!("cargo:rerun-if-changed={}", root.join("ports/mbedtls").display());

    let status = Command::new("bash")
        .arg(&build_sh)
        .current_dir(&root)
        .status()
        .expect("run ports/mbedtls/build.sh");
    if !status.success() {
        panic!("ports/mbedtls/build.sh failed");
    }

    let lib = root.join(format!("target/mbedtls-{arch}/lib"));
    let inc = root.join(format!("target/mbedtls-{arch}/include"));
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=static=mbedtls");
    println!("cargo:rustc-link-lib=static=mbedx509");
    println!("cargo:rustc-link-lib=static=mbedcrypto");
    println!("cargo:rerun-if-changed={}", lib.join("libmbedtls.a").display());

    // Compile platform glue (entropy/time hooks + thin C API).
    let glue = manifest.join("src/platform.c");
    println!("cargo:rerun-if-changed={}", glue.display());
    let mut cc = Command::new("clang");
    let triple = format!("{arch}-unknown-none");
    cc.arg("--target").arg(&triple)
        .arg("-ffreestanding").arg("-fno-builtin").arg("-fPIC").arg("-Os")
        .arg("-I").arg(&inc)
        .arg("-I").arg(root.join("ports/mbedtls"))
        .arg("-DMBEDTLS_CONFIG_FILE=\"mbedtls_config.h\"")
        .arg("-c").arg(&glue)
        .arg("-o").arg(env::var("OUT_DIR").unwrap() + "/platform.o");
    if arch == "riscv64" {
        cc.arg("-march=rv64imac").arg("-mabi=lp64");
    }
    let st = cc.status().expect("compile platform.c");
    if !st.success() {
        panic!("platform.c compile failed");
    }
    println!("cargo:rustc-link-arg={}/platform.o", env::var("OUT_DIR").unwrap());
}
