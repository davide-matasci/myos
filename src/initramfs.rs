// Build a newc (cpio) archive of the userspace ELFs for a target arch.
//
// The kernel no longer embeds the big port trees (sbase, coreutils, ripgrep,
// tcc, newlib sysroot). Instead they are packed into a newc archive that
// Limine loads as a module (`boot():/boot/initramfs`); the kernel parses it at
// boot and registers each entry into the matching `/bin/<category>/…` or
// `/lib/…` mount. This decouples userspace from the kernel ELF so the kernel
// shrinks and userspace can be swapped without a kernel rebuild.
//
// The archive layout mirrors the VFS tree exactly: `bin/sbase/cat`,
// `bin/coreutils/echo`, `lib/newlib/include/…`, and so on.

use std::path::Path;
use std::process::Command;

/// Per-arch triples for the three flavors of user ELF in `target/`:
/// `(kernel_triple, none_triple, myos_triple)`.
fn triples(arch: &str) -> (&'static str, &'static str, &'static str) {
    match arch {
        "x86_64" => (
            "x86_64-unknown-none",
            "x86_64-unknown-none",
            "x86_64-unknown-myos",
        ),
        "aarch64" => (
            "aarch64-unknown-none-softfloat",
            "aarch64-unknown-none",
            "aarch64-unknown-myos",
        ),
        "riscv64" => (
            "riscv64imac-unknown-none-elf",
            "riscv64-unknown-none",
            "riscv64-unknown-myos",
        ),
        other => panic!("initramfs: unsupported arch {other}"),
    }
}

/// One archive entry. `ino`/`nlink` are used for hardlinked multicall aliases
/// so the shared ELF is stored once in the archive.

/// Active Cargo features. Two contexts call into this file:
///
/// - The build script (`build.rs`, which `include!`s this file) runs while
///   Cargo has `CARGO_CFG_FEATURE` set (comma-joined, including `default` and
///   any expanded members). `MYOS_FEATURES` is not baked into the build script.
/// - The host `myos` binary packs the initramfs at runtime (e.g. the prebuilt
///   CI boot jobs), where `CARGO_CFG_FEATURE` is NOT set — only the
///   compile-time `MYOS_FEATURES` that `build.rs` prints as `cargo:rustc-env=`
///   survives. Reading `std::env::var("CARGO_CFG_FEATURE")` there returns empty
///   and every gate silently turns off (see the 162-vs-311 initramfs gap).
///
/// So prefer the compile-time baked `MYOS_FEATURES` (present in the runtime
/// binary), and fall back to `CARGO_CFG_FEATURE` for the build-script context.
pub fn active_features() -> Vec<String> {
    let raw = option_env!("MYOS_FEATURES")
        .map(str::to_string)
        .or_else(|| std::env::var("CARGO_CFG_FEATURE").ok())
        .unwrap_or_default();
    raw.split(',')
        .map(|p| p.trim().to_string())
        .filter_map(|p| if p.is_empty() { None } else { Some(p) })
        .collect()
}

/// True when the given feature (`linux_compat`) is in the active set.
pub fn feature_enabled(feature: &str) -> bool {
    active_features().iter().any(|f| *f == feature)
}
pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) data: Vec<u8>,
    pub(crate) ino: u64,
    pub(crate) nlink: u32,
    // Full st_mode (S_IFREG | perm). Defaults to 0644 for callers that don't
    // set it; prebuilt smoke ELFs must be 0755 or the guest shell refuses to
    // exec them ("Permission denied") and every smoke test falls back to a
    // minutes-long guest tcc compile.
    pub(crate) mode: u32,
}

/// A file the image needs: a missing one is a build error, never a silently
/// smaller image (the guest then fails with "not found" much later, or an
/// ISO ships without curl). `build.rs` builds every port of the image from
/// its descriptor, so this names what to run when it did not.
fn read(path: &Path) -> Option<Vec<u8>> {
    read_any(&[path])
}

/// Like `read`, but accepts the first of several paths (canonical name or
/// the `coreutils-*` pack alias of ci-build.tar).
fn read_any(paths: &[&Path]) -> Option<Vec<u8>> {
    match read_optional(paths) {
        Some(v) => Some(v),
        None => {
            let tried: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
            panic!(
                "initramfs: required file missing: {} (run `cargo build`, which builds the enabled \
                 ports and programs; or the port's build.sh under ports/, see README)",
                tried.join(" or ")
            );
        }
    }
}

