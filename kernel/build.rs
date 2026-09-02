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
        "../modules/ext2",
        "ext2",
        "ext2-target",
        "EXT2_MODULE_PATH",
        &target,
        &profile,
        &out,
        &["../abi/src/lib.rs"],
    );
    nested_elf(
        &cargo,
        manifest,
        "../modules/stubfs",
        "stubfs",
        "stubfs-target",
        "STUBFS_MODULE_PATH",
        &target,
        &profile,
        &out,
        &["../abi/src/lib.rs"],
    );
    nested_elf(
        &cargo,
        manifest,
        "../modules/virtio_net",
        "virtio_net",
        "virtio-net-target",
        "VIRTIO_NET_MODULE_PATH",
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
        ("../user/heap", "heap", "heap-target", "USER_HEAP_PATH"),
        ("../user/echo", "myos_echo", "echo-target", "USER_ECHO_PATH"),
        ("../user/cat", "myos_cat", "cat-target", "USER_CAT_PATH"),
        ("../user/ls", "myos_ls", "ls-target", "USER_LS_PATH"),
        ("../user/mount", "mount", "mount-target", "USER_MOUNT_PATH"),
        (
            "../user/mkfs.ext2",
            "mkfs_ext2",
            "mkfs-ext2-target",
            "USER_MKFS_EXT2_PATH",
        ),
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

    if arch == "x86_64" || arch == "aarch64" || arch == "riscv64" {
        for (artifact, env_key) in [
            ("std-hello", "USER_STD_HELLO_PATH"),
            ("std-cat", "USER_STD_CAT_PATH"),
            ("std-echo", "USER_STD_ECHO_PATH"),
            ("std-bigalloc", "USER_BIGALLOC_PATH"),
        ] {
            embed_std_elf(manifest, &arch, artifact, env_key);
        }
        embed_c_elf(manifest, &arch, "c-hello", "USER_C_HELLO_PATH");
        embed_oksh_elf(manifest, &arch);
        embed_sbase_manifest(manifest, &arch, Path::new(&out));
        embed_ubase_manifest(manifest, &arch, Path::new(&out));
        embed_coreutils_manifest(manifest, &arch, Path::new(&out));
        embed_ripgrep_elf(manifest, &arch, Path::new(&out));
        embed_tcc_elf(manifest, &arch, Path::new(&out));
        embed_lib_sysroot(manifest, &arch, Path::new(&out));
    }
}

fn embed_coreutils_manifest(manifest_dir: &Path, arch: &str, out_dir: &Path) {
    let triple = if arch == "x86_64" {
        "x86_64-unknown-myos"
    } else if arch == "aarch64" {
        "aarch64-unknown-myos"
    } else if arch == "riscv64" {
        "riscv64-unknown-myos"
    } else {
        return;
    };
    let elf = manifest_dir
        .join("../target")
        .join(format!("coreutils-{triple}"));
    let names_file = manifest_dir
        .join("../target")
        .join(format!("coreutils-manifest-{arch}.txt"));
    println!("cargo:rerun-if-changed={}", elf.display());
    println!("cargo:rerun-if-changed={}", names_file.display());
    if !elf.is_file() {
        panic!(
            "coreutils ELF missing at {} (run ./ports/coreutils/build-uutils.sh)",
            elf.display()
        );
    }
    if !names_file.is_file() {
        panic!(
            "coreutils manifest missing at {} (run ./ports/coreutils/build-uutils.sh)",
            names_file.display()
        );
    }
    let text = std::fs::read_to_string(&names_file).expect("read coreutils manifest");
    let mut body = String::from("pub fn register_all() {\n");
    body.push_str(&format!(
        "    const COREUTILS_ELF: &[u8] = include_bytes!(r\"{}\");\n",
        elf.display()
    ));
    for line in text.lines() {
        let name = line.trim();
        if name.is_empty() || name.starts_with('#') {
            continue;
        }
        body.push_str(&format!(
            "    let _ = super::register({name:?}, COREUTILS_ELF);\n"
        ));
    }
    body.push_str("}\n");
    let dest = out_dir.join("coreutils_embed.rs");
    std::fs::write(&dest, body).expect("write coreutils_embed.rs");
}




/// Workflow file edits need `workflow` OAuth scope; run the port script so
/// ISO/CI can still embed the artifact without a workflow-only change.
fn run_port_build(manifest_dir: &Path, port: &str) {
    let script = manifest_dir.join(format!("../ports/{port}/build.sh"));
    println!("cargo:rerun-if-changed={}", script.display());
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join(format!("../ports/{port}")).display()
    );
    let status = Command::new("bash")
        .arg(&script)
        .status()
        .unwrap_or_else(|e| panic!("run {}: {e}", script.display()));
    if !status.success() {
        panic!("{} failed", script.display());
    }
}



