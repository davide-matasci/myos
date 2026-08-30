//! Tiny VFS: a mount table. First backend is bootfs (embed, Limine, vfs_register).

pub mod bootfs;

/// A file whose bytes live for the life of the kernel (Limine map or embed).
pub struct File {
    pub data: &'static [u8],
}

struct Mount {
    #[allow(dead_code)]
    name: &'static str,
    lookup: fn(&str) -> Option<&'static [u8]>,
}

static MOUNTS: &[Mount] = &[Mount {
    name: "bootfs",
    lookup: bootfs::lookup,
}];

/// Metadata returned by [`stat`].
pub struct StatInfo {
    pub mode: u32,
    pub size: u32,
    pub ino: u32,
    pub nlink: u32,
}

/// Look up `path` (`/ok` or `ok`) on the first matching mount.
pub fn lookup(path: &str) -> Option<&'static [u8]> {
    let name = normalize_path(path);
    if name.is_empty() {
        return None;
    }
    for m in MOUNTS {
        if let Some(data) = (m.lookup)(name) {
            return Some(File { data }.data);
        }
    }
    None
}

/// Stat a bootfs basename or the flat root directory (`.` / `/`).
pub fn stat(path: &str) -> Option<StatInfo> {
    bootfs::stat(normalize_path(path))
}

fn normalize_path(path: &str) -> &str {
    path.trim_start_matches('/')
}

/// List bootfs basenames into `buf` (newline-separated).
pub fn listdir(buf: &mut [u8]) -> usize {
    bootfs::listdir(buf)
}

/// Register the embedded `/ok` fallback, then Limine modules (ESP wins).
pub fn init() {
    bootfs::init();
}