/// The first readable of `paths`, or `None` with a note per miss: only for
/// files a build may legitimately lack.
///
/// Under the build script, every path tried becomes a `rerun-if-changed`
/// input, so a port rebuilt (or removed) under `target/` re-packs the images
/// on the next `cargo build` without any source change.
fn read_optional(paths: &[&Path]) -> Option<Vec<u8>> {
    let in_build_script = std::env::var_os("OUT_DIR").is_some()
        && std::env::var_os("CARGO_CFG_FEATURE").is_some();
    let mut errors: Vec<(String, String)> = Vec::new();
    for path in paths {
        if in_build_script {
            println!("cargo:rerun-if-changed={}", path.display());
        }
        match std::fs::read(path) {
            Ok(v) => return Some(v),
            Err(e) => errors.push((path.display().to_string(), e.to_string())),
        }
    }
    for (path, e) in errors {
        eprintln!("initramfs: skip {path} ({e})");
    }
    None
}


fn add(entries: &mut Vec<Entry>, rel: &str, data: Option<Vec<u8>>) {
    if let Some(d) = data {
        entries.push(Entry {
            name: rel.to_string(),
            data: d,
            ino: entries.len() as u64 + 1,
            nlink: 1,
            mode: 0o100755,
        });
    }
}

/// A generated file with its mode.
fn add_mode(entries: &mut Vec<Entry>, rel: &str, data: Vec<u8>, mode: u32) {
    entries.push(Entry { name: rel.to_string(), data, ino: entries.len() as u64 + 1, nlink: 1, mode });
}

/// Add a hardlink group: every `name` shares `data` under one inode (nlink =
/// count). Only the first name carries the bytes; the rest are zero-length
/// links, so the archive stores the ELF once instead of once per alias.
fn add_hardlink_group(entries: &mut Vec<Entry>, names: &[String], data: Option<Vec<u8>>) {
    let Some(data) = data else { return };
    let ino = entries.len() as u64 + 1;
    let nlink = names.len().max(1) as u32;
    for (i, name) in names.iter().enumerate() {
        entries.push(Entry {
            name: name.clone(),
            data: if i == 0 { data.clone() } else { Vec::new() },
            ino,
            nlink,
            mode: 0o100755,
        });
    }
}


/// Pack what one port of the image ships (`PORT_FILES`, see docs/ports.md).
/// A user program the kernel embeds (`PORT_EMBED`) is optional here: the CI
/// boot jobs pack the aarch64/riscv64 initramfs from ci-build.tar, which
/// carries only what the kernel does not embed.
pub(crate) fn install_port(entries: &mut Vec<Entry>, port: &crate::ports::Port, manifest_dir: &Path, arch: &str) {
    use crate::ports::{FileSpec, expand};
    let target = manifest_dir.join("target");
    let optional = port.embed.is_some();
    let take = |path: &Path| -> Option<Vec<u8>> {
        if optional { read_optional(&[path]) } else { read(path) }
    };
    for f in &port.files {
        match f {
            FileSpec::Bin { src, paths } => {
                let data = take(&target.join(expand(src, arch)));
                if paths.len() == 1 {
                    add(entries, &paths[0], data);
                } else {
                    add_hardlink_group(entries, paths, data);
                }
            }
            FileSpec::Data { src, path } => {
                add(entries, path, take(&target.join(expand(src, arch))));
            }
            FileSpec::File { src, path } => {
                add(entries, path, read(&manifest_dir.join(&port.dir).join(src)));
            }
            FileSpec::Manifest { src, dir } => {
                // `name:/path/to/elf` per line.
                let Some(text) = read(&target.join(expand(src, arch))) else {
                    continue;
                };
                for line in String::from_utf8_lossy(&text).lines() {
                    let line = line.trim();
                    if let Some((name, path)) = line.split_once(':') {
                        add(entries, &format!("{dir}/{name}"), read(Path::new(path)));
                    }
                }
            }
            FileSpec::Multicall { elf, manifest, dir } => {
                // One ELF stored once, aliased under every name of the manifest.
                let Some(text) = read(&target.join(expand(manifest, arch))) else {
                    continue;
                };
                let names: Vec<String> = String::from_utf8_lossy(&text)
                    .lines()
                    .map(str::trim)
                    .filter(|n| !n.is_empty() && !n.starts_with('#'))
                    .map(|n| format!("{dir}/{n}"))
                    .collect();
                add_hardlink_group(entries, &names, read(&target.join(expand(elf, arch))));
            }
            FileSpec::Tree { src, dir } => {
                let root = target.join(expand(src, arch));
                if !root.is_dir() {
                    panic!(
                        "initramfs: required directory missing: {} (port {}; run its build script)",
                        root.display(),
                        port.name
                    );
                }
                collect_tree(&root, dir, entries);
            }
        }
    }
}

