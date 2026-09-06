//! binfs: nested read-only tree mounted at `/bin/…`.
//!
//! Holds all on-disk user programs grouped by category (`/bin/std/…`,
//! `/bin/custom/…`, `/bin/sbase/…`, `/bin/ubase/…`, `/bin/tcc/…`,
//! `/bin/coreutils/…`). Port backends are mounted at their own `/bin/…`
//! prefixes (longer prefix wins in `resolve_index`); this backend only
//! provides the `/bin` directory nodes and the leaf programs that live
//! under `/bin/std` and `/bin/custom`, `/bin/modules`, `/bin/etc`.
//!
//! Paths are relative to the mount, e.g. `std/cat`, `custom/netd`.
//! Directories are implied by children.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::fs::StatInfo;

const PATH_CAP: usize = 96;
const MAX_FILES: usize = 64;

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;

struct File {
    path: String,
    data: &'static [u8],
}

static FILES: Mutex<Vec<File>> = Mutex::new(Vec::new());

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
    if let Some(e) = files.iter_mut().find(|e| e.path == name) {
        e.data = bytes;
        return true;
    }
    if files.len() >= MAX_FILES {
        return false;
    }
    files.push(File {
        path: String::from(name),
        data: bytes,
    });
    true
}

pub fn lookup(name: &str) -> Option<&'static [u8]> {
    if !valid_rel(name) {
        return None;
    }
    let files = FILES.lock();
    files.iter().find(|e| e.path == name).map(|e| e.data)
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
    files.iter().any(|e| {
        e.path == dir
            || (e.path.starts_with(dir) && e.path.as_bytes().get(dir.len()) == Some(&b'/'))
    })
}

pub fn listdir_at(rel: &str, buf: &mut [u8]) -> usize {
    let dir = if rel.is_empty() || rel == "." {
        ""
    } else if valid_rel(rel) {
        rel
    } else {
        return 0;
    };
    let files = FILES.lock();
    if !dir.is_empty() && !is_dir_path(&files, dir) {
        return 0;
    }
    let mut n = 0;
    let mut seen: Vec<&str> = Vec::new();
    for e in files.iter() {
        let child = if dir.is_empty() {
            match e.path.split_once('/') {
                Some((head, _)) => head,
                None => e.path.as_str(),
            }
        } else if e.path.starts_with(dir)
            && e.path.as_bytes().get(dir.len()) == Some(&b'/')
        {
            let rest = &e.path[dir.len() + 1..];
            match rest.split_once('/') {
                Some((head, _)) => head,
                None => rest,
            }
        } else {
            continue;
        };
        if seen.iter().any(|s| *s == child) {
            continue;
        }
        seen.push(child);
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
        });
    }
    if !valid_rel(name) {
        return None;
    }
    let files = FILES.lock();
    if let Some((i, e)) = files.iter().enumerate().find(|(_, e)| e.path == name) {
        return Some(StatInfo {
            mode: S_IFREG | 0o555,
            size: e.data.len() as u32,
            ino: (i as u32) + 2,
            nlink: 1,
            dev: 0,
        });
    }
    if is_dir_path(&files, name) {
        return Some(StatInfo {
            mode: S_IFDIR | 0o755,
            size: 0,
            ino: 1,
            nlink: 2,
            dev: 0,
        });
    }
    None
}

/// Register embedded builtin programs by their `/bin/<category>/<name>` path
/// (relative to the `/bin` mount). These were previously flat at the root (on
/// bootfs); remapping gathers them under a nested, typed tree.
pub fn init_embedded() {
    let _ = register("std/bigalloc", BIGALLOC_ELF);
    let _ = register("std/cat", STD_CAT_ELF);
    let _ = register("std/echo", STD_ECHO_ELF);
    let _ = register("std/hello", STD_HELLO_ELF);
    let _ = register("etc/hello", C_HELLO_ELF);
    let _ = register("modules/hello", HELLO_MODULE_ELF);
    let _ = register("custom/heap", HEAP_ELF);
    let _ = register("custom/ok", OK_ELF);
    let _ = register("custom/sh", SH_ELF);
    let _ = register("custom/cat", CAT_ELF);
    let _ = register("custom/echo", ECHO_ELF);
    let _ = register("custom/ls", LS_ELF);
    let _ = register("custom/mount", MOUNT_ELF);
    let _ = register("custom/mkfs.ext2", MKFS_EXT2_ELF);
    let _ = register("custom/ping", PING_ELF);
    let _ = register("custom/http", HTTP_ELF);
    let _ = register("custom/dns", DNS_ELF);
    let _ = register("custom/netd", NETD_ELF);
}

const HEAP_ELF: &[u8] = include_bytes!(env!("USER_HEAP_PATH"));
const STD_HELLO_ELF: &[u8] = include_bytes!(env!("USER_STD_HELLO_PATH"));
const STD_CAT_ELF: &[u8] = include_bytes!(env!("USER_STD_CAT_PATH"));
const STD_ECHO_ELF: &[u8] = include_bytes!(env!("USER_STD_ECHO_PATH"));
const BIGALLOC_ELF: &[u8] = include_bytes!(env!("USER_BIGALLOC_PATH"));
const C_HELLO_ELF: &[u8] = include_bytes!(env!("USER_C_HELLO_PATH"));
const HELLO_MODULE_ELF: &[u8] = include_bytes!(env!("HELLO_MODULE_PATH"));
const OK_ELF: &[u8] = include_bytes!(env!("USER_OK_PATH"));
const SH_ELF: &[u8] = include_bytes!(env!("USER_SH_PATH"));
const ECHO_ELF: &[u8] = include_bytes!(env!("USER_ECHO_PATH"));
const CAT_ELF: &[u8] = include_bytes!(env!("USER_CAT_PATH"));
const LS_ELF: &[u8] = include_bytes!(env!("USER_LS_PATH"));
const MOUNT_ELF: &[u8] = include_bytes!(env!("USER_MOUNT_PATH"));
const MKFS_EXT2_ELF: &[u8] = include_bytes!(env!("USER_MKFS_EXT2_PATH"));
const PING_ELF: &[u8] = include_bytes!(env!("USER_PING_PATH"));
const HTTP_ELF: &[u8] = include_bytes!(env!("USER_HTTP_PATH"));
const DNS_ELF: &[u8] = include_bytes!(env!("USER_DNS_PATH"));
const NETD_ELF: &[u8] = include_bytes!(env!("USER_NETD_PATH"));