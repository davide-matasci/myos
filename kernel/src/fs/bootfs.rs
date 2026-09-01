//! bootfs: flat read-only namespace mounted at `/` by the VFS.
//!
//! Embedded user ELFs are registered at boot via [`init_embedded`]; Limine ESP
//! modules override via [`init_limine`]. Loadable modules add files through
//! `KernelApi::vfs_register` (e.g. the FAT module registers `/msg`).

use spin::Mutex;

use crate::console;
use crate::fs::StatInfo;

const HEAP_ELF: &[u8] = include_bytes!(env!("USER_HEAP_PATH"));
const STD_HELLO_ELF: &[u8] = include_bytes!(env!("USER_STD_HELLO_PATH"));
const STD_CAT_ELF: &[u8] = include_bytes!(env!("USER_STD_CAT_PATH"));
const STD_ECHO_ELF: &[u8] = include_bytes!(env!("USER_STD_ECHO_PATH"));
const BIGALLOC_ELF: &[u8] = include_bytes!(env!("USER_BIGALLOC_PATH"));
const C_HELLO_ELF: &[u8] = include_bytes!(env!("USER_C_HELLO_PATH"));
const OK_ELF: &[u8] = include_bytes!(env!("USER_OK_PATH"));
const SH_ELF: &[u8] = include_bytes!(env!("USER_SH_PATH"));
const ECHO_ELF: &[u8] = include_bytes!(env!("USER_ECHO_PATH"));
const CAT_ELF: &[u8] = include_bytes!(env!("USER_CAT_PATH"));
const LS_ELF: &[u8] = include_bytes!(env!("USER_LS_PATH"));

const MAX_FILES: usize = 32;
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
                log_reg(name, bytes.len(), true);
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
            log_reg(name, bytes.len(), false);
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

/// List the flat root when `rel` is empty or `"."`; otherwise return 0.
pub fn listdir_at(rel: &str, buf: &mut [u8]) -> usize {
    if !rel.is_empty() && rel != "." {
        return 0;
    }
    listdir(buf)
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
        });
    }
    let files = FILES.lock();
    for (i, slot) in files.iter().flatten().enumerate() {
        if slot.len == name.len() && &slot.name[..slot.len] == name.as_bytes() {
            return Some(StatInfo {
                mode: S_IFREG | 0o555,
                size: slot.data.len() as u32,
                ino: (i as u32) + 2,
                nlink: 1,
            });
        }
    }
    None
}

fn log_reg(name: &str, n: usize, replace: bool) {
    let kind = if replace { "replace" } else { "new" };
    console::status_progress(&alloc::format!("vfs {kind} {name} ({n} bytes)"));
}

/// Register embedded user ELFs (Limine modules loaded later may override).
pub fn init_embedded() {
    let _ = register("ok", OK_ELF);
    let _ = register("heap", HEAP_ELF);
    let _ = register("stdhello", STD_HELLO_ELF);
    let _ = register("stdcat", STD_CAT_ELF);
    let _ = register("stdecho", STD_ECHO_ELF);
    let _ = register("bigalloc", BIGALLOC_ELF);
    let _ = register("chello", C_HELLO_ELF);
    let _ = register("sh", SH_ELF);
    let _ = register("echo", ECHO_ELF);
    let _ = register("cat", CAT_ELF);
    let _ = register("ls", LS_ELF);
}

/// Register Limine-mapped modules by basename (overrides embedded names).
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
        let _ = register(name, bytes);
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