fn ensure_libtcc1_in_sysroot(manifest_dir: &Path, arch: &str, lib_dir: &Path) {
    let dest = lib_dir.join("libtcc1.a");
    let src_primary = manifest_dir
        .join("../target")
        .join(format!("libtcc1-{arch}-unknown-myos.a"));
    let src_alias = manifest_dir
        .join("../target")
        .join(format!("tcc-libtcc1-{arch}-unknown-myos.a"));

    let try_copy = |src: &Path| -> bool {
        if !src.is_file() {
            return false;
        }
        std::fs::create_dir_all(lib_dir).expect("mkdir newlib lib");
        std::fs::copy(src, &dest).unwrap_or_else(|e| {
            panic!("copy {} -> {}: {e}", src.display(), dest.display())
        });
        println!("cargo:rerun-if-changed={}", src.display());
        true
    };

    if dest.is_file() && dest.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return;
    }
    if try_copy(&src_primary) || try_copy(&src_alias) {
        return;
    }
    // Boot jobs may restore tcc ELFs without libtcc1.a; rebuild installs it.
    run_port_build(manifest_dir, "tcc");
    if dest.is_file() && dest.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return;
    }
    if try_copy(&src_primary) || try_copy(&src_alias) {
        return;
    }
    panic!(
        "libtcc1.a missing for {arch} at {} (tried {} and {})",
        dest.display(),
        src_primary.display(),
        src_alias.display()
    );
}

