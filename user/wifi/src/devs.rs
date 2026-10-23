//! Devices handed to programs as directories (`/dev/usb/NNN`,
//! `/dev/sdio/N`): finding one by the `id VVVV:PPPP` line of its `info`,
//! and reading such files.

use alloc::vec::Vec;

use myos_user::{close, listdir, open_flags, read, LISTDIR_BUF, O_RDONLY};

/// The first device under `root` (`/dev/usb/`) whose `id VVVV:PPPP` is
/// one of `ids`: its directory and id.
pub fn find(root: &[u8], ids: &[(u16, u16)]) -> Option<(Vec<u8>, (u16, u16))> {
    let mut names = [0u8; LISTDIR_BUF];
    let n = listdir(root.strip_suffix(b"/").unwrap_or(root), &mut names);
    if n == usize::MAX {
        return None;
    }
    for name in names[..n].split(|&c| c == b'\n' || c == 0).filter(|s| !s.is_empty() && s[0] != b'.') {
        let dir = path(root, name, b"");
        let Some(text) = slurp(&path(&dir, b"/info", b"")) else {
            continue;
        };
        let Some(id) = value(&text, b"id ") else {
            continue;
        };
        let mut parts = id.split(|&c| c == b':');
        let (Some(v), Some(p)) = (parts.next().and_then(hex), parts.next().and_then(hex)) else {
            continue;
        };
        if let Some(&id) = ids.iter().find(|&&id| id == (v as u16, p as u16)) {
            return Some((dir, id));
        }
    }
    None
}

pub fn path(a: &[u8], b: &[u8], c: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(a.len() + b.len() + c.len());
    p.extend_from_slice(a);
    p.extend_from_slice(b);
    p.extend_from_slice(c);
    p
}

/// The first KiB of the file at `path` (an `info`).
pub fn slurp(path: &[u8]) -> Option<Vec<u8>> {
    let fd = open_flags(path, O_RDONLY)?;
    let mut buf = [0u8; 1024];
    let n = read(fd, &mut buf);
    close(fd);
    (n != usize::MAX).then(|| buf[..n].to_vec())
}

/// The whole file at `path` (a firmware).
pub fn load(path: &[u8]) -> Option<Vec<u8>> {
    let fd = open_flags(path, O_RDONLY)?;
    let mut data = Vec::new();
    let mut buf = [0u8; 16384];
    loop {
        let n = read(fd, &mut buf);
        if n == usize::MAX {
            close(fd);
            return None;
        }
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
    }
    close(fd);
    Some(data)
}

/// What follows `key` on its line of `text`.
pub fn value<'a>(text: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    text.split(|&c| c == b'\n').find_map(|l| l.strip_prefix(key)).map(|v| v.trim_ascii())
}

pub fn hex(s: &[u8]) -> Option<u64> {
    u64::from_str_radix(core::str::from_utf8(s).ok()?, 16).ok()
}
