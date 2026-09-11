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

/// True when the given feature (e.g. `port_vim`) is in the active set.
pub fn feature_enabled(feature: &str) -> bool {
    active_features().iter().any(|f| *f == feature)
}
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

/// Like `read`, but tries several paths and only logs skips if all miss.
fn read_any(paths: &[&Path]) -> Option<Vec<u8>> {
    let mut errors: Vec<(String, String)> = Vec::new();
    for path in paths {
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
    if feature_enabled("port_sbase")
        && let Some(text) = read(&target.join(format!("sbase-manifest-{arch}.txt"))) {
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
    if feature_enabled("port_ubase")
        && let Some(text) = read(&target.join(format!("ubase-manifest-{arch}.txt"))) {
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
    if feature_enabled("port_coreutils")
        && let Some(text) = read(&target.join(format!("coreutils-manifest-{arch}.txt"))) {
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
    if feature_enabled("port_ripgrep") {
        add(
            &mut entries,
            "bin/coreutils/rg",
            read(&target.join(format!("rg-{myos_triple}"))),
        );
    }

    // tcc -> bin/tcc/tcc.
    if feature_enabled("port_tcc") {
        add(
            &mut entries,
            "bin/tcc/tcc",
            read(&target.join(format!("tcc-{myos_triple}"))),
        );
    }

    // std programs -> bin/std/<name>.
    if feature_enabled("std") {
        for name in ["hello", "cat", "echo", "bigalloc"] {
            add(
                &mut entries,
                &format!("bin/std/{name}"),
                read(&target.join(format!("std-{name}-{myos_triple}"))),
            );
        }
    }

    // c-hello -> bin/etc/hello.
    if feature_enabled("c_hello") {
        add(
            &mut entries,
            "bin/etc/hello",
            read(&target.join(format!("c-hello-{none_triple}"))),
        );
    }

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

    // Mozilla CA bundle for curl's mbedtls backend (CURL_CA_BUNDLE=/lib/cacert.pem).
    // Same PEM mbedtls/fetch.sh downloads and embeds as myos_ca_bundle_pem for `http`.
    // Fallback: coreutils-cacert.pem pack alias (ci-build.tar glob is target/coreutils-*).
    let cacert_canon = target.join("cacert.pem");
    let cacert_alias = target.join("coreutils-cacert.pem");
    add(
        &mut entries,
        "lib/cacert.pem",
        read_any(&[&cacert_canon, &cacert_alias]),
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
    if feature_enabled("port_oksh") {
        // Also serve the shell at /bin/sh: PATH-independent consumers (GNU
        // make's default SHELL=/bin/sh) need it at the canonical location.
        add_hardlink_group(
            &mut entries,
            &["bin/custom/sh".to_string(), "bin/sh".to_string()],
            read(&target.join(format!("oksh-{none_triple}"))),
        );
    }
    // vim (FEAT_TINY) -> bin/custom/vim (none triple, like oksh).
    // Gated on the port_vim feature: exclude with --no-default-features.
    if feature_enabled("port_vim") {
        let vim_path = target.join(format!("vim-{none_triple}"));
        let vim_bytes = std::fs::read(&vim_path).unwrap_or_else(|e| {
            panic!(
                "initramfs: required bin/custom/vim missing at {} ({e}); run ./ports/vim/build.sh",
                vim_path.display()
            )
        });
        add(&mut entries, "bin/custom/vim", Some(vim_bytes));
    }
    // GNU make -> bin/custom/make (none triple).
    // Gated on the port_make feature.
    if feature_enabled("port_make") {
        let make_path = target.join(format!("make-{none_triple}"));
        let make_bytes = std::fs::read(&make_path).unwrap_or_else(|e| {
            panic!(
                "initramfs: required bin/custom/make missing at {} ({e}); run ./ports/make/build.sh",
                make_path.display()
            )
        });
        add(&mut entries, "bin/custom/make", Some(make_bytes));
    }
    // lynx (text browser) -> bin/custom/lynx (none triple).
    // HTTPS via ports/lynx/tidy_tls.c over mbedtls; sockets via libgloss /net.
    // Gated on the port_lynx feature.
    if feature_enabled("port_lynx") {
        let lynx_path = target.join(format!("lynx-{none_triple}"));
        let lynx_bytes = std::fs::read(&lynx_path).unwrap_or_else(|e| {
            panic!(
                "initramfs: required bin/custom/lynx missing at {} ({e}); run ./ports/lynx/build.sh",
                lynx_path.display()
            )
        });
        add(&mut entries, "bin/custom/lynx", Some(lynx_bytes));
        // System lynx.cfg (LYNX_CFG_FILE=/lib/lynx.cfg). Prefer /lib like
        // cacert/termcap/kbd maps (bootfs /etc exists now, but lynx is built for /lib).
        add(
            &mut entries,
            "lib/lynx.cfg",
            read(&manifest_dir.join("ports/lynx/lynx.cfg")),
        );
    }
    // git (Phase-1 local porcelain) -> bin/custom/git (none triple, like vim).
    // Gated on the port_git feature.
    if feature_enabled("port_git") {
        let git_path = target.join(format!("git-{none_triple}"));
        let git_alias = target.join(format!("coreutils-git-{none_triple}"));
        let git_bytes = std::fs::read(&git_path)
            .or_else(|_| std::fs::read(&git_alias))
            .unwrap_or_else(|e| {
                panic!(
                    "initramfs: required bin/custom/git missing at {} (or {}); run ./ports/git/build.sh ({e})",
                    git_path.display(),
                    git_alias.display()
                )
            });
        // Same ELF at /bin/git so `git` is obvious even if PATH is minimal.
        add_hardlink_group(
            &mut entries,
            &["bin/custom/git".to_string(), "bin/git".to_string()],
            Some(git_bytes),
        );
    }

    // newlib sysroot -> lib/newlib/include/… and lib/newlib/lib/….
    // libc is always required (every port links against it), so it is not gated.
    let sysroot = target.join(format!("newlib-{arch}")).join(myos_triple);
    collect_tree(&sysroot.join("include"), "lib/newlib/include", &mut entries);
    collect_tree(&sysroot.join("lib"), "lib/newlib/lib", &mut entries);
    // Compiler headers (stddef.h, stdarg.h, float.h, …) come from the tcc
    // source tree: newlib's sys/cdefs.h includes them, but tcc has no GCC
    // builtins, so they must exist in the archive. Only stricter when tcc is
    // enabled; without tcc nothing compiles on the guest so they are unneeded.
    if feature_enabled("port_tcc") {
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
    }

    // Minimal termcap (linux/ansi/vt100/dumb) for ncurses tgetent — see
    // ports/termcap/README.md. Served at /lib/termcap; getty sets TERMCAP.
    add(
        &mut entries,
        "lib/termcap",
        read(&manifest_dir.join("ports/termcap/termcap")),
    );

    // os-test (POSIX compliance test suite) -> lib/os-test, run manually on
    // the guest with the GNU make port: cd /lib/os-test && make.
    // Sources are fetched at build time by ports/os-test/fetch.sh (pinned
    // sortix/os-test rev + myos GNU-make harness overlay); nothing vendored.
    // Always embedded.
    {
        let embed = manifest_dir.join("target/os-test-embed");
        if !embed.is_dir() {
            let fetch = manifest_dir.join("ports/os-test/fetch.sh");
            let status = std::process::Command::new(&fetch)
                .current_dir(manifest_dir)
                .status();
            match status {
                Ok(st) if st.success() => {}
                other => {
                    panic!("os-test fetch failed ({other:?}); run {} manually", fetch.display())
                }
            }
        }
        collect_tree(&embed, "lib/os-test", &mut entries);
    }

    // Vim system vimrc (pathdef.c points default_vim_dir at /lib/vim):
    // without it vim starts in Vi-compatible mode, which turns 'esckeys' off
    // (arrow keys dead in insert mode) and empties 'backspace' (BS cannot
    // erase before the insert start) — "arrows/backspace don't work in vim".
    add(
        &mut entries,
        "lib/vim/vimrc",
        read(&manifest_dir.join("ports/vim/vimrc")),
    );

    // Loadable keyboard maps (Swiss German default; US alternate).
    // Served at /lib/kbd/*.map via libfs (cpio lib/ → libfs nested tree).
    // Do NOT pack under etc/ — bootfs is flat (MAX_FILES=32) and register
    // failures are ignored, so /etc/kbd/*.map never appears on the guest.
    add(
        &mut entries,
        "lib/kbd/ch.map",
        read(&manifest_dir.join("kbd/ch.map")),
    );
    add(
        &mut entries,
        "lib/kbd/us.map",
        read(&manifest_dir.join("kbd/us.map")),
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