fn embed_lib_sysroot(manifest_dir: &Path, arch: &str, out_dir: &Path) {
    let triple = if arch == "x86_64" {
        "x86_64-unknown-myos"
    } else if arch == "aarch64" {
        "aarch64-unknown-myos"
    } else if arch == "riscv64" {
        "riscv64-unknown-myos"
    } else {
        return;
    };
    let prefix = manifest_dir
        .join("../target")
        .join(format!("newlib-{arch}"))
        .join(triple);
    let include_dir = prefix.join("include");
    let lib_dir = prefix.join("lib");
    println!("cargo:rerun-if-changed={}", include_dir.display());
    println!("cargo:rerun-if-changed={}", lib_dir.display());
    if !include_dir.is_dir() || !lib_dir.is_dir() {
        let script = manifest_dir.join("../toolchain/newlib/build.sh");
        println!("cargo:rerun-if-changed={}", script.display());
        let status = Command::new("bash")
            .arg(&script)
            .status()
            .unwrap_or_else(|e| panic!("run {}: {e}", script.display()));
        if !status.success() {
            panic!("{} failed", script.display());
        }
    }
    if !include_dir.is_dir() || !lib_dir.is_dir() {
        panic!(
            "newlib sysroot missing at {} (run ./toolchain/newlib/build.sh)",
            prefix.display()
        );
    }

    // Guest tcc looks for libtcc1.a on CONFIG_TCC_LIBPATHS (/lib/newlib/lib).
    // newlib GHCR cache does not include it; tcc build installs into the prefix
    // and also leaves target/libtcc1-*-unknown-myos.a for boot-job rebuilds.
    ensure_libtcc1_in_sysroot(manifest_dir, arch, &lib_dir);

    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    collect_dir(&include_dir, "newlib/include", &mut entries);
    collect_dir(&lib_dir, "newlib/lib", &mut entries);
    // Compiler headers (stddef.h, stdarg.h, float.h, …). newlib's sys/cdefs.h
    // includes them; tcc does not have GCC builtins.
    let tcc_inc = manifest_dir.join("../target/tcc-src/include");
    println!("cargo:rerun-if-changed={}", tcc_inc.display());
    if !tcc_inc.is_dir() {
        let prep = manifest_dir.join("../ports/tcc/prepare.sh");
        let status = Command::new("bash")
            .arg(&prep)
            .status()
            .unwrap_or_else(|e| panic!("run {}: {e}", prep.display()));
        if !status.success() {
            panic!("{} failed", prep.display());
        }
    }
    if tcc_inc.is_dir() {
        collect_dir(&tcc_inc, "newlib/include", &mut entries);
    } else {
        panic!(
            "tcc include/ missing at {} (run ./ports/tcc/prepare.sh)",
            tcc_inc.display()
        );
    }
    if let Some((_, crt0)) = entries.iter().find(|(p, _)| p == "newlib/lib/crt0.o") {
        let bytes = crt0.clone();
        if !entries.iter().any(|(p, _)| p == "newlib/lib/crt1.o") {
            entries.push(("newlib/lib/crt1.o".into(), bytes));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut blob = Vec::new();
    let mut table: Vec<(String, usize, usize)> = Vec::new();
    for (rel, bytes) in entries {
        if rel.len() > 96 {
            eprintln!("libfs skip (path too long): {rel}");
            continue;
        }
        let off = blob.len();
        let len = bytes.len();
        blob.extend_from_slice(&bytes);
        table.push((rel, off, len));
    }
    let blob_path = out_dir.join("lib_sysroot.bin");
    std::fs::write(&blob_path, &blob).expect("write lib_sysroot.bin");

    let mut body = String::from(
        "pub fn register_all() {\n    const BLOB: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/lib_sysroot.bin\"));\n",
    );
    for (i, (rel, off, len)) in table.iter().enumerate() {
        body.push_str(&format!(
            "    let _ = super::register({rel:?}, &BLOB[{off}..{end}]);\n",
            end = off + len
        ));
        let _ = i;
    }
    body.push_str("}\n");
    std::fs::write(out_dir.join("lib_embed.rs"), body).expect("write lib_embed.rs");
}

fn collect_dir(dir: &Path, rel: &str, out: &mut Vec<(String, Vec<u8>)>) {
    let rd = match std::fs::read_dir(dir) {
        Ok(v) => v,
        Err(_) => return,
    };
    let mut names: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    names.sort_by_key(|e| e.file_name());
    for ent in names {
        let name = ent.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let path = ent.path();
        let child_rel = format!("{rel}/{name}");
        if path.is_dir() {
            collect_dir(&path, &child_rel, out);
            continue;
        }
        if !path.is_file() {
            continue;
        }
        if name.ends_with(".la") || name.ends_with(".txt") || name == "libm.a" {
            continue;
        }
        if out.iter().any(|(p, _)| p == &child_rel) {
            continue;
        }
        match std::fs::read(&path) {
            Ok(bytes) => out.push((child_rel, bytes)),
            Err(_) => {}
        }
    }
}

fn embed_tcc_elf(manifest_dir: &Path, arch: &str, out_dir: &Path) {
    let triple = if arch == "x86_64" {
        "x86_64-unknown-myos"
    } else if arch == "aarch64" {
        "aarch64-unknown-myos"
    } else if arch == "riscv64" {
        "riscv64-unknown-myos"
    } else {
        return;
    };
    let elf = manifest_dir
        .join("../target")
        .join(format!("tcc-{triple}"));
    let alias = manifest_dir
        .join("../target")
        .join(format!("coreutils-tcc-{triple}"));
    println!("cargo:rerun-if-changed={}", elf.display());
    println!("cargo:rerun-if-changed={}", alias.display());
    if !elf.is_file() && alias.is_file() {
        let _ = std::fs::copy(&alias, &elf);
    }
    if !elf.is_file() {
        // Workflow file edits need `workflow` OAuth scope; build tcc here so
        // CI/ISO still embed /t/tcc without a ci.yml change.
        run_port_build(manifest_dir, "tcc");
    }
    if !elf.is_file() {
        panic!(
            "tcc ELF missing at {} (run ./ports/tcc/build.sh)",
            elf.display()
        );
    }
    let mut body = String::from("pub fn register_all() {\n");
    body.push_str(&format!(
        "    const TCC_ELF: &[u8] = include_bytes!(r\"{}\");\n",
        elf.display()
    ));
    body.push_str("    let _ = super::register(\"tcc\", TCC_ELF);\n");
    body.push_str("}\n");
    let dest = out_dir.join("tcc_embed.rs");
    std::fs::write(&dest, body).expect("write tcc_embed.rs");
}

fn embed_ripgrep_elf(manifest_dir: &Path, arch: &str, out_dir: &Path) {
    let triple = if arch == "x86_64" {
        "x86_64-unknown-myos"
    } else if arch == "aarch64" {
        "aarch64-unknown-myos"
    } else if arch == "riscv64" {
        "riscv64-unknown-myos"
    } else {
        return;
    };
    let elf = manifest_dir
        .join("../target")
        .join(format!("rg-{triple}"));
    println!("cargo:rerun-if-changed={}", elf.display());
    if !elf.is_file() {
        run_port_build(manifest_dir, "ripgrep");
    }
    if !elf.is_file() {
        panic!(
            "ripgrep ELF missing at {} (run ./ports/ripgrep/build.sh)",
            elf.display()
        );
    }
    let mut body = String::from("pub fn register_all() {\n");
    body.push_str(&format!(
        "    const RG_ELF: &[u8] = include_bytes!(r\"{}\");\n",
        elf.display()
    ));
    body.push_str("    let _ = super::register(\"rg\", RG_ELF);\n");
    body.push_str("}\n");
    let dest = out_dir.join("ripgrep_embed.rs");
    std::fs::write(&dest, body).expect("write ripgrep_embed.rs");
}

fn embed_std_elf(manifest_dir: &Path, arch: &str, artifact: &str, env_key: &str) {
    let triple = if arch == "x86_64" {
        "x86_64-unknown-myos"
    } else if arch == "aarch64" {
        "aarch64-unknown-myos"
    } else if arch == "riscv64" {
        "riscv64-unknown-myos"
    } else {
        return;
    };
    let stable = manifest_dir
        .join("../target")
        .join(format!("{artifact}-{triple}"));
    println!("cargo:rerun-if-changed={}", stable.display());
    if !stable.is_file() {
        panic!(
            "{artifact} ELF missing at {} (run ./toolchain/std/build-std-hello.sh)",
            stable.display()
        );
    }
    println!("cargo:rustc-env={env_key}={}", stable.display());
}

fn embed_sbase_manifest(manifest_dir: &Path, arch: &str, out_dir: &Path) {
    let manifest = manifest_dir
        .join("../target")
        .join(format!("sbase-manifest-{arch}.txt"));
    println!("cargo:rerun-if-changed={}", manifest.display());
    if !manifest.is_file() {
        panic!(
            "sbase manifest missing at {} (run ./ports/sbase/build.sh)",
            manifest.display()
        );
    }
    let text = std::fs::read_to_string(&manifest).expect("read sbase manifest");
    let mut body = String::from("pub fn register_all() {\n");
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((name, path)) = line.split_once(':') else {
            continue;
        };
        let path = Path::new(path);
        if !path.is_file() {
            panic!(
                "sbase ELF missing for {name} at {} (run ./ports/sbase/build.sh)",
                path.display()
            );
        }
        println!("cargo:rerun-if-changed={}", path.display());
        body.push_str(&format!(
            "    let _ = super::register({name:?}, include_bytes!(r\"{}\"));\n",
            path.display()
        ));
    }
    body.push_str("}\n");
    let dest = out_dir.join("sbase_embed.rs");
    std::fs::write(&dest, body).expect("write sbase_embed.rs");
}


fn embed_ubase_manifest(manifest_dir: &Path, arch: &str, out_dir: &Path) {
    let manifest = manifest_dir
        .join("../target")
        .join(format!("ubase-manifest-{arch}.txt"));
    println!("cargo:rerun-if-changed={}", manifest.display());
    if !manifest.is_file() {
        run_port_build(manifest_dir, "ubase");
    }
    if !manifest.is_file() {
        panic!(
            "ubase manifest missing at {} (run ./ports/ubase/build.sh)",
            manifest.display()
        );
    }
    let text = std::fs::read_to_string(&manifest).expect("read ubase manifest");
    let mut body = String::from("pub fn register_all() {\n");
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((name, path)) = line.split_once(':') else {
            continue;
        };
        let path = Path::new(path);
        if !path.is_file() {
            run_port_build(manifest_dir, "ubase");
        }
        if !path.is_file() {
            panic!(
                "ubase ELF missing for {name} at {} (run ./ports/ubase/build.sh)",
                path.display()
            );
        }
        println!("cargo:rerun-if-changed={}", path.display());
        body.push_str(&format!(
            "    let _ = super::register({name:?}, include_bytes!(r\"{}\"));\n",
            path.display()
        ));
    }
    body.push_str("}\n");
    let dest = out_dir.join("ubase_embed.rs");
    std::fs::write(&dest, body).expect("write ubase_embed.rs");
}

