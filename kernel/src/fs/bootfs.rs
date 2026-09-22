//! bootfs: flat read-only namespace mounted at `/` by the VFS.
//!
//! Embedded user ELFs were moved to the nested `/bin/<category>/…` tree in
//! [`binfs`] (issue #79), so bootfs now only holds Limine-mapped modules and
//! anything loaded at runtime via `KernelApi::vfs_register`.

use spin::Mutex;

use crate::fs::StatInfo;

const MAX_FILES: usize = 48;
const NAME_CAP: usize = 32;

#[derive(Clone, Copy)]
struct Slot {
    name: [u8; NAME_CAP],
    len: usize,
    data: &'static [u8],
}

static FILES: Mutex<[Option<Slot>; MAX_FILES]> = Mutex::new([None; MAX_FILES]);

pub fn register(name: &str, bytes: &'static [u8]) -> bool {
    if name.is_empty() {
        return false;
    }
    let len = name.len().min(NAME_CAP);
    let mut n = [0u8; NAME_CAP];
    n[..len].copy_from_slice(&name.as_bytes()[..len]);

    let mut files = FILES.lock();
    for slot in files.iter_mut() {
        if let Some(s) = slot {
            if s.len == len && s.name[..len] == n[..len] {
                s.data = bytes;
                return true;
            }
        }
    }
    for slot in files.iter_mut() {
        if slot.is_none() {
            *slot = Some(Slot {
                name: n,
                len,
                data: bytes,
            });
            return true;
        }
    }
    false
}

pub fn lookup(name: &str) -> Option<&'static [u8]> {
    let files = FILES.lock();
    for slot in files.iter().flatten() {
        if slot.len == name.len() && &slot.name[..slot.len] == name.as_bytes() {
            return Some(slot.data);
        }
    }
    None
}

/// True when `name` is a directory in the flat namespace: the mount root,
/// or a prefix of any registered entry followed by `/`. Flat entries like
/// `root/.ssh/authorized_keys` therefore expose real `/root` and `/root/.ssh`
/// directories, which tools that stat every path component (e.g. dropbear's
/// authorized_keys permission walk) require.
fn is_dir_prefix(name: &str) -> bool {
    if name.is_empty() || name == "." || name == ".." {
        return true;
    }
    let needle = alloc::format!("{name}/");
    let nb = needle.as_bytes();
    let files = FILES.lock();
    for slot in files.iter().flatten() {
        let s = &slot.name[..slot.len];
        if s.len() > nb.len() && &s[..nb.len()] == nb {
            return true;
        }
    }
    false
}

pub fn create(_name: &str) -> bool {
    false
}

pub fn truncate(_name: &str) -> bool {
    false
}

pub fn read(name: &str, pos: usize, out: &mut [u8]) -> usize {
    let Some(data) = lookup(name) else {
        return 0;
    };
    let n = out.len().min(data.len().saturating_sub(pos));
    if n != 0 {
        out[..n].copy_from_slice(&data[pos..pos + n]);
    }
    n
}

pub fn write(_name: &str, _pos: usize, _buf: &[u8]) -> Option<usize> {
    None
}

pub fn listdir_at(rel: &str, buf: &mut [u8]) -> usize {
    if rel.is_empty() || rel == "." {
        return listdir(buf);
    }
    // Subdirectory of the flat namespace: list the immediate child basenames
    // of every entry under `rel/` (so `/root` lists `.ssh`, etc.).
    if !is_dir_prefix(rel) {
        return 0;
    }
    let prefix = alloc::format!("{rel}/");
    let pb = prefix.as_bytes();
    let files = FILES.lock();
    let mut n = 0;
    for slot in files.iter().flatten() {
        let s = &slot.name[..slot.len];
        if s.len() <= pb.len() || &s[..pb.len()] != pb {
            continue;
        }
        let rest = &s[pb.len()..];
        let child = match rest.iter().position(|&c| c == b'/') {
            Some(p) => &rest[..p],
            None => rest,
        };
        if child.is_empty() || n + child.len() + 1 > buf.len() {
            continue;
        }
        // Skip duplicates from multiple entries sharing a subdirectory.
        let mut base = 0;
        let mut dup = false;
        while base < n {
            let end = buf[base..n].iter().position(|&c| c == b'\n').map(|p| base + p).unwrap_or(n);
            if buf[base..end] == *child {
                dup = true;
                break;
            }
            base = end + 1;
        }
        if dup {
            continue;
        }
        buf[n..n + child.len()].copy_from_slice(child);
        n += child.len();
        buf[n] = b'\n';
        n += 1;
    }
    n
}

