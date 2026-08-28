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

    // Hello, fat, init, and ok are their own tiny workspaces so nested `cargo build`
    // does not share the myos lock (and is not an artifact-dep: those panic
    // cargo's resolver when nested under the kernel artifact, and build-deps
    // cannot set panic=abort).
    let target = env::var("TARGET").expect("TARGET");
    let out = env::var("OUT_DIR").unwrap();
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let manifest = Path::new(&manifest_dir);

    nested_elf(
        &cargo,
        manifest,
        "../modules/hello",
        "hello",
        "hello-target",
        "HELLO_MODULE_PATH",
        &target,
        &profile,
        &out,
        &["../abi/src/lib.rs"],
    );
    nested_elf(
        &cargo,
        manifest,
        "../modules/fat",
        "fat",
        "fat-target",
        "FAT_MODULE_PATH",
        &target,
        &profile,
        &out,
        &["../abi/src/lib.rs"],
    );
    nested_elf(
        &cargo,
        manifest,
        "../user/init",
        "init",
        "init-target",
        "USER_INIT_PATH",
        &target,
        &profile,
        &out,
        &["../lib/src/lib.rs", "../lib/Cargo.toml"],
    );
    nested_elf(
        &cargo,
        manifest,
        "../user/ok",
        "ok",
        "ok-target",
        "USER_OK_PATH",
        &target,
        &profile,
        &out,
        &["../lib/src/lib.rs", "../lib/Cargo.toml"],
    );
}

fn nested_elf(
    cargo: &str,
    manifest_dir: &Path,
    crate_rel: &str,
    bin: &str,
    td_name: &str,
    env_key: &str,
    target: &str,
    profile: &str,
    out: &str,
    extra_rerun: &[&str],
) {
    let crate_dir = manifest_dir.join(crate_rel);
    println!("cargo:rerun-if-changed={}/src/main.rs", crate_dir.display());
    println!("cargo:rerun-if-changed={}/build.rs", crate_dir.display());
    println!("cargo:rerun-if-changed={}/Cargo.toml", crate_dir.display());
    for rel in extra_rerun {
        println!(
            "cargo:rerun-if-changed={}",
            crate_dir.join(rel).display()
        );
    }

    let td = PathBuf::from(out).join(td_name);
    let mut cmd = Command::new(cargo);
    cmd.arg("build")
        .arg("--manifest-path")
        .arg(crate_dir.join("Cargo.toml"))
        .arg("--target")
        .arg(target)
        .arg("--bin")
        .arg(bin)
        .arg("--target-dir")
        .arg(&td);
    if profile == "release" {
        cmd.arg("--release");
    }
    cmd.env("RUSTFLAGS", "-C panic=abort");
    cmd.env_remove("CARGO_ENCODED_RUSTFLAGS");
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn cargo for {bin}: {e}"));
    if !status.success() {
        panic!("{bin} failed to build for {target}");
    }

    let elf = td
        .join(target)
        .join(if profile == "release" {
            "release"
        } else {
            "debug"
        })
        .join(bin);
    if !elf.is_file() {
        panic!("{bin} ELF missing at {}", elf.display());
    }
    println!("cargo:rustc-env={env_key}={}", elf.display());

    // Stable path so the host image builder can also put the ELF on the ESP.
    let ws_target = manifest_dir.join("../target");
    std::fs::create_dir_all(&ws_target).expect("workspace target dir");
    let stable = ws_target.join(format!("{bin}-{target}"));
    std::fs::copy(&elf, &stable)
        .unwrap_or_else(|e| panic!("copy {bin} ELF to {}: {e}", stable.display()));
}
