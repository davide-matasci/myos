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
struct Entry {
    name: String,
    data: Vec<u8>,
    ino: u64,
    nlink: u32,
}

fn read(path: &Path) -> Option<Vec<u8>> {
    match std::fs::read(path) {
        Ok(v) => Some(v),
        Err(e) => {
            eprintln!("initramfs: skip {} ({e})", path.display());
            None
        }
    }
}

fn add(entries: &mut Vec<Entry>, rel: &str, data: Option<Vec<u8>>) {
    if let Some(d) = data {
        entries.push(Entry {
            name: rel.to_string(),
            data: d,
            ino: entries.len() as u64 + 1,
            nlink: 1,
        });
    }
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
        });
    }
}

/// Recursively collect a directory tree into `entries` under `rel/…`,
/// mirroring `kernel/build.rs::collect_dir` (skip dotfiles, `.la`, `.txt`,
/// `libm.a`).
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
        if name.ends_with(".la") || name.ends_with(".txt") || name == "libm.a" {
            continue;
        }
        if entries.iter().any(|e| e.name == child_rel) {
            continue;
        }
        if let Some(bytes) = read(&path) {
            entries.push(Entry {
                name: child_rel,
                data: bytes,
                ino: entries.len() as u64 + 1,
                nlink: 1,
            });
        }
    }
}