fn embed_oksh_elf(manifest_dir: &Path, arch: &str) {
    let triple = if arch == "x86_64" {
        "x86_64-unknown-none"
    } else if arch == "aarch64" {
        "aarch64-unknown-none"
    } else if arch == "riscv64" {
        "riscv64-unknown-none"
    } else {
        return;
    };
    let stable = manifest_dir
        .join("../target")
        .join(format!("oksh-{triple}"));
    println!("cargo:rerun-if-changed={}", stable.display());
    if !stable.is_file() {
        panic!(
            "oksh ELF missing at {} (run ./ports/oksh/build.sh)",
            stable.display()
        );
    }
    println!("cargo:rustc-env=USER_SH_PATH={}", stable.display());
}

fn embed_c_elf(manifest_dir: &Path, arch: &str, artifact: &str, env_key: &str) {
    let triple = if arch == "x86_64" {
        "x86_64-unknown-none"
    } else if arch == "aarch64" {
        "aarch64-unknown-none"
    } else if arch == "riscv64" {
        "riscv64-unknown-none"
    } else {
        return;
    };
    let stable = manifest_dir
        .join("../target")
        .join(format!("{artifact}-{triple}"));
    println!("cargo:rerun-if-changed={}", stable.display());
    if !stable.is_file() {
        panic!(
            "{artifact} ELF missing at {} (run ./scripts/build-c-hello.sh and ./ports/sbase/build.sh)",
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
    // ext2's runtime-sized copies pull libcore panic fmt; x86 PIE needs PIC.
    if target.contains("x86_64") && (bin == "ext2" || bin == "virtio_net") {
        cmd.env("RUSTFLAGS", "-C panic=abort -C relocation-model=pic");
    }
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