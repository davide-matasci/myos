use std::env;
use std::path::{Path, PathBuf};

#[allow(dead_code)]
mod ports {
    include!("../src/ports.rs");
}
use std::process::Command;


/// Build a port script when its repo-root-relative artifact is missing.
/// Port build scripts are stamped and early-exit when current, so this is
/// cheap for up-to-date trees. The kernel package has no [features]
/// (CARGO_CFG_FEATURE is empty here); feature gating for the initramfs-only
/// ports lives in the root package build.rs, which has the real feature set.
fn ensure_artifact(manifest: &Path, artifact: &str, script: &str) {
    let root = manifest.parent().expect("repo root");
    let script_path = root.join(script);
    println!("cargo:rerun-if-changed={}", script_path.display());
    println!(
        "cargo:rerun-if-changed={}",
        script_path.parent().expect("script parent dir").display()
    );
    if root.join(artifact).is_file() {
        return;
    }
    eprintln!("==> cargo: {artifact} missing; running {script}");
    // Strip the cargo-injected environment (RUSTC, TARGET, CARGO_CFG_*,
    // LD_LIBRARY_PATH, ...) — it leaks into the nested cargo invocation and
    // breaks target probing ("failed to run `rustc` to learn about
    // target-specific information").
    let status = Command::new("bash")
        .arg(&script_path)
        .current_dir(root)
        .env_clear()
        .env("PATH", env::var("PATH").unwrap_or_default())
        .env("HOME", env::var("HOME").unwrap_or_default())
        .status()
        .unwrap_or_else(|e| panic!("run {script}: {e}"));
    if !status.success() {
        panic!("{script} failed");
    }
}

/// Kernel modules, `(directory under modules/, bin name)`, in build order.
const MODULES: &[(&str, &str)] = &[
    ("console", "console"),
    ("hello", "hello"),
    ("stubfs", "stubfs"),
    ("pci_enum", "pci_enum"),
    ("acpi", "acpi"),
    ("virtio_blk", "virtio_blk"),
    ("nvme", "nvme"),
    ("virtio_net", "virtio_net"),
    ("netfs", "netfs"),
    ("fat", "fat"),
    ("ext2", "ext2"),
    ("linux", "linux"),
];

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

    // Kernel modules: built here so `target/<module>-<triple>` exists for the
    // image builders (Limine loads them from `boot/modules/` at boot) and for
    // the initramfs (`/lib/modules`, `insmod`). The kernel embeds none of them.
    for &(dir, bin) in MODULES {
        let _ = nested_elf(
            &cargo,
            manifest,
            &format!("../modules/{dir}"),
            bin,
            &format!("{bin}-target"),
            "_unused",
            &target,
            &profile,
            &out,
            // `.`: crates inside the module's directory too (ext2fs, ps2-scancode).
            &[".", "../abi/src/lib.rs", "../virtq/src/lib.rs"],
        );
    }
    let _ = nested_elf(
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
    // The Rust userspace programs: every `user/<name>/port.env` (and a
    // `packages/<name>/port.env` of kind `user`), built for this arch; the
    // ones with PORT_EMBED are generated into binfs (`user_embed.rs`).
    let repo = manifest.join("..");
    println!("cargo:rerun-if-changed={}", repo.join("src/ports.rs").display());
    let mut embed = String::from("pub fn register_all() {\n");
    for port in ports::load_all(&repo) {
        if port.kind != ports::Kind::User {
            continue;
        }
        println!("cargo:rerun-if-changed={}", repo.join(&port.dir).join("port.env").display());
        let crate_rel = format!("../{}", port.dir.display());
        let watch: Vec<String> = port.watch.clone();
        let watch_refs: Vec<&str> = watch.iter().map(String::as_str).collect();
        let embedded = nested_elf(
            &cargo,
            manifest,
            &crate_rel,
            &port.bin,
            &format!("{}-target", port.name),
            if port.embed.is_some() { "_embed" } else { "_unused" },
            &target,
            &profile,
            &out,
            &watch_refs,
        );
        if let (Some(path), Some(bytes)) = (&port.embed, embedded) {
            embed.push_str(&format!(
                "    let _ = register({path:?}, include_bytes!(r\"{}\"));\n",
                bytes.display()
            ));
        }
    }
    embed.push_str("}\n");
    std::fs::write(PathBuf::from(&out).join("user_embed.rs"), embed).expect("write user_embed.rs");

    if arch == "x86_64" || arch == "aarch64" || arch == "riscv64" {
        // Auto-build artifacts the kernel embeds unconditionally (binfs.rs
        // uses env!(); the kernel package has no feature gates). These are
        // cheap when present — the scripts early-exit when current.
        ensure_artifact(manifest, &format!("target/std-hello-{arch}-unknown-myos"), "user/std/build.sh");
        ensure_artifact(manifest, &format!("target/c-hello-{arch}-unknown-none"), "scripts/build-c-hello.sh");
        ensure_artifact(manifest, &format!("target/oksh-{arch}-unknown-none"), "ports/oksh/build.sh");
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
        // The big port trees (sbase, coreutils, ripgrep, tcc) and the newlib
        // sysroot are no longer embedded; they ship as a newc initramfs module
        // (`src/initramfs.rs`) that the kernel parses at boot. ubase (getty /
        // login) stays embedded as a boot-critical fallback.
        embed_ubase_manifest(manifest, &arch, Path::new(&out));
    }
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
            "{artifact} ELF missing at {} (run ./user/std/build.sh)",
            stable.display()
        );
    }
    println!("cargo:rustc-env={env_key}={}", stable.display());
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