/// Build the newc initramfs archive for `arch` from the ELFs under `target/`.
/// Missing files are skipped (the kernel keeps a small embedded fallback for
/// boot-critical programs), so this is safe to run before every port has built.
pub fn build_initramfs(manifest_dir: &Path, arch: &str) -> Vec<u8> {
    let target = manifest_dir.join("target");
    let (kernel_triple, none_triple, myos_triple) = triples(arch);
    let mut entries: Vec<Entry> = Vec::new();

    // sbase manifest: `name:/path/to/sbase-name-<triple>` -> bin/sbase/<name>.
    if let Some(text) = read(&target.join(format!("sbase-manifest-{arch}.txt"))) {
        let text = String::from_utf8_lossy(&text);
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some((name, path)) = line.split_once(':') {
                add(&mut entries, &format!("bin/sbase/{name}"), read(Path::new(path)));
            }
        }
    }

    // ubase manifest -> bin/ubase/<name>.
    if let Some(text) = read(&target.join(format!("ubase-manifest-{arch}.txt"))) {
        let text = String::from_utf8_lossy(&text);
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some((name, path)) = line.split_once(':') {
                add(&mut entries, &format!("bin/ubase/{name}"), read(Path::new(path)));
            }
        }
    }

    // coreutils: one multicall ELF aliased under every name -> bin/coreutils/<name>.
    // Stored once via a hardlink group.
    let coreutils_elf = read(&target.join(format!("coreutils-{myos_triple}")));
    if let Some(text) = read(&target.join(format!("coreutils-manifest-{arch}.txt"))) {
        let text = String::from_utf8_lossy(&text);
        let mut names: Vec<String> = Vec::new();
        for line in text.lines() {
            let name = line.trim();
            if name.is_empty() || name.starts_with('#') {
                continue;
            }
            names.push(format!("bin/coreutils/{name}"));
        }
        add_hardlink_group(&mut entries, &names, coreutils_elf);
    }

    // ripgrep -> bin/coreutils/rg.
    add(
        &mut entries,
        "bin/coreutils/rg",
        read(&target.join(format!("rg-{myos_triple}"))),
    );

    // tcc -> bin/tcc/tcc.
    add(
        &mut entries,
        "bin/tcc/tcc",
        read(&target.join(format!("tcc-{myos_triple}"))),
    );

    // std programs -> bin/std/<name>.
    for name in ["hello", "cat", "echo", "bigalloc"] {
        add(
            &mut entries,
            &format!("bin/std/{name}"),
            read(&target.join(format!("std-{name}-{myos_triple}"))),
        );
    }

    // c-hello -> bin/etc/hello.
    add(
        &mut entries,
        "bin/etc/hello",
        read(&target.join(format!("c-hello-{none_triple}"))),
    );

    // userspace BSD sockets smoke -> bin/etc/socket_smoke.
    // Fallback: coreutils-* pack alias when ci-build.tar omitted the canonical name
    // (workflow glob is `target/c-hello-*`, not `c-socket_smoke-*`).
    add(
        &mut entries,
        "bin/etc/socket_smoke",
        read(&target.join(format!("c-socket_smoke-{none_triple}"))).or_else(|| {
            read(&target.join(format!("coreutils-c-socket_smoke-{none_triple}")))
        }),
    );

    // trimmed curl (HTTPS GET + -o) over userspace sockets + mbedtls.
    // Canonical guest path is /bin/etc/curl ($PATH includes /bin/etc). Also install
    // /bin/custom/curl next to ping/http/dns (hardlink group = one ELF in the archive).
    // Fallback to coreutils-curl-* pack alias when ci-build.tar omitted the canonical name.
    let curl_elf = read(&target.join(format!("curl-{none_triple}"))).or_else(|| {
        read(&target.join(format!("coreutils-curl-{none_triple}")))
    });
    add_hardlink_group(
        &mut entries,
        &["bin/etc/curl".to_string(), "bin/custom/curl".to_string()],
        curl_elf,
    );

    // hello demo module -> bin/modules/hello.
    add(
        &mut entries,
        "bin/modules/hello",
        read(&target.join(format!("hello-{kernel_triple}"))),
    );

    // Nested user/* ELFs -> bin/custom/<name>.
    for (rel, bin) in [
        ("ok", "ok"),
        ("heap", "heap"),
        ("cat", "myos_cat"),
        ("echo", "myos_echo"),
        ("ls", "myos_ls"),
        ("mount", "mount"),
        ("mkfs.ext2", "mkfs_ext2"),
        ("ping", "ping"),
        ("http", "http"),
        ("dns", "dns"),
        ("netd", "netd"),
    ] {
        add(
            &mut entries,
            &format!("bin/custom/{rel}"),
            read(&target.join(format!("{bin}-{kernel_triple}"))),
        );
    }
    // oksh -> bin/custom/sh (none triple).
    add(
        &mut entries,
        "bin/custom/sh",
        read(&target.join(format!("oksh-{none_triple}"))),
    );
    // vim (FEAT_TINY) -> bin/custom/vim (none triple, like oksh).
    // Fail loud once CI always builds vim (iso.yml / ci.yml); silent skip hid
    // missing ELFs from the ISO for too long.
    {
        let vim_path = target.join(format!("vim-{none_triple}"));
        let vim_bytes = std::fs::read(&vim_path).unwrap_or_else(|e| {
            panic!(
                "initramfs: required bin/custom/vim missing at {} ({e}); run ./ports/vim/build.sh",
                vim_path.display()
            )
        });
        add(&mut entries, "bin/custom/vim", Some(vim_bytes));
    }

    // newlib sysroot -> lib/newlib/include/… and lib/newlib/lib/….
    let sysroot = target.join(format!("newlib-{arch}")).join(myos_triple);
    collect_tree(&sysroot.join("include"), "lib/newlib/include", &mut entries);
    collect_tree(&sysroot.join("lib"), "lib/newlib/lib", &mut entries);
    // Compiler headers (stddef.h, stdarg.h, float.h, …) come from the tcc
    // source tree: newlib's sys/cdefs.h includes them, but tcc has no GCC
    // builtins, so they must exist in the archive. On CI tcc is pulled as a
    // cached GHCR output and target/tcc-src is absent, so prepare it here and
    // fail loudly rather than silently omitting the headers (which would only
    // surface later when hosted tcc compiles a program at boot).
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
            });
        }
    }

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
    });
    out
}

fn write_entry(out: &mut Vec<u8>, e: &Entry) {
    let namesize = e.name.len() + 1;
    let mut hdr = String::from("070701");
    for field in [
        e.ino,        // ino
        0o100644,     // mode (regular file)
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