//! ptsfs: slave nodes at `/dev/pts/N` (one per live pty pair, see [`crate::pty`]).
//!
//! Stat/list-only backend: pty fd I/O does not route through VFS — opening a
//! slave node creates an `FdEntry::PtySlave` fd in [`crate::task`], and all
//! reads/writes/ioctls go through the pty module directly. This backend
//! exists so the POSIX `/dev/pts/N` naming (and `listdir` for pty discovery)
//! resolves through the normal VFS tree.

use crate::fs::{IoctlResult, StatInfo};
use crate::pty;

const S_IFDIR: u32 = 0o040000;
const S_IFCHR: u32 = 0o020000;

fn parse_index(name: &str) -> Option<usize> {
    if name.is_empty() || name.len() > 2 {
        return None;
    }
    let n: usize = name.parse().ok()?;
    if n >= pty::MAX_PTYS || !pty::slave_exists(n) {
        return None;
    }
    Some(n)
}

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

pub fn read(_name: &str, _pos: usize, _out: &mut [u8]) -> usize {
    0
}

pub fn write(_name: &str, _pos: usize, _buf: &[u8]) -> Option<usize> {
    None
}

pub fn listdir_at(rel: &str, buf: &mut [u8]) -> usize {
    if !rel.is_empty() && rel != "." {
        return 0;
    }
    let mut n = 0;
    for id in 0..pty::MAX_PTYS {
        if !pty::slave_exists(id) {
            continue;
        }
        let name = alloc::format!("{}", id);
        let need = name.len() + 1;
        if n + need > buf.len() {
            break;
        }
        buf[n..n + name.len()].copy_from_slice(name.as_bytes());
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
    let idx = parse_index(name)?;
    Some(StatInfo {
        mode: S_IFCHR | 0o620,
        size: 0,
        ino: 200 + idx as u32,
        nlink: 1,
        dev: 0,
    })
}

pub fn ioctl(_name: &str, _request: usize, _arg: usize) -> IoctlResult {
    IoctlResult::Notty
}
