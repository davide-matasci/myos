//! Virtual filesystem: path resolution and mount dispatch.
//!
//! Backends (e.g. [`super::bootfs`]) implement [`MountOps`] and are attached
//! with [`mount`]. The root mount uses an empty prefix so `/ok` and `ok` both
//! resolve on bootfs today; future mounts can use a prefix such as `fat/`.

use alloc::vec::Vec;
use spin::Mutex;

/// Metadata returned by [`stat`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatInfo {
    pub mode: u32,
    pub size: u32,
    pub ino: u32,
    pub nlink: u32,
}

/// Operations provided by a mounted filesystem backend.
#[derive(Clone, Copy)]
pub struct MountOps {
    pub lookup: fn(&str) -> Option<&'static [u8]>,
    pub stat: fn(&str) -> Option<StatInfo>,
    pub listdir: fn(&mut [u8]) -> usize,
    pub register: fn(&str, &'static [u8]) -> bool,
}

struct Mount {
    name: &'static str,
    /// Path prefix without leading slash (`""` = root / flat namespace).
    prefix: &'static str,
    ops: MountOps,
}

static MOUNTS: Mutex<Vec<Mount>> = Mutex::new(Vec::new());

/// Attach a backend at `prefix` (empty string = root).
pub fn mount(name: &'static str, prefix: &'static str, ops: MountOps) {
    MOUNTS.lock().push(Mount { name, prefix, ops });
}

/// Look up `path` on the best matching mount.
pub fn lookup(path: &str) -> Option<&'static [u8]> {
    let rel = normalize_path(path);
    if rel.is_empty() {
        return None;
    }
    let (ops, rel) = resolve(path)?;
    (ops.lookup)(rel)
}

/// Stat `path` on the best matching mount.
pub fn stat(path: &str) -> Option<StatInfo> {
    let (ops, rel) = resolve(path)?;
    (ops.stat)(rel)
}

/// List directory entries into `buf` (newline-separated basenames).
pub fn listdir(buf: &mut [u8]) -> usize {
    let mounts = MOUNTS.lock();
    let Some(m) = mounts.first() else {
        return 0;
    };
    (m.ops.listdir)(buf)
}

/// Register `name` on mount `mount_name` (e.g. `"bootfs"`).
pub fn register(mount_name: &str, name: &str, bytes: &'static [u8]) -> bool {
    let mounts = MOUNTS.lock();
    let Some(m) = mounts.iter().find(|m| m.name == mount_name) else {
        return false;
    };
    (m.ops.register)(name, bytes)
}

fn resolve(path: &str) -> Option<(MountOps, &str)> {
    let path = normalize_path(path);
    let mounts = MOUNTS.lock();
    let mut best: Option<(usize, MountOps, &str)> = None;
    for m in mounts.iter() {
        let (score, rel) = match m.prefix {
            "" => (0, path),
            prefix => {
                if path == prefix {
                    (prefix.len(), "")
                } else if path.starts_with(prefix) && path.as_bytes().get(prefix.len()) == Some(&b'/')
                {
                    (prefix.len(), &path[prefix.len() + 1..])
                } else {
                    continue;
                }
            }
        };
        let prev_score = best.as_ref().map(|(s, _, _)| *s);
        if prev_score.is_none() || score > prev_score.unwrap() {
            best = Some((score, m.ops, rel));
        }
    }
    best.map(|(_, ops, rel)| (ops, rel))
}

fn normalize_path(path: &str) -> &str {
    path.trim_start_matches('/')
}
