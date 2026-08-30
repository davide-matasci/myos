use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let script = format!("{manifest_dir}/link.ld");
    if arch == "aarch64" || arch == "x86_64" || arch == "riscv64" {
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
    for (crate_rel, bin, td, env_key) in [
        ("../user/ok", "ok", "ok-target", "USER_OK_PATH"),
        ("../user/sh", "sh", "sh-target", "USER_SH_PATH"),
        ("../user/heap", "heap", "heap-target", "USER_HEAP_PATH"),
        ("../user/echo", "echo", "echo-target", "USER_ECHO_PATH"),
        ("../user/cat", "cat", "cat-target", "USER_CAT_PATH"),
        ("../user/ls", "ls", "ls-target", "USER_LS_PATH"),
    ] {
        nested_elf(
            &cargo,
            manifest,
            crate_rel,
            bin,
            td,
            env_key,
            &target,
            &profile,
            &out,
            &["../lib/src/lib.rs", "../lib/Cargo.toml"],
        );
    }

    if arch == "x86_64" || arch == "aarch64" {
        for (artifact, env_key) in [
            ("std-hello", "USER_STD_HELLO_PATH"),
            ("std-cat", "USER_STD_CAT_PATH"),
            ("std-echo", "USER_STD_ECHO_PATH"),
        ] {
            embed_std_elf(manifest, &arch, artifact, env_key);
        }
        embed_c_elf(manifest, &arch, "c-hello", "USER_C_HELLO_PATH");
        for (artifact, env_key) in [
            ("sbase-echo", "USER_SBASE_ECHO_PATH"),
            ("sbase-cat", "USER_SBASE_CAT_PATH"),
            ("sbase-true", "USER_SBASE_TRUE_PATH"),
            ("sbase-ls", "USER_SBASE_LS_PATH"),
            ("sbase-false", "USER_SBASE_FALSE_PATH"),
            ("sbase-pwd", "USER_SBASE_PWD_PATH"),
            ("sbase-basename", "USER_SBASE_BASENAME_PATH"),
        ] {
            embed_c_elf(manifest, &arch, artifact, env_key);
        }
    } else if arch == "riscv64" {
        let stub = PathBuf::from(&out).join("std-stub.elf");
        std::fs::write(&stub, []).expect("write riscv64 std stub");
        for env_key in [
            "USER_STD_HELLO_PATH",
            "USER_STD_CAT_PATH",
            "USER_STD_ECHO_PATH",
            "USER_C_HELLO_PATH",
            "USER_SBASE_ECHO_PATH",
            "USER_SBASE_CAT_PATH",
            "USER_SBASE_TRUE_PATH",
            "USER_SBASE_LS_PATH",
            "USER_SBASE_FALSE_PATH",
            "USER_SBASE_PWD_PATH",
            "USER_SBASE_BASENAME_PATH",
        ] {
            println!("cargo:rustc-env={env_key}={}", stub.display());
        }
    }
}

fn embed_std_elf(manifest_dir: &Path, arch: &str, artifact: &str, env_key: &str) {
    let triple = if arch == "x86_64" {
        "x86_64-unknown-myos"
    } else if arch == "aarch64" {
        "aarch64-unknown-myos"
    } else {
        return;
    };
    let stable = manifest_dir
        .join("../target")
        .join(format!("{artifact}-{triple}"));
    println!("cargo:rerun-if-changed={}", stable.display());
    if !stable.is_file() {
        panic!(
            "{artifact} ELF missing at {} (run ./scripts/build-std-hello.sh)",
            stable.display()
        );
    }
    println!("cargo:rustc-env={env_key}={}", stable.display());
}

fn embed_c_elf(manifest_dir: &Path, arch: &str, artifact: &str, env_key: &str) {
    let triple = if arch == "x86_64" {
        "x86_64-unknown-none"
    } else if arch == "aarch64" {
        "aarch64-unknown-none"
    } else {
        return;
    };
    let stable = manifest_dir
        .join("../target")
        .join(format!("{artifact}-{triple}"));
    println!("cargo:rerun-if-changed={}", stable.display());
    if !stable.is_file() {
        panic!(
            "{artifact} ELF missing at {} (run ./scripts/build-c-hello.sh and ./scripts/build-sbase.sh)",
            stable.display()
        );
    }
    println!("cargo:rustc-env={env_key}={}", stable.display());
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
    // Nested target dirs reuse myos-user rlibs aggressively; drop deps when
    // the shared user library changed so fork/exec stubs stay in sync.
    let profile_dir = if profile == "release" { "release" } else { "debug" };
    let deps = td.join(target).join(profile_dir).join("deps");
    let _ = std::fs::remove_dir_all(deps);
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
    if target.contains("riscv64") {
        cmd.env(
            "RUSTFLAGS",
            "-C panic=abort -C relocation-model=static -C code-model=medium",
        );
    }
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
    if env_key != "_unused" {
        println!("cargo:rustc-env={env_key}={}", elf.display());
    }
    let ws_target = manifest_dir.join("../target");
    std::fs::create_dir_all(&ws_target).expect("workspace target dir");
    let stable = ws_target.join(format!("{bin}-{target}"));
    std::fs::copy(&elf, &stable)
        .unwrap_or_else(|e| panic!("copy {bin} ELF to {}: {e}", stable.display()));
}
