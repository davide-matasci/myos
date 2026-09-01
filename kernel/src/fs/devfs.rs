//! devfs: device nodes at `/dev/…` (`null`, `tty`, `console`).

use crate::fs::StatInfo;
use crate::input;
use crate::task;

const S_IFDIR: u32 = 0o040000;
const S_IFCHR: u32 = 0o020000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Node {
    Null,
    Tty,
    Console,
}

fn parse(name: &str) -> Option<Node> {
    match name {
        "null" => Some(Node::Null),
        "tty" => Some(Node::Tty),
        "console" => Some(Node::Console),
        _ => None,
    }
}

/// No static file bytes; open uses [`stat`] / custom read-write.
pub fn lookup(_name: &str) -> Option<&'static [u8]> {
    None
}

pub fn register(_name: &str, _bytes: &'static [u8]) -> bool {
    false
}

pub fn create(_name: &str) -> bool {
    false
}

pub fn truncate(name: &str) -> bool {
    // O_TRUNC on char devices is a no-op.
    parse(name).is_some()
}

pub fn read(name: &str, _pos: usize, out: &mut [u8]) -> usize {
    match parse(name) {
        Some(Node::Null) => 0,
        Some(Node::Tty) | Some(Node::Console) => {
            if out.is_empty() {
                0
            } else {
                // May yield; callers must not hold TASKS across this.
                input::read(out)
            }
        }
        None => 0,
    }
}

pub fn write(name: &str, _pos: usize, buf: &[u8]) -> Option<usize> {
    match parse(name) {
        Some(Node::Null) => Some(buf.len()),
        Some(Node::Tty) | Some(Node::Console) => {
            task::print_bytes(buf);
            Some(buf.len())
        }
        None => None,
    }
}

pub fn listdir_at(rel: &str, buf: &mut [u8]) -> usize {
    if !rel.is_empty() && rel != "." {
        return 0;
    }
    const NAMES: &[&[u8]] = &[b"null", b"tty", b"console"];
    let mut n = 0;
    for name in NAMES {
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
        });
    }
    let node = parse(name)?;
    let ino = match node {
        Node::Null => 2,
        Node::Tty => 3,
        Node::Console => 4,
    };
    Some(StatInfo {
        mode: S_IFCHR | 0o666,
        size: 0,
        ino,
        nlink: 1,
    })
}
