//! Packages: one gzip tar per port with the files it would have in the
//! image (`bin/custom/vim`, `lib/vim/vimrc`, ...), and an index per
//! architecture, under `target/packages/`:
//!
//! ```text
//! <arch>-index.txt          `# myos release=... commit=... abi=...`, then one line
//!                           per package: name version size sha256 file deps
//! <arch>-packages.txt       the names of the ports the image does not carry (packages/)
//! <arch>-<name>.tar.gz      the package
//! ```
//!
//! `deps` are the package's runtime dependencies (`PORT_RDEPS`), comma
//! separated, `-` for none; the header is the build's release
//! (`src/release.rs`), which `get-myos` compares with the system's
//! `/lib/myos-release`.
//!
//! `get-myos` (user/get-myos) installs them on a running system from a
//! mirror with this layout: the project's rolling GitHub release, or the
//! build's own `target/packages/` that the full boot test serves to the
//! guest over slirp (`serve_mirror`). The entries come from the same
//! descriptor code the initramfs is packed with (`install_port`), so a
//! port moving between `ports/` and `packages/` changes nothing in what
//! it ships. See docs/packages.md.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::initramfs::{Entry, install_port};
use crate::ports;
use crate::release::Release;

/// The mirror's host port. The guest reaches the host's loopback as
/// 10.0.2.2 on QEMU's user network, so no forward is needed (a `guestfwd`
/// chardev would serve one connection only).
pub const MIRROR_PORT: u16 = 8765;