/// Copy newline-separated basenames into `buf`. Returns bytes written.
pub fn listdir(buf: &mut [u8]) -> usize {
    let files = FILES.lock();
    let mut n = 0;
    for slot in files.iter().flatten() {
        let name = &slot.name[..slot.len];
        let need = name.len() + 1;
        if n + need > buf.len() {
            break;
        }
        buf[n..n + name.len()].copy_from_slice(name);
        n += name.len();
        buf[n] = b'\n';
        n += 1;
    }
    n
}

/// Number of registered bootfs entries.
pub fn count() -> usize {
    FILES.lock().iter().flatten().count()
}

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;

/// Stat the flat root (`.` / `/` / empty) or a bootfs basename.
pub fn stat(name: &str) -> Option<StatInfo> {
    if name.is_empty() || name == "." || name == ".." {
        return Some(StatInfo {
            mode: S_IFDIR | 0o755,
            size: 0,
            ino: 1,
            nlink: 2,
            dev: 0,
        });
    }
    if is_dir_prefix(name) {
        return Some(StatInfo {
            mode: S_IFDIR | 0o755,
            size: 0,
            ino: crate::fs::vfs::dir_ino(name),
            nlink: 2,
            dev: 0,
        });
    }
    let files = FILES.lock();
    for slot in files.iter().flatten() {
        if slot.len == name.len() && &slot.name[..slot.len] == name.as_bytes() {
            return Some(StatInfo {
                mode: S_IFREG | 0o555,
                size: slot.data.len() as u32,
                ino: crate::fs::vfs::data_ino(slot.data),
                nlink: 1,
                dev: 0,
            });
        }
    }
    None
}

/// Nothing embedded anymore: builtin ELFs live in [`binfs`] under `/bin/…`.
pub fn init_embedded() {}

/// Register Limine-mapped modules under `/bin/…` (remapped in issue #79).
///
/// The bios/uefi image embeds `boot/hello` (the hello demo module, e.g. the
/// LF `modules/hello` tree) and `boot/ok` (user/ok). These land in their
/// `/bin` category now instead of the flat root: `hello` -> `/bin/modules/hello`,
/// everything else -> `/bin/custom/<basename>`.
///
/// The `boot/initramfs` module is special: it is a newc archive of the
/// userspace ELFs (sbase, coreutils, ripgrep, tcc, std, custom, and the newlib
/// sysroot). It is parsed by [`crate::fs::cpio`] and each entry is registered
/// into the matching mount, overriding the small embedded fallback so the cpio
/// is the primary source of userspace.
pub fn init_limine() {
    let Some(resp) = crate::limine_boot::MODULES.response() else {
        return;
    };
    for file in resp.modules() {
        let name = basename(file.path());
        if name.is_empty() {
            continue;
        }
        let data = file.data();
        // Limine keeps module mappings for the life of the kernel.
        let bytes: &'static [u8] =
            unsafe { core::slice::from_raw_parts(data.as_ptr(), data.len()) };
        if name == "initramfs" {
            let n = crate::fs::cpio::parse(bytes);
            crate::console::status_ok(&alloc::format!("initramfs: {n} files"));
            continue;
        }
        let rel = if name == "hello" {
            alloc::format!("modules/{name}")
        } else {
            alloc::format!("custom/{name}")
        };
        let _ = crate::fs::binfs::register(&rel, bytes);
    }
}

/// Basename of a Limine module path (`boot():/boot/ok` -> `ok`).
fn basename(path: &str) -> &str {
    let p = path.strip_prefix("boot():").unwrap_or(path);
    let p = p.trim_start_matches('/');
    match p.rsplit_once('/') {
        Some((_, name)) => name,
        None => p,
    }
}
