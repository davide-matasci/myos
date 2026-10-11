//! evfs: the event counters at `/dev/event/…`, the timers at
//! `/dev/timer/…` and the wait sets at `/dev/wait/…` (`docs/events.md`,
//! [`crate::evfd`]). Reading `clone` allocates an object and answers its
//! number; each live one is a directory `N/` with `data` (the fd a
//! program holds: opened as an event, timer or wait fd in [`crate::task`],
//! never read or written here), `ctl` (its commands, as text) and
//! `status` (what it is doing).

use crate::evfd;
use crate::fs::StatInfo;

const S_IFDIR: u32 = 0o040000;
const S_IFCHR: u32 = 0o020000;
const S_IFREG: u32 = 0o100000;

#[derive(Clone, Copy)]
pub enum Kind {
    Event,
    Timer,
    Wait,
}

impl Kind {
    fn exists(self, id: usize) -> bool {
        match self {
            Kind::Event => evfd::event_exists(id),
            Kind::Timer => evfd::timer_exists(id),
            Kind::Wait => evfd::wait_exists(id),
        }
    }

    fn id_bound(self) -> usize {
        match self {
            Kind::Event => evfd::event_id_bound(),
            Kind::Timer => evfd::timer_id_bound(),
            Kind::Wait => evfd::wait_id_bound(),
        }
    }

    fn alloc(self) -> Option<usize> {
        match self {
            Kind::Event => evfd::event_alloc(),
            Kind::Timer => evfd::timer_alloc(),
            Kind::Wait => evfd::wait_alloc(),
        }
    }

    fn status(self, id: usize) -> Option<alloc::vec::Vec<u8>> {
        match self {
            Kind::Event => evfd::event_status(id),
            Kind::Timer => evfd::timer_status(id),
            Kind::Wait => evfd::wait_status(id),
        }
    }

    fn ctl_write(self, id: usize, text: &[u8]) -> Option<usize> {
        match self {
            Kind::Event => evfd::event_ctl_write(id, text),
            Kind::Timer => evfd::timer_ctl_write(id, text),
            Kind::Wait => evfd::wait_ctl_write(id, text),
        }
    }
}

enum Node {
    Clone,
    /// `N`, the object's directory.
    Dir(usize),
    Data(usize),
    Ctl(usize),
    Status(usize),
}

fn parse(kind: Kind, name: &str) -> Option<Node> {
    if name == "clone" {
        return Some(Node::Clone);
    }
    let (index, member) = match name.split_once('/') {
        Some((index, member)) => (index, Some(member)),
        None => (name, None),
    };
    if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let id: usize = index.parse().ok()?;
    if !kind.exists(id) {
        return None;
    }
    match member {
        None => Some(Node::Dir(id)),
        Some("data") => Some(Node::Data(id)),
        Some("ctl") => Some(Node::Ctl(id)),
        Some("status") => Some(Node::Status(id)),
        Some(_) => None,
    }
}

/// The object `/dev/<kind>/N/data` names, for an `open` of it.
pub fn data_id(kind: Kind, rel: &str) -> Option<usize> {
    match parse(kind, rel)? {
        Node::Data(id) => Some(id),
        _ => None,
    }
}

fn read(kind: Kind, name: &str, pos: usize, out: &mut [u8]) -> usize {
    let text = match parse(kind, name) {
        // One answer per read of `clone` from its start: the new object's
        // number.
        Some(Node::Clone) if pos == 0 => match kind.alloc() {
            Some(id) => alloc::format!("{id}\n").into_bytes(),
            None => return 0,
        },
        Some(Node::Status(id)) => match kind.status(id) {
            Some(text) => text,
            None => return 0,
        },
        _ => return 0,
    };
    let n = out.len().min(text.len().saturating_sub(pos));
    if n != 0 {
        out[..n].copy_from_slice(&text[pos..pos + n]);
    }
    n
}

fn write(kind: Kind, name: &str, buf: &[u8]) -> Option<usize> {
    match parse(kind, name)? {
        Node::Ctl(id) => kind.ctl_write(id, buf),
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

fn listdir_at(kind: Kind, rel: &str, buf: &mut [u8]) -> usize {
    let mut n = 0;
    if rel.is_empty() || rel == "." {
        push_name(buf, &mut n, b"clone");
        for id in (0..kind.id_bound()).filter(|&id| kind.exists(id)) {
            let name = alloc::format!("{id}");
            if !push_name(buf, &mut n, name.as_bytes()) {
                break;
            }
        }
        return n;
    }
    if let Some(Node::Dir(_)) = parse(kind, rel) {
        for name in [b"data".as_slice(), b"ctl".as_slice(), b"status".as_slice()] {
            if !push_name(buf, &mut n, name) {
                break;
            }
        }
    }
    n
}

fn stat(kind: Kind, name: &str) -> Option<StatInfo> {
    let info = |mode: u32, ino: u32, size: u64| StatInfo {
        mode,
        size,
        ino,
        nlink: if mode & S_IFDIR != 0 { 2 } else { 1 },
        dev: 0,
        mtime: 0,
        atime: 0,
    };
    if name.is_empty() || name == "." || name == ".." {
        return Some(info(S_IFDIR | 0o755, 1, 0));
    }
    // Inodes: 2 for clone, then four per object (its directory and members).
    let ino = |id: usize, member: u32| {
        u32::try_from(id).map_or(u32::MAX, |id| 10 + id.saturating_mul(4).saturating_add(member))
    };
    Some(match parse(kind, name)? {
        Node::Clone => info(S_IFREG | 0o444, 2, 0),
        Node::Dir(id) => info(S_IFDIR | 0o755, ino(id, 0), 0),
        Node::Data(id) => info(S_IFCHR | 0o666, ino(id, 1), 0),
        Node::Ctl(id) => info(S_IFREG | 0o644, ino(id, 2), 0),
        Node::Status(id) => {
            let len = kind.status(id).map_or(0, |t| t.len());
            info(S_IFREG | 0o444, ino(id, 3), len as u64)
        }
    })
}

/// The backend functions of one kind, for the mount table.
macro_rules! backend {
    ($name:ident, $kind:expr) => {
        pub mod $name {
            use super::*;

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
                matches!(parse($kind, name), Some(Node::Ctl(_)))
            }

            pub fn read(name: &str, pos: usize, out: &mut [u8]) -> usize {
                super::read($kind, name, pos, out)
            }

            pub fn write(name: &str, _pos: usize, buf: &[u8]) -> Option<usize> {
                super::write($kind, name, buf)
            }

            pub fn listdir_at(rel: &str, buf: &mut [u8]) -> usize {
                super::listdir_at($kind, rel, buf)
            }

            pub fn stat(name: &str) -> Option<StatInfo> {
                super::stat($kind, name)
            }
        }
    };
}

backend!(event, Kind::Event);
backend!(timer, Kind::Timer);
backend!(wait, Kind::Wait);