/// Recursively collect a directory tree into `entries` under `rel/…`,
/// mirroring `kernel/build.rs::collect_dir` (skip dotfiles, `.la`, `.txt`).
fn collect_tree(dir: &Path, rel: &str, entries: &mut Vec<Entry>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        eprintln!("initramfs: no sysroot tree at {}", dir.display());
        return;
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
            collect_tree(&path, &child_rel, entries);
            continue;
        }
        if !path.is_file() {
            continue;
        }
        if name.ends_with(".la") || name.ends_with(".txt") {
            continue;
        }
        if entries.iter().any(|e| e.name == child_rel) {
            continue;
        }
        if let Some(bytes) = read(&path) {
            // Preserve the host exec bit: /lib/os-test/prebuilt ELFs must be
            // 0755 in the guest or the shell refuses to exec them.
            let exec = {
                use std::os::unix::fs::PermissionsExt;
                std::fs::metadata(&path)
                    .map(|m| m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
            };
            entries.push(Entry {
                name: child_rel,
                data: bytes,
                ino: entries.len() as u64 + 1,
                nlink: 1,
                mode: if exec { 0o100755 } else { 0o100644 },
            });
        }
    }
}

/// Build the newc initramfs archive for `arch` from the ELFs under `target/`.
/// Every file a port of the image ships must exist (`read` panics
/// otherwise): `build.rs` runs the missing ports' build scripts before this.
pub fn build_initramfs(manifest_dir: &Path, arch: &str) -> Vec<u8> {
    let target = manifest_dir.join("target");
    let (kernel_triple, _none_triple, _myos_triple) = triples(arch);
    let mut entries: Vec<Entry> = Vec::new();

    // Everything the ports ship: one descriptor per port (`port.env`, see
    // docs/ports.md), the same files `get-myos` would install from a package.
    let all_ports = crate::ports::load_all(manifest_dir);
    let image_ports: Vec<&crate::ports::Port> =
        all_ports.iter().filter(|p| p.role == crate::ports::Role::Image).collect();
    for port in &image_ports {
        install_port(&mut entries, port, manifest_dir, arch);
    }

    // /usr skeleton for the os-test paths suite (/usr, /usr/bin, /usr/bin/env,
    // /usr/lib): access(F_OK) only, so a placeholder is enough; the directory
    // prefix logic of the filesystems exposes the parents as directories.
    add(
        &mut entries,
        "usr/bin/env",
        Some(b"#!/bin/sh\nexec \"$@\"\n".to_vec()),
    );
    add(&mut entries, "usr/lib/.keep", Some(b"\n".to_vec()));
    // /mnt: a directory to mount a disk on (mount(2) wants an existing one).
    add(&mut entries, "mnt/.keep", Some(b"\n".to_vec()));
    // The security policy (docs/security.md); the kernel carries the same
    // file as its fallback.
    add(&mut entries, "etc/policy", read(&manifest_dir.join("etc/policy")));

    // The `linux` launcher of the Linux compatibility layer (native, always
    // shipped: with the module loaded, `linux PROGRAM` works in any build).
    add(
        &mut entries,
        "bin/etc/linux",
        read(&target.join(format!("linux-launcher-{arch}-unknown-none"))),
    );
    // Optional Linux compatibility layer: the Linux test binaries, kept out
    // of $PATH under bin/linux, and the Alpine package fetcher.
    if feature_enabled("linux_compat") {
        add(
            &mut entries,
            "bin/linux/linux-smoke",
            read(&target.join(format!("linux-smoke-{arch}-linux-musl"))),
        );
        // A dynamically linked test: musl's libc.so as the dynamic linker
        // (its PT_INTERP path) and two shared objects in the default path.
        let dyn_dir = target.join("linux-compat").join(arch);
        add(
            &mut entries,
            &format!("lib/ld-musl-{arch}.so.1"),
            read(&dyn_dir.join(format!("ld-musl-{arch}.so.1"))),
        );
        for lib in ["libsmoke.so", "libsmoke2.so"] {
            add(&mut entries, &format!("lib/{lib}"), read(&dyn_dir.join(lib)));
        }
        add(&mut entries, "bin/linux/linux-dyn", read(&dyn_dir.join("linux-dyn")));
        // Alpine Linux package fetcher; packages are downloaded at run time.
        add(&mut entries, "bin/etc/get-alpine", read(&dyn_dir.join("get-alpine")));
        // Building myos inside myos with Alpine's Rust (`sh /lib/self-host.sh DIR`).
        add(&mut entries, "lib/self-host.sh", read(&manifest_dir.join("linux-compat/self-host.sh")));
    }

    // Kernel modules -> lib/modules/<name> (the same ELFs Limine loads at
    // boot; `insmod /lib/modules/<name>` loads one that was not, e.g. the
    // optional `linux` module in a build without its feature).
    for m in crate::limine_image::all_modules() {
        add(
            &mut entries,
            &format!("lib/modules/{m}"),
            read(&target.join(format!("{m}-{kernel_triple}"))),
        );
    }

    // Compiler headers (stddef.h, stdarg.h, float.h, …) come from the tcc
    // source tree: newlib's sys/cdefs.h includes them (the newlib sysroot is
    // under lib/newlib, from the `newlib` port), but tcc has no GCC builtins,
    // so they must exist in the archive. Only when tcc is in the image;
    // without tcc nothing compiles on the guest so they are unneeded.
    if image_ports.iter().any(|p| p.name == "tcc") {
    let tcc_inc = target.join("tcc-src/include");
    let stddef = tcc_inc.join("stddef.h");
    if !stddef.is_file() {
        let prep = manifest_dir.join("ports/tcc/prepare.sh");
        let status = Command::new("bash")
            .arg(&prep)
            .status()
            .unwrap_or_else(|e| panic!("run {}: {e}", prep.display()));
        if !status.success() {
            panic!("{} failed", prep.display());
        }
    }
    if !stddef.is_file() {
        panic!(
            "tcc include/stddef.h missing at {} (run ./ports/tcc/prepare.sh)",
            stddef.display()
        );
    }
    collect_tree(&tcc_inc, "lib/newlib/include", &mut entries);
    // Hosted tcc wants crt1.o; newlib only ships crt0.o. Mirror build.rs.
    let crt0 = entries
        .iter()
        .find(|e| e.name == "lib/newlib/lib/crt0.o")
        .map(|e| e.data.clone());
    if let Some(crt0) = crt0 {
        if !entries.iter().any(|e| e.name == "lib/newlib/lib/crt1.o") {
            entries.push(Entry {
                name: "lib/newlib/lib/crt1.o".to_string(),
                data: crt0,
                ino: entries.len() as u64 + 1,
                nlink: 1,
                mode: 0o100644,
            });
        }
    }
    }

    // The build's release (src/release.rs): what get-myos compares a
    // mirror's index with before installing from it.
    add_mode(
        &mut entries,
        "lib/myos-release",
        crate::release::Release::current(manifest_dir).text().into_bytes(),
        0o100644,
    );

    // Loadable keyboard maps (Swiss German default; US alternate).
    // Served at /lib/kbd/*.map via libfs (cpio lib/ → libfs nested tree).
    // Do NOT pack under etc/ — bootfs is flat (MAX_FILES=32) and register
    // failures are ignored, so /etc/kbd/*.map never appears on the guest.
    add(
        &mut entries,
        "lib/kbd/ch.map",
        read(&manifest_dir.join("modules/console/keymaps/ch.map")),
    );
    add(
        &mut entries,
        "lib/kbd/us.map",
        read(&manifest_dir.join("modules/console/keymaps/us.map")),
    );

    // Sort + dedupe by path (later duplicates win for the same path).
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries.dedup_by(|a, b| a.name == b.name);
    write_newc(&entries)
}