/// Build every package for `arch` into `target/packages/` and return that
/// directory. Every port with files is packaged, whichever its role: the
/// packages of the image's ports are how the mechanism is tested while
/// they are in the image.
pub fn build(manifest_dir: &Path, arch: &str) -> PathBuf {
    let out = manifest_dir.join("target/packages");
    std::fs::create_dir_all(&out).expect("create target/packages");
    let all = ports::load_all(manifest_dir);
    check_rdeps(&all);
    let mut index = format!("# myos {}", Release::current(manifest_dir).text());
    // The ports the image lacks: what `get-myos` is for, and what the full
    // boot test installs (an image port installed over itself would bind
    // its files over the image's).
    let mut packages = String::new();
    for port in &all {
        if port.files.is_empty() {
            continue;
        }
        if port.role == ports::Role::Package {
            packages.push_str(&port.name);
            packages.push('\n');
        }
        ensure_built(manifest_dir, port, arch);
        let mut entries: Vec<Entry> = Vec::new();
        install_port(&mut entries, port, manifest_dir, arch);
        if entries.is_empty() {
            continue;
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let tar_path = out.join(format!("{arch}-{}.tar", port.name));
        std::fs::write(&tar_path, ustar(&entries)).expect("write package tar");
        // -n: no name or timestamp in the gzip header, so the same files
        // give the same bytes (the release is re-uploaded only when they
        // change).
        let status = Command::new("gzip")
            .args(["-n", "-f", "-6"])
            .arg(&tar_path)
            .status()
            .expect("run gzip");
        assert!(status.success(), "gzip {} failed", tar_path.display());
        let file = format!("{arch}-{}.tar.gz", port.name);
        let gz = out.join(&file);
        let size = std::fs::metadata(&gz).expect("package size").len();
        // The version is the port's input hash (its stamp); a user program
        // has no stamp, its tarball's own hash stands in.
        let sha = sha256_file(&gz);
        let version = std::fs::read_to_string(manifest_dir.join("target").join(&port.stamp))
            .map(|s| s.trim().chars().take(12).collect::<String>())
            .unwrap_or_else(|_| sha.chars().take(12).collect());
        let deps = if port.rdeps.is_empty() { "-".to_string() } else { port.rdeps.join(",") };
        index.push_str(&format!("{} {version} {size} {sha} {file} {deps}\n", port.name));
    }
    std::fs::write(out.join(format!("{arch}-index.txt")), index).expect("write package index");
    write_boot(manifest_dir, &out, arch);
    std::fs::write(out.join(format!("{arch}-packages.txt")), packages).expect("write package list");
    out
}

/// The release's boot files for `arch`, what `get-myos --upgrade` writes
/// into a boot slot (`docs/install.md`): `<arch>-kernel` (as the boot disk
/// has it) and `<arch>-initramfs` of a default build (no Linux layer,
/// whatever this one has), listed with their sizes and SHA-256 in
/// `<arch>-boot.txt` under the index's header.
fn write_boot(manifest_dir: &Path, out: &Path, arch: &str) {
    let target = manifest_dir.join("target");
    let kernel = match arch {
        "x86_64" => std::fs::read(target.join("boot-kernel-x86_64")).expect("read target/boot-kernel-x86_64 (cargo build)"),
        _ => {
            let (triple, _, _) = crate::initramfs::triples(arch);
            let elf = target.join(triple).join("debug/kernel");
            let elf = std::fs::read(&elf).unwrap_or_else(|e| panic!("read {}: {e}", elf.display()));
            crate::limine_image::boot_kernel(&elf)
        }
    };
    let initramfs = crate::initramfs::build_initramfs_default(manifest_dir, arch);
    let mut list = format!("# myos {}", Release::current(manifest_dir).text());
    for (name, data) in [("kernel", kernel), ("initramfs", initramfs)] {
        let file = format!("{arch}-{name}");
        let path = out.join(&file);
        std::fs::write(&path, &data).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        list.push_str(&format!("{name} {} {} {file}\n", data.len(), sha256_file(&path)));
    }
    std::fs::write(out.join(format!("{arch}-boot.txt")), list).expect("write the boot file list");
}

/// Every runtime dependency (`PORT_RDEPS`) names a package with files, and
/// no chain of them loops: `get-myos` installs them depth first.
fn check_rdeps(all: &[ports::Port]) {
    let find = |name: &str| all.iter().find(|p| p.name == name);
    for port in all {
        for dep in &port.rdeps {
            let Some(d) = find(dep) else {
                panic!("port {}: PORT_RDEPS names an unknown port {dep:?}", port.name);
            };
            assert!(
                d.role == ports::Role::Package && !d.files.is_empty(),
                "port {}: PORT_RDEPS entry {dep:?} is not a package with files (an image port is always there)",
                port.name
            );
        }
        let mut chain = vec![port.name.as_str()];
        let mut stack: Vec<(&str, usize)> = port.rdeps.iter().map(|d| (d.as_str(), 1)).collect();
        while let Some((name, depth)) = stack.pop() {
            chain.truncate(depth);
            assert!(!chain.contains(&name), "PORT_RDEPS loop: {} -> {name}", chain.join(" -> "));
            chain.push(name);
            if let Some(d) = find(name) {
                stack.extend(d.rdeps.iter().map(|x| (x.as_str(), depth + 1)));
            }
        }
    }
}

/// Run a port's build script when its ready file for `arch` is missing: a
/// package (`packages/`) is not built by `build.rs` like the image's ports,
/// so a local `cargo run -- packages` builds it here (the scripts skip
/// themselves when current; CI has them from the registry).
fn ensure_built(manifest_dir: &Path, port: &ports::Port, arch: &str) {
    let (Some(script), Some(ready)) = (&port.build, port.ready_file(arch)) else {
        return;
    };
    if port.kind == ports::Kind::User || manifest_dir.join("target").join(&ready).exists() {
        return;
    }
    eprintln!("==> packages: {ready} missing; running {script}");
    let status = Command::new("bash")
        .arg(manifest_dir.join(script))
        .current_dir(manifest_dir)
        .status()
        .unwrap_or_else(|e| panic!("run {script}: {e}"));
    assert!(status.success(), "{script} failed; the {} package cannot be built", port.name);
}

/// Build the packages of every architecture (`cargo run -- packages`).
pub fn build_all(manifest_dir: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for arch in ["x86_64", "aarch64", "riscv64"] {
        out = build(manifest_dir, arch);
    }
    out
}

fn sha256_file(path: &Path) -> String {
    let output = Command::new("sha256sum").arg(path).output().expect("run sha256sum");
    assert!(output.status.success(), "sha256sum {} failed", path.display());
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .expect("sha256sum output")
        .to_string()
}

/// A ustar archive of regular files, mtime 0 and uid/gid 0 so the archive
/// is reproducible. The aliases of a program (a hard-link group, which only
/// one entry has the data of) are symlinks to that entry: the guest's tmpfs
/// has no hard links to make, and a copy each would multiply the program
/// (uutils' 32 names).
fn ustar(entries: &[Entry]) -> Vec<u8> {
    let stored: HashMap<u64, &str> = entries
        .iter()
        .filter(|e| e.nlink > 1 && !e.data.is_empty())
        .map(|e| (e.ino, e.name.as_str()))
        .collect();
    let mut out = Vec::new();
    for e in entries {
        let link = stored.get(&e.ino).filter(|n| **n != e.name).map(|n| link_target(&e.name, n));
        let (prefix, name) = split_name(&e.name);
        let mut h = [0u8; 512];
        put(&mut h[0..100], name.as_bytes());
        put(&mut h[100..108], format!("{:07o}", e.mode & 0o7777).as_bytes());
        put(&mut h[108..116], b"0000000");
        put(&mut h[116..124], b"0000000");
        put(&mut h[124..136], format!("{:011o}", e.data.len()).as_bytes());
        put(&mut h[136..148], b"00000000000");
        h[148..156].copy_from_slice(b"        ");
        if let Some(link) = &link {
            h[156] = b'2';
            put(&mut h[157..257], link.as_bytes());
        } else {
            h[156] = b'0';
        }
        put(&mut h[257..263], b"ustar\0");
        put(&mut h[263..265], b"00");
        put(&mut h[265..297], b"root");
        put(&mut h[297..329], b"root");
        put(&mut h[329..337], b"0000000");
        put(&mut h[337..345], b"0000000");
        put(&mut h[345..500], prefix.as_bytes());
        let sum: u32 = h.iter().map(|b| u32::from(*b)).sum();
        put(&mut h[148..156], format!("{sum:06o}\0 ").as_bytes());
        out.extend_from_slice(&h);
        out.extend_from_slice(&e.data);
        let pad = (512 - e.data.len() % 512) % 512;
        out.extend(std::iter::repeat_n(0u8, pad));
    }
    out.extend(std::iter::repeat_n(0u8, 1024));
    out
}

/// `target` as a symlink at `link` names it: relative to the link's
/// directory (both are archive paths), so it resolves wherever the package
/// is unpacked.
fn link_target(link: &str, target: &str) -> String {
    let dir: Vec<&str> = link.split('/').collect();
    let dir = &dir[..dir.len() - 1];
    let target: Vec<&str> = target.split('/').collect();
    let common = dir.iter().zip(&target).take_while(|(a, b)| a == b).count();
    let mut parts = vec![".."; dir.len() - common];
    parts.extend(&target[common..]);
    parts.join("/")
}

/// A path longer than the 100-byte name field goes into prefix/name.
fn split_name(path: &str) -> (&str, &str) {
    if path.len() <= 100 {
        return ("", path);
    }
    let cut = path[..=path.len().min(156) - 1]
        .rfind('/')
        .filter(|i| path.len() - i - 1 <= 100)
        .unwrap_or_else(|| panic!("package path too long for ustar: {path}"));
    (&path[..cut], &path[cut + 1..])
}

fn put(field: &mut [u8], bytes: &[u8]) {
    assert!(bytes.len() <= field.len(), "ustar field overflow");
    field[..bytes.len()].copy_from_slice(bytes);
}

/// Serve `dir` over HTTP on 127.0.0.1:MIRROR_PORT in a background thread
/// (GET only: `/` lists the files, `index.txt` and `packages.txt` are the
/// files of `arch`, anything else is a file of `dir`).
/// False when the port is taken: the launcher then skips the forward and
/// the guest's install stage fails visibly instead of hitting a stranger.
pub fn serve_mirror(dir: PathBuf, arch: &str) -> bool {
    let Ok(listener) = TcpListener::bind(("127.0.0.1", MIRROR_PORT)) else {
        return false;
    };
    let arch = arch.to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let dir = dir.clone();
            let arch = arch.clone();
            std::thread::spawn(move || serve_one(stream, &dir, &arch));
        }
    });
    true
}

