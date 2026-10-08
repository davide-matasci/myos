use std::env;
use std::path::{Path, PathBuf};

#[allow(dead_code)]
mod ports {
    include!("../src/ports.rs");
}
use std::process::Command;

/// Kernel modules, `(directory under modules/, bin name)`, in build order.
const MODULES: &[(&str, &str)] = &[
    ("console", "console"),
    ("hello", "hello"),
    ("pci_enum", "pci_enum"),
    ("acpi", "acpi"),
    ("virtio_blk", "virtio_blk"),
    ("nvme", "nvme"),
    ("xhci", "xhci"),
    ("usb_hub", "usb_hub"),
    ("usb_storage", "usb_storage"),
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
    // initramfs (`/lib/modules`: the kernel loads the boot ones from there,
    // `insmod` the others). The kernel embeds none of them.
    for &(dir, bin) in MODULES {
        let _ = nested_elf(
            &cargo,
            manifest,
            &format!("../modules/{dir}"),
            bin,
            &format!("{bin}-target"),
            &target,
            &profile,
            &out,
            // `.`: crates inside the module's directory too (ext2fs, ps2-scancode).
            &[".", "../abi/src/lib.rs", "../virtq/src/lib.rs"],
        );
    }
    // The Rust userspace programs: every `user/<name>/port.env` (and a
    // `packages/<name>/port.env` of kind `user`), init among them, built for
    // this arch into `target/<bin>-<triple>`, where the initramfs packer
    // takes them. The kernel embeds none of them.
    let repo = manifest.join("..");
    println!("cargo:rerun-if-changed={}", repo.join("src/ports.rs").display());
    for port in ports::load_all(&repo) {
        if port.kind != ports::Kind::User {
            continue;
        }
        println!("cargo:rerun-if-changed={}", repo.join(&port.dir).join("port.env").display());
        let crate_rel = format!("../{}", port.dir.display());
        if let Some(script) = &port.prepare {
            prepare(&repo, script, &port.bin);
        }
        let watch: Vec<String> = port.watch.clone();
        let watch_refs: Vec<&str> = watch.iter().map(String::as_str).collect();
        nested_elf(
            &cargo,
            manifest,
            &crate_rel,
            &port.bin,
            &format!("{}-target", port.name),
            &target,
            &profile,
            &out,
            &watch_refs,
        );
    }

}

/// Run a user program's `PORT_PREPARE` script (repo-relative), unless the
/// program comes prebuilt (`MYOS_PREBUILT`, see `nested_elf`).
fn prepare(repo: &Path, script: &str, bin: &str) {
    if env::var("MYOS_PREBUILT").is_ok_and(|v| v.split(',').any(|b| b == bin)) {
        return;
    }
    let path = repo.join(script);
    println!("cargo:rerun-if-changed={}", path.display());
    let status = Command::new("bash")
        .arg(&path)
        .status()
        .unwrap_or_else(|e| panic!("failed to run {script} for {bin}: {e}"));
    if !status.success() {
        panic!("{script} failed for {bin}");
    }
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
    target: &str,
    profile: &str,
    out: &str,
    extra_rerun: &[&str],
) {
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

    // A kernel built inside myos (linux-compat/self-host.sh) takes a program
    // it cannot build there from the running system: MYOS_PREBUILT lists
    // them, each already at target/<bin>-<target>.
    println!("cargo:rerun-if-env-changed=MYOS_PREBUILT");
    if env::var("MYOS_PREBUILT").is_ok_and(|v| v.split(',').any(|b| b == bin)) {
        let stable = stable_elf(manifest_dir, bin, target);
        assert!(stable.is_file(), "MYOS_PREBUILT names {bin}, but {} is missing", stable.display());
        println!("cargo:rerun-if-changed={}", stable.display());
        return;
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
                | "xhci" | "usb_hub" | "usb_storage"
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
    let stable = stable_elf(manifest_dir, bin, target);
    std::fs::copy(&elf, &stable)
        .unwrap_or_else(|e| panic!("copy {bin} ELF to {}: {e}", stable.display()));
}

/// `bin`'s ELF for `target` where the image builders take it:
/// `target/<bin>-<target>`.
fn stable_elf(manifest_dir: &Path, bin: &str, target: &str) -> PathBuf {
    let ws_target = manifest_dir.join("../target");
    std::fs::create_dir_all(&ws_target).expect("workspace target dir");
    ws_target.join(format!("{bin}-{target}"))
}

/// Programs linked at USER_BASE (ET_EXEC with absolute vtables; see
/// `PORT_IMAGE_BASE` in the descriptor).
fn user_image_base(bin: &str) -> bool {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    ports::load_all(&repo)
        .iter()
        .any(|p| p.kind == ports::Kind::User && p.bin == bin && p.image_base)
}