/// Serialize entries as a newc archive with a `TRAILER!!!` terminator.
fn write_newc(entries: &[Entry]) -> Vec<u8> {
    let mut out = Vec::new();
    for e in entries {
        write_entry(&mut out, e);
    }
    write_entry(&mut out, &Entry {
        name: "TRAILER!!!".to_string(),
        data: Vec::new(),
        ino: 0,
        nlink: 1,
        mode: 0o100644,
    });
    out
}

fn write_entry(out: &mut Vec<u8>, e: &Entry) {
    let namesize = e.name.len() + 1;
    let mut hdr = String::from("070701");
    for field in [
        e.ino,        // ino
        e.mode as u64, // mode (S_IFREG | perm, per entry)
        0,            // uid
        0,            // gid
        e.nlink as u64, // nlink
        0,            // mtime
        e.data.len() as u64, // filesize
        0,            // devmajor
        0,            // devminor
        0,            // rdevmajor
        0,            // rdevminor
        namesize as u64, // namesize
        0,            // check
    ] {
        hdr.push_str(&format!("{field:08x}"));
    }
    out.extend_from_slice(hdr.as_bytes());
    out.extend_from_slice(e.name.as_bytes());
    out.push(0);
    pad4(out);
    out.extend_from_slice(&e.data);
    pad4(out);
}

fn pad4(out: &mut Vec<u8>) {
    while out.len() % 4 != 0 {
        out.push(0);
    }
}