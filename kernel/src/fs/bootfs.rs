//! bootfs: Limine modules by basename, plus an embedded `/ok` fallback.
//! Extra files (e.g. `/msg` from the FAT16 module) land here via `register`.

use spin::Mutex;

use crate::console;

const HEAP_ELF: &[u8] = include_bytes!(env!("USER_HEAP_PATH"));
const STD_HELLO_ELF: &[u8] = include_bytes!(env!("USER_STD_HELLO_PATH"));
const STD_CAT_ELF: &[u8] = include_bytes!(env!("USER_STD_CAT_PATH"));
const STD_ECHO_ELF: &[u8] = include_bytes!(env!("USER_STD_ECHO_PATH"));
const OK_ELF: &[u8] = include_bytes!(env!("USER_OK_PATH"));
const SH_ELF: &[u8] = include_bytes!(env!("USER_SH_PATH"));
const ECHO_ELF: &[u8] = include_bytes!(env!("USER_ECHO_PATH"));
const CAT_ELF: &[u8] = include_bytes!(env!("USER_CAT_PATH"));
const LS_ELF: &[u8] = include_bytes!(env!("USER_LS_PATH"));
const MAX_FILES: usize = 16;
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

fn log_reg(name: &str, n: usize, replace: bool) {
    let kind = if replace { "replace" } else { "new" };
    console::status_progress(&alloc::format!("vfs {kind} {name} ({n} bytes)"));
}

pub fn init() {
    let _ = register("ok", OK_ELF);
    let _ = register("heap", HEAP_ELF);
    let _ = register("stdhello", STD_HELLO_ELF);
    let _ = register("stdcat", STD_CAT_ELF);
    let _ = register("stdecho", STD_ECHO_ELF);
    let _ = register("sh", SH_ELF);
    let _ = register("echo", ECHO_ELF);
    let _ = register("cat", CAT_ELF);
    let _ = register("ls", LS_ELF);
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
