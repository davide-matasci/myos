//! procfs: generated nodes at `/proc/…` (`mounts`).

use crate::fs::StatInfo;
use crate::fs::vfs;

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;

/// No static file bytes; open uses [`stat`] / custom read.
pub fn lookup(_name: &str) -> Option<&'static [u8]> {
    None
}

pub fn register(_name: &str, _bytes: &'static [u8]) -> bool {
    false
}

pub fn create(_name: &str) -> bool {
    false
}

pub fn truncate(_name: &str) -> bool {
    false
}

pub fn write(_name: &str, _pos: usize, _buf: &[u8]) -> Option<usize> {
    None
}

pub fn read(name: &str, pos: usize, out: &mut [u8]) -> usize {
    if name != "mounts" {
        return 0;
    }
    copy_at(&vfs::mounts_text(), pos, out)
}

pub fn listdir_at(rel: &str, buf: &mut [u8]) -> usize {
    if !rel.is_empty() && rel != "." {
        return 0;
    }
    const NAME: &[u8] = b"mounts";
    if NAME.len() + 1 > buf.len() {
        return 0;
    }
    buf[..NAME.len()].copy_from_slice(NAME);
    buf[NAME.len()] = b'\n';
    NAME.len() + 1
}

pub fn stat(name: &str) -> Option<StatInfo> {
    if name.is_empty() || name == "." || name == ".." {
        return Some(StatInfo {
            mode: S_IFDIR | 0o555,
            size: 0,
            ino: 1,
            nlink: 2,
        });
    }
    if name != "mounts" {
        return None;
    }
    let text = vfs::mounts_text();
    Some(StatInfo {
        mode: S_IFREG | 0o444,
        size: u32::try_from(text.len()).unwrap_or(u32::MAX),
        ino: 2,
        nlink: 1,
    })
}

fn copy_at(data: &[u8], pos: usize, out: &mut [u8]) -> usize {
    let n = out.len().min(data.len().saturating_sub(pos));
    if n != 0 {
        out[..n].copy_from_slice(&data[pos..pos + n]);
    }
    n
}
