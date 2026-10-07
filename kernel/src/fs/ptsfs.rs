//! ptsfs: the ptys at `/dev/pts/…` (`docs/tty.md`, [`crate::pty`]).
//!
//! `clone` hands out pairs: opening it allocates one and returns the master
//! fd. Each live pair is a directory `N/` with `master` (the end `clone`
//! returned, the only way to hold it), `data` (the slave, the terminal a
//! session runs on) and `ctl` (the pair's control file: termios, window
//! size, `ctty`, `flush`, as text).
//!
//! Only `ctl` is read and written through the VFS. Opening `clone` or
//! `data` makes a pty fd in [`crate::task`] whose I/O goes straight to the
//! pty module, so this backend is stat/list for those, and `master` is
//! never openable by path.

use crate::fs::StatInfo;
use crate::pty;

const S_IFDIR: u32 = 0o040000;
const S_IFCHR: u32 = 0o020000;
const S_IFREG: u32 = 0o100000;

enum Node {
    Clone,
    /// `N`, the pair's directory.
    Dir(usize),
    Master(usize),
    Data(usize),
    Ctl(usize),
}

fn parse_index(name: &str) -> Option<usize> {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: usize = name.parse().ok()?;
    if !pty::slave_exists(n) {
        return None;
    }
    Some(n)
}

fn parse(name: &str) -> Option<Node> {
    if name == "clone" {
        return Some(Node::Clone);
    }
    let (index, member) = match name.split_once('/') {
        Some((index, member)) => (index, Some(member)),
        None => (name, None),
    };
    let id = parse_index(index)?;
    match member {
        None => Some(Node::Dir(id)),
        Some("master") => Some(Node::Master(id)),
        Some("data") => Some(Node::Data(id)),
        Some("ctl") => Some(Node::Ctl(id)),
        Some(_) => None,
    }
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

/// A control file takes the shell's truncating open (`echo … > ctl`).
pub fn truncate(name: &str) -> bool {
    matches!(parse(name), Some(Node::Ctl(_)))
}

pub fn read(name: &str, pos: usize, out: &mut [u8]) -> usize {
    let Some(Node::Ctl(id)) = parse(name) else {
        return 0;
    };
    let Some(text) = pty::ctl_text(id) else {
        return 0;
    };
    let n = out.len().min(text.len().saturating_sub(pos));
    if n != 0 {
        out[..n].copy_from_slice(&text[pos..pos + n]);
    }
    n
}

pub fn write(name: &str, _pos: usize, buf: &[u8]) -> Option<usize> {
    match parse(name)? {
        Node::Ctl(id) => pty::ctl_write(id, buf),
        _ => None,
    }
}

fn push_name(buf: &mut [u8], n: &mut usize, name: &[u8]) -> bool {
    if *n + name.len() + 1 > buf.len() {
        return false;
    }
    buf[*n..*n + name.len()].copy_from_slice(name);
    *n += name.len();
    buf[*n] = b'\n';
    *n += 1;
    true
}

pub fn listdir_at(rel: &str, buf: &mut [u8]) -> usize {
    let mut n = 0;
    if rel.is_empty() || rel == "." {
        push_name(buf, &mut n, b"clone");
        for id in 0..pty::id_bound() {
            if !pty::slave_exists(id) {
                continue;
            }
            let name = alloc::format!("{}", id);
            if !push_name(buf, &mut n, name.as_bytes()) {
                break;
            }
        }
        return n;
    }
    if let Some(Node::Dir(_)) = parse(rel) {
        for name in [b"master".as_slice(), b"data".as_slice(), b"ctl".as_slice()] {
            if !push_name(buf, &mut n, name) {
                break;
            }
        }
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
    // Inodes: 2 for clone, then four per pair (its directory and members).
    let ino = |id: usize, member: u32| {
        u32::try_from(id).map_or(u32::MAX, |id| 10 + id.saturating_mul(4).saturating_add(member))
    };
    let (mode, ino, size) = match parse(name)? {
        Node::Clone => (S_IFCHR | 0o666, 2, 0),
        Node::Dir(id) => (S_IFDIR | 0o755, ino(id, 0), 0),
        Node::Master(id) => (S_IFCHR | 0o600, ino(id, 1), 0),
        Node::Data(id) => (S_IFCHR | 0o620, ino(id, 2), 0),
        Node::Ctl(id) => {
            let len = pty::ctl_text(id).map_or(0, |t| t.len());
            (S_IFREG | 0o644, ino(id, 3), u32::try_from(len).unwrap_or(u32::MAX))
        }
    };
    Some(StatInfo {
        mode,
        size,
        ino,
        nlink: if mode & S_IFDIR != 0 { 2 } else { 1 },
        dev: 0,
        mtime: 0,
        atime: 0,
    })
}