/// Ping/netd on AArch64/RISC-V are ET_EXEC. netd has absolute smoltcp vtables;
/// ping keeps the same link base. Kernel slides PT_LOAD without abs relocs.
fn assert_elf_linked_at_user_base(elf: &Path, bin: &str, target: &str) {
    const USER_BASE: u64 = 0x4000_0000;
    let bytes = std::fs::read(elf).unwrap_or_else(|e| panic!("read {bin} ELF: {e}"));
    if bytes.len() < 64 || &bytes[0..4] != b"\x7fELF" {
        panic!("{bin} for {target} is not ELF");
    }
    // e_entry at offset 24 (ELF64)
    let entry = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
    if entry < USER_BASE {
        panic!(
            "{bin} for {target} entry {entry:#x} is below USER_BASE {USER_BASE:#x};              --image-base did not apply (absolute vtables would fault after slide)"
        );
    }
    // e_entry alone is not enough (--section-start .text can raise entry while
    // rodata/vtables stay at 0x200000 / 0x10000).
    let phoff = u64::from_le_bytes(bytes[32..40].try_into().unwrap()) as usize;
    let phentsize = u16::from_le_bytes(bytes[54..56].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes(bytes[56..58].try_into().unwrap()) as usize;
    let mut min_vaddr = u64::MAX;
    for i in 0..phnum {
        let p = phoff + i * phentsize;
        if p + 40 > bytes.len() {
            break;
        }
        let p_type = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
        if p_type != 1 {
            continue; // PT_LOAD
        }
        let vaddr = u64::from_le_bytes(bytes[p + 16..p + 24].try_into().unwrap());
        min_vaddr = min_vaddr.min(vaddr);
    }
    if min_vaddr == u64::MAX || min_vaddr < USER_BASE {
        panic!(
            "{bin} for {target} min PT_LOAD p_vaddr {min_vaddr:#x} is below USER_BASE              {USER_BASE:#x}; abs data would still fault after slide (entry was {entry:#x})"
        );
    }
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
) -> Option<PathBuf> {
    let crate_dir = manifest_dir.join(crate_rel);
    // The whole source tree (a directory is watched recursively).
    println!("cargo:rerun-if-changed={}/src", crate_dir.display());
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
    // smoltcp (ping) needs a clean link for --image-base; deps-only wipe can
    // leave a stale ET_EXEC at the default 0x200000 / 0x10000 link base.
    let need_image_base = user_image_base(bin) && (target.contains("aarch64") || target.contains("riscv64"));
    if need_image_base {
        let _ = std::fs::remove_dir_all(&td);
    } else {
        let deps = td.join(target).join(profile_dir).join("deps");
        let _ = std::fs::remove_dir_all(deps);
    }
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
    let mut rustflags = String::from("-C panic=abort");
    let is_module = MODULES.iter().any(|(_, m)| *m == bin);
    // ext2's runtime-sized copies pull libcore panic fmt; x86 PIE needs PIC.
    if target.contains("x86_64")
        && matches!(
            bin,
            "ext2" | "virtio_net" | "netfs" | "pci_enum" | "acpi" | "virtio_blk" | "nvme" | "linux"
        )
    {
        rustflags = String::from("-C panic=abort -C relocation-model=pic");
    }
    if target.contains("aarch64") {
        // Match .cargo/config.toml; RUSTFLAGS replaces target rustflags entirely.
        rustflags = String::from("-C panic=abort -C relocation-model=static");
    }
    if target.contains("riscv64") {
        rustflags = String::from(
            "-C panic=abort -C relocation-model=static -C code-model=medium",
        );
    }
    // Kernel modules are PIEs on every arch, so the loader applies real
    // RELATIVE relocs: a static ET_EXEC slid by the loader only gets its code
    // pointers rebased (`rebase_exec_abs_ptrs`), not pointers into .rodata
    // (`&str` tables), which then fault in the module. The prebuilt libcore
    // is not PIC on aarch64/riscv64; `-z notext` lets lld emit dynamic
    // relocs for its read-only sections (the loader relocates before use).
    if is_module && (target.contains("aarch64") || target.contains("riscv64")) {
        rustflags = String::from(
            "-C panic=abort -C relocation-model=pic -C link-arg=-pie -C link-arg=-z -C link-arg=notext",
        );
    }
    // Belt-and-suspenders with user/ping/build.rs. Prefer split link-arg form
    // (equals form alone previously left CI at 0x200000/0x10000).
    if need_image_base {
        rustflags.push_str(" -C link-arg=--image-base -C link-arg=0x40000000");
    }
    cmd.env("RUSTFLAGS", rustflags);
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
    if need_image_base {
        assert_elf_linked_at_user_base(&elf, bin, target);
    }
    let ws_target = manifest_dir.join("../target");
    std::fs::create_dir_all(&ws_target).expect("workspace target dir");
    let stable = ws_target.join(format!("{bin}-{target}"));
    std::fs::copy(&elf, &stable)
        .unwrap_or_else(|e| panic!("copy {bin} ELF to {}: {e}", stable.display()));
    // Path string alone is not enough: same path with new bytes left bootfs
    // include_bytes! stale (rust-cache reused a fingerprint). Watch the stable
    // copy, and give an embedded program a content-hashed file name.
    println!("cargo:rerun-if-changed={}", stable.display());
    if env_key == "_unused" {
        return None;
    }
    let bytes = std::fs::read(&elf).unwrap_or_default();
    let mut hash = 0xcbf29ce484222325u64;
    for b in &bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    let hashed = PathBuf::from(out).join(format!("{bin}-{hash:016x}.elf"));
    std::fs::write(&hashed, &bytes).unwrap_or_else(|e| panic!("write hashed {bin}: {e}"));
    if env_key != "_embed" {
        println!("cargo:rustc-env={env_key}={}", hashed.display());
    }
    Some(hashed)
}

/// Programs linked at USER_BASE (ET_EXEC with absolute vtables; see
/// `PORT_IMAGE_BASE` in the descriptor).
fn user_image_base(bin: &str) -> bool {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    ports::load_all(&repo)
        .iter()
        .any(|p| p.kind == ports::Kind::User && p.bin == bin && p.image_base)
}