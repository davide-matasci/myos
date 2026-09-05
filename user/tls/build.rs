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
    let newlib_inc = root.join(format!("target/newlib-{arch}/{arch}-unknown-myos/include"));
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=static=mbedtls");
    println!("cargo:rustc-link-lib=static=mbedx509");
    println!("cargo:rustc-link-lib=static=mbedcrypto");
    println!("cargo:rerun-if-changed={}", lib.join("libmbedtls.a").display());

    let glue = manifest.join("src/platform.c");
    println!("cargo:rerun-if-changed={}", glue.display());
    let out_dir = env::var("OUT_DIR").unwrap();
    let obj = format!("{out_dir}/platform.o");
    // Prefer myos clang wrapper when available (newlib headers).
    let cc = format!("{arch}-unknown-myos-cc");
    let mut cmd = Command::new(&cc);
    if Command::new(&cc).arg("--version").output().is_err() {
        cmd = Command::new("clang");
        cmd.arg(format!("--target={arch}-unknown-none"));
    }
    cmd.arg("-ffreestanding")
        .arg("-fPIC")
        .arg("-Os")
        .arg("-isystem")
        .arg(&newlib_inc)
        .arg("-I")
        .arg(&inc)
        .arg("-I")
        .arg(root.join("ports/mbedtls"))
        .arg("-DMBEDTLS_CONFIG_FILE=\"mbedtls_config.h\"")
        .arg("-c")
        .arg(&glue)
        .arg("-o")
        .arg(&obj);
    let st = cmd.status().expect("compile platform.c");
    if !st.success() {
        panic!("platform.c compile failed");
    }
    println!("cargo:rustc-link-arg={obj}");
}
