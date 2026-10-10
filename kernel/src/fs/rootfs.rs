//! rootfs: the read-only tree mounted at `/`, every file the image carries.
//!
//! The initramfs ([`init_limine`], [`crate::fs::cpio`]) registers its files
//! here by absolute path without the leading `/` (`bin/custom/init`,
//! `lib/newlib/lib/libc.a`, `etc/passwd`); a module adds one at runtime with
//! `vfs_register`. A directory is implied by the files below
//! it, so an empty one carries a `.keep` file (`mnt/.keep`). The kernel's
//! own trees (`/tmp`, `/dev`, `/proc`) and module filesystems are mounts
//! over it.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::fs::StatInfo;

const PATH_CAP: usize = 160;
// os-test alone contributes ~6.5k lib/ files; a smaller cap silently
// dropped every later registration, leaving most of the tree unreadable.
const MAX_FILES: usize = 16384;

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;

struct File {
    path: String,
    data: &'static [u8],
}

static FILES: Mutex<Vec<File>> = Mutex::new(Vec::new());
// Lazy-sort flag: registrations arrive in arbitrary order at boot; the first
// read sorts once, after which every read path is binary search. A later
// registration (a module's) unsorts it again.
static SORTED: Mutex<bool> = Mutex::new(false);

/// Sort by path, keeping the last registration of a path (a module's
/// `vfs_register` replaces a file).
fn ensure_sorted(files: &mut Vec<File>) {
    let mut sorted = SORTED.lock();
    if !*sorted {
        // Stable: registrations of one path stay in order.
        files.sort_by(|a, b| a.path.cmp(&b.path));
        files.dedup_by(|later, earlier| {
            let same = later.path == earlier.path;
            if same {
                earlier.data = later.data;
            }
            same
        });
        *sorted = true;
    }
}

fn valid_rel(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= PATH_CAP
        && !path.starts_with('/')
        && !path.ends_with('/')
        && !path.contains("//")
}

pub fn register(name: &str, bytes: &'static [u8]) -> bool {
    if !valid_rel(name) {
        return false;
    }
    let mut files = FILES.lock();
    if files.len() >= MAX_FILES {
        return false;
    }
    // Appended: the next read sorts (no per-register insert, which was
    // O(n^2) over the ~6.8k os-test registrations).
    files.push(File {
        path: String::from(name),
        data: bytes,
    });
    *SORTED.lock() = false;
    true
}

pub fn lookup(name: &str) -> Option<&'static [u8]> {
    if !valid_rel(name) {
        return None;
    }
    let mut files = FILES.lock();
    ensure_sorted(&mut files);
    files
        .binary_search_by(|e| e.path.as_str().cmp(name))
        .ok()
        .map(|i| files[i].data)
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

fn is_dir_path(files: &[File], dir: &str) -> bool {
    if dir.is_empty() {
        return true;
    }
    // Exact entry...
    let idx = files.partition_point(|e| e.path.as_str() < dir);
    if files.get(idx).map(|e| e.path.as_str()) == Some(dir) {
        return true;
    }
    // ...or any child below dir/ (sorted, so a prefix partition point suffices).
    let mut prefix = String::from(dir);
    prefix.push('/');
    let idx = files.partition_point(|e| e.path.as_str() < prefix.as_str());
    files
        .get(idx)
        .map(|e| e.path.starts_with(prefix.as_str()))
        .unwrap_or(false)
}

pub fn listdir_at(rel: &str, buf: &mut [u8]) -> usize {
    let dir = if rel.is_empty() || rel == "." {
        ""
    } else if valid_rel(rel) {
        rel
    } else {
        return 0;
    };
    let mut files = FILES.lock();
    // partition_point below requires sorted paths; sort lazily on first read.
    ensure_sorted(&mut files);
    if !dir.is_empty() && !is_dir_path(&files, dir) {
        return 0;
    }
    let mut n = 0;
    // Sorted paths: children of dir are contiguous; identical first
    // components are adjacent, so tracking the previous child suffices
    // (the old seen-Vec dedup was O(n^2) over 6.5k files).
    let prefix = if dir.is_empty() {
        String::from("")
    } else {
        let mut p = String::from(dir);
        p.push('/');
        p
    };
    let start = if prefix.is_empty() {
        0
    } else {
        files.partition_point(|e| e.path.as_str() < prefix.as_str())
    };
    let mut last_child: Option<&str> = None;
    for e in files[start..].iter() {
        let rest = if prefix.is_empty() {
            e.path.as_str()
        } else {
            match e.path.strip_prefix(prefix.as_str()) {
                Some(r) => r,
                None => break,
            }
        };
        let child = match rest.split_once('/') {
            Some((head, _)) => head,
            None => rest,
        };
        if last_child == Some(child) {
            continue;
        }
        last_child = Some(child);
        let name = child.as_bytes();
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

pub fn stat(name: &str) -> Option<StatInfo> {
    if name.is_empty() || name == "." || name == ".." {
        return Some(StatInfo {
            mode: S_IFDIR | 0o755,
            size: 0,
            ino: 1,
            nlink: 2,
            dev: 0,
            mtime: 0,
            atime: 0,
        });
    }
    if !valid_rel(name) {
        return None;
    }
    let mut files = FILES.lock();
    ensure_sorted(&mut files);
    if let Ok(i) = files.binary_search_by(|e| e.path.as_str().cmp(name)) {
        let e = &files[i];
        return Some(StatInfo {
            mode: S_IFREG | 0o755,
            size: e.data.len() as u64,
            ino: crate::fs::vfs::data_ino(e.data),
            nlink: 1,
            dev: 0,
            mtime: 0,
            atime: 0,
        });
    }
    if is_dir_path(&files, name) {
        return Some(StatInfo {
            mode: S_IFDIR | 0o755,
            size: 0,
            ino: crate::fs::vfs::dir_ino(name),
            nlink: 2,
            dev: 0,
            mtime: 0,
            atime: 0,
        });
    }
    None
}

/// Unpack the initramfs, the Limine module named `initramfs`, into the
/// tree ([`crate::fs::cpio`]). It carries the kernel modules too
/// (`modules::load_boot_modules`).
pub fn init_limine() {
    let Some(resp) = crate::limine_boot::MODULES.response() else {
        return;
    };
    for file in resp.modules() {
        if file.path().rsplit('/').next() != Some("initramfs") {
            continue;
        }
        let data = file.data();
        // Limine keeps module mappings for the life of the kernel.
        let bytes: &'static [u8] =
            unsafe { core::slice::from_raw_parts(data.as_ptr(), data.len()) };
        let n = crate::fs::cpio::parse(bytes);
        crate::console::status_ok(&alloc::format!("initramfs: {n} files"));
    }
}
