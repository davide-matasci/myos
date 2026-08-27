use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let script = format!("{manifest_dir}/link.ld");
    if arch == "aarch64" || arch == "x86_64" {
        println!("cargo:rustc-link-arg-bins=-T{script}");
        println!("cargo:rerun-if-changed={script}");
        println!("cargo:rustc-link-arg-bins=-z");
        println!("cargo:rustc-link-arg-bins=max-page-size=0x1000");
    }

    // Hello is its own tiny workspace so this nested `cargo build` does not
    // share the myos lock (and is not an artifact-dep: those panic cargo's
    // resolver when nested under the kernel artifact, and build-deps cannot
    // set panic=abort).
    let hello_dir = Path::new(&manifest_dir).join("../modules/hello");
    println!("cargo:rerun-if-changed={}/src/main.rs", hello_dir.display());
    println!("cargo:rerun-if-changed={}/build.rs", hello_dir.display());
    println!("cargo:rerun-if-changed={}/Cargo.toml", hello_dir.display());
    println!("cargo:rerun-if-changed={}/../abi/src/lib.rs", hello_dir.display());

    let target = env::var("TARGET").expect("TARGET");
    let out = env::var("OUT_DIR").unwrap();
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let hello_td = PathBuf::from(&out).join("hello-target");

    let mut cmd = Command::new(&cargo);
    cmd.arg("build")
        .arg("--manifest-path")
        .arg(hello_dir.join("Cargo.toml"))
        .arg("--target")
        .arg(&target)
        .arg("--bin")
        .arg("hello")
        .arg("--target-dir")
        .arg(&hello_td);
    if profile == "release" {
        cmd.arg("--release");
    }
    cmd.env("RUSTFLAGS", "-C panic=abort");
    cmd.env_remove("CARGO_ENCODED_RUSTFLAGS");
    let status = cmd
        .status()
        .expect("failed to spawn cargo for hello module");
    if !status.success() {
        panic!("hello module failed to build for {target}");
    }

    let elf = hello_td
        .join(&target)
        .join(if profile == "release" {
            "release"
        } else {
            "debug"
        })
        .join("hello");
    if !elf.is_file() {
        panic!("hello ELF missing at {}", elf.display());
    }
    println!("cargo:rustc-env=HELLO_MODULE_PATH={}", elf.display());
}