fn serve_one(mut stream: TcpStream, dir: &Path, arch: &str) {
    let mut req = Vec::new();
    let mut buf = [0u8; 1024];
    while !req.windows(4).any(|w| w == b"\r\n\r\n") && req.len() < 8192 {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => req.extend_from_slice(&buf[..n]),
        }
    }
    let line = String::from_utf8_lossy(&req);
    let mut words = line.split_whitespace();
    let (method, target) = (words.next().unwrap_or(""), words.next().unwrap_or("/"));
    let path = target.split('?').next().unwrap_or("").trim_start_matches('/');
    // The guest test asks for the arch's files without knowing its arch
    // name: `index.txt` and `packages.txt` are this mirror's `<arch>-...`.
    let per_arch = format!("{arch}-{path}");
    let path = if path == "index.txt" || path == "packages.txt" { per_arch.as_str() } else { path };
    let body: Option<Vec<u8>> = if method != "GET" || path.contains("..") || path.contains('/') {
        None
    } else if path.is_empty() {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect())
            .unwrap_or_default();
        names.sort();
        Some(names.join("\n").into_bytes())
    } else {
        std::fs::read(dir.join(path)).ok()
    };
    let response = match body {
        Some(b) => {
            let mut r = format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len())
                .into_bytes();
            r.extend_from_slice(&b);
            r
        }
        None => b"HTTP/1.0 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    };
    let _ = stream.write_all(&response);
    let _ = stream.flush();
}
