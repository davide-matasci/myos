//! devfs: device nodes at `/dev/…` (`null`, `tty`, `console`, `vd*`).

use crate::blk;
use crate::fs::StatInfo;
use crate::input;
use crate::task;

const S_IFDIR: u32 = 0o040000;
const S_IFCHR: u32 = 0o020000;
const S_IFBLK: u32 = 0o060000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Node {
    Null,
    Tty,
    Console,
    Block(u32),
}

fn vd_name(id: u32) -> Option<[u8; 3]> {
    if id >= 26 {
        return None;
    }
    Some([b'v', b'd', b'a' + id as u8])
}

fn parse_vd(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("vd")?;
    if rest.len() != 1 {
        return None;
    }
    let c = rest.as_bytes()[0];
    if !(b'a'..=b'z').contains(&c) {
        return None;
    }
    let id = (c - b'a') as u32;
    if id < blk::count() { Some(id) } else { None }
}

fn parse(name: &str) -> Option<Node> {
    match name {
        "null" => Some(Node::Null),
        "tty" => Some(Node::Tty),
        "console" => Some(Node::Console),
        _ => parse_vd(name).map(Node::Block),
    }
}

/// Block-device id for `/dev/vdX`, if `name` is a probed disk.
pub fn blk_id(name: &str) -> Option<u32> {
    parse_vd(name)
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
    // O_TRUNC on char/block devices is a no-op.
    parse(name).is_some()
}

pub fn read(name: &str, pos: usize, out: &mut [u8]) -> usize {
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
        Some(Node::Block(id)) => blk::read_bytes(id, pos as u64, out).unwrap_or(0),
        None => 0,
    }
}

pub fn write(name: &str, pos: usize, buf: &[u8]) -> Option<usize> {
    match parse(name) {
        Some(Node::Null) => Some(buf.len()),
        Some(Node::Tty) | Some(Node::Console) => {
            task::print_bytes(buf);
            Some(buf.len())
        }
        Some(Node::Block(id)) => blk::write_bytes(id, pos as u64, buf).ok(),
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
    let disks = blk::count();
    for id in 0..disks {
        let Some(name) = vd_name(id) else {
            break;
        };
        let need = name.len() + 1;
        if n + need > buf.len() {
            break;
        }
        buf[n..n + name.len()].copy_from_slice(&name);
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
    match node {
        Node::Null => Some(StatInfo {
            mode: S_IFCHR | 0o666,
            size: 0,
            ino: 2,
            nlink: 1,
        }),
        Node::Tty => Some(StatInfo {
            mode: S_IFCHR | 0o666,
            size: 0,
            ino: 3,
            nlink: 1,
        }),
        Node::Console => Some(StatInfo {
            mode: S_IFCHR | 0o666,
            size: 0,
            ino: 4,
            nlink: 1,
        }),
        Node::Block(id) => {
            let bytes = blk::capacity_bytes(id).unwrap_or(0);
            let size = if bytes > u32::MAX as u64 {
                u32::MAX
            } else {
                bytes as u32
            };
            Some(StatInfo {
                mode: S_IFBLK | 0o666,
                size,
                ino: 10 + id,
                nlink: 1,
            })
        }
    }
}
