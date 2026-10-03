//! procfs: generated nodes at `/proc/…` (`mounts`, `pci`, `cpuinfo`, `acpi/…`)
//! and the calling process's view under `self/`: `fd/N` links to what fd N
//! is open on, `tty` to its controlling terminal's directory (`docs/tty.md`).

use crate::fs::StatInfo;
use crate::fs::vfs;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;

const MAX_DYNAMIC: usize = 16;
const MAX_NAME: usize = 32;

type ProcWriter = unsafe extern "C" fn(*const u8, usize) -> i32;

struct DynNode {
    name: [u8; MAX_NAME],
    name_len: usize,
    data: &'static [u8],
    ino: u32,
    writer: Option<ProcWriter>,
}

static DYN: Mutex<[Option<DynNode>; MAX_DYNAMIC]> = Mutex::new([const { None }; MAX_DYNAMIC]);
static NEXT_INO: AtomicUsize = AtomicUsize::new(11); // 1..=10 are fixed nodes

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

/// A node with a writer takes commands (`echo rescan > /proc/pci`): the
/// shell's `O_TRUNC` open has nothing to cut and succeeds; the others are
/// read-only.
pub fn truncate(name: &str) -> bool {
    let nodes = DYN.lock();
    nodes
        .iter()
        .flatten()
        .any(|n| n.name_len == name.len() && &n.name[..n.name_len] == name.as_bytes() && n.writer.is_some())
}

/// Write handler for dynamic nodes that registered a writer (e.g. `/proc/pci`
/// rescan). After a successful `pci` write the drivers probe for devices that
/// appeared (`module_rescan`, `crate::modules::rescan_all`).
pub fn write(name: &str, _pos: usize, buf: &[u8]) -> Option<usize> {
    let writer = {
        let nodes = DYN.lock();
        let mut found = None;
        for n in nodes.iter().flatten() {
            if n.name_len == name.len() && &n.name[..n.name_len] == name.as_bytes() {
                found = n.writer;
                break;
            }
        }
        found
    }?;
    let rc = unsafe { writer(buf.as_ptr(), buf.len()) };
    if rc < 0 {
        return None;
    }
    if name == "pci" {
        crate::modules::rescan_all();
    }
    Some(rc as usize)
}

/// Attach or clear a write(2) handler for an existing dynamic `/proc/<name>`.
pub fn set_writer(name: &str, writer: Option<ProcWriter>) -> bool {
    let mut nodes = DYN.lock();
    for slot in nodes.iter_mut() {
        if let Some(n) = slot {
            if n.name_len == name.len() && &n.name[..n.name_len] == name.as_bytes() {
                n.writer = writer;
                return true;
            }
        }
    }
    false
}

fn dyn_writable(name: &str) -> bool {
    let nodes = DYN.lock();
    for n in nodes.iter().flatten() {
        if n.name_len == name.len() && &n.name[..n.name_len] == name.as_bytes() {
            return n.writer.is_some();
        }
    }
    false
}

/// Register or replace a generated text node under `/proc/<name>`.
/// `name` may be `pci`, `acpi/tables`, etc. (at most one `/`).
/// Preserves any previously attached writer on replace.
pub fn register_dynamic(name: &str, data: &'static [u8]) -> bool {
    if name.is_empty() || name.len() > MAX_NAME || name == "mounts" || name == "cpuinfo"
        || name == "meminfo" || name == "interrupts" || name == "modules" || name == "self"
        || name.starts_with("self/")
    {
        return false;
    }
    if name.matches('/').count() > 1 {
        return false;
    }
    let mut nodes = DYN.lock();
    for slot in nodes.iter_mut() {
        if let Some(n) = slot {
            if n.name_len == name.len() && &n.name[..n.name_len] == name.as_bytes() {
                n.data = data;
                return true;
            }
        }
    }
    for slot in nodes.iter_mut() {
        if slot.is_none() {
            let mut nb = [0u8; MAX_NAME];
            nb[..name.len()].copy_from_slice(name.as_bytes());
            let ino = NEXT_INO.fetch_add(1, Ordering::SeqCst) as u32;
            *slot = Some(DynNode {
                name: nb,
                name_len: name.len(),
                data,
                ino,
                writer: None,
            });
            return true;
        }
    }
    false
}

fn dyn_get(name: &str) -> Option<(u32, &'static [u8])> {
    let nodes = DYN.lock();
    for n in nodes.iter().flatten() {
        if n.name_len == name.len() && &n.name[..n.name_len] == name.as_bytes() {
            return Some((n.ino, n.data));
        }
    }
    None
}

fn cpuinfo_text() -> alloc::vec::Vec<u8> {
    crate::smp::cpuinfo_text()
}

/// The nodes under `self/`.
enum SelfNode {
    Dir,
    FdDir,
    /// `self/fd/N`, a link to what the fd is open on.
    Fd(usize),
    /// `self/tty`, a link to the controlling terminal's directory.
    Tty,
}

fn parse_self(name: &str) -> Option<SelfNode> {
    match name {
        "self" => Some(SelfNode::Dir),
        "self/fd" => Some(SelfNode::FdDir),
        "self/tty" => Some(SelfNode::Tty),
        _ => {
            let n = name.strip_prefix("self/fd/")?;
            if n.is_empty() || n.len() > 3 || n.starts_with('+') {
                return None;
            }
            Some(SelfNode::Fd(n.parse().ok()?))
        }
    }
}

/// The target of a `self/` link, if the fd is open or the terminal exists.
fn self_link(node: &SelfNode) -> Option<alloc::string::String> {
    match node {
        SelfNode::Fd(fd) => crate::task::fd_path(*fd),
        SelfNode::Tty => crate::tty::ctty_dir(),
        SelfNode::Dir | SelfNode::FdDir => None,
    }
}

/// `readlink` on `self/fd/N` and `self/tty`.
pub fn readlink(name: &str, buf: &mut [u8]) -> Option<usize> {
    let target = self_link(&parse_self(name)?)?;
    let n = target.len().min(buf.len());
    buf[..n].copy_from_slice(&target.as_bytes()[..n]);
    Some(n)
}

fn list_self(name: &str, buf: &mut [u8]) -> usize {
    let mut off = 0usize;
    let mut push = |entry: &[u8]| {
        if off + entry.len() + 1 > buf.len() {
            return false;
        }
        buf[off..off + entry.len()].copy_from_slice(entry);
        off += entry.len();
        buf[off] = b'\n';
        off += 1;
        true
    };
    match parse_self(name) {
        Some(SelfNode::Dir) => {
            push(b"fd");
            if crate::tty::ctty_dir().is_some() {
                push(b"tty");
            }
        }
        Some(SelfNode::FdDir) => {
            for fd in 0..crate::task::MAX_FDS {
                if crate::task::fd_path(fd).is_some() && !push(alloc::format!("{fd}").as_bytes()) {
                    break;
                }
            }
        }
        _ => {}
    }
    off
}

pub fn read(name: &str, pos: usize, out: &mut [u8]) -> usize {
    if name == "mounts" {
        return copy_at(&vfs::mounts_text(), pos, out);
    }
    if name == "cpuinfo" {
        return copy_at(&cpuinfo_text(), pos, out);
    }
    if name == "meminfo" {
        return copy_at(&crate::mm::meminfo_text(), pos, out);
    }
    if name == "interrupts" {
        return copy_at(&crate::irq::interrupts_text(), pos, out);
    }
    if name == "modules" {
        return copy_at(&crate::modules::modules_text(), pos, out);
    }
    if let Some((_, data)) = dyn_get(name) {
        return copy_at(data, pos, out);
    }
    if let Some(data) = stub_acpi(name) {
        return copy_at(data, pos, out);
    }
    0
}

fn list_root(buf: &mut [u8]) -> usize {
    // Dynamic nodes all live under `acpi/` (see `list_acpi`).
    const FIXED: &[&[u8]] =
        &[b"mounts", b"cpuinfo", b"meminfo", b"interrupts", b"modules", b"pci", b"acpi", b"self"];
    let mut off = 0usize;
    for name in FIXED {
        if off + name.len() + 1 > buf.len() {
            break;
        }
        buf[off..off + name.len()].copy_from_slice(name);
        off += name.len();
        buf[off] = b'\n';
        off += 1;
    }
    off
}

fn list_acpi(buf: &mut [u8]) -> usize {
    let mut off = 0usize;
    let nodes = DYN.lock();
    for n in nodes.iter().flatten() {
        let s = core::str::from_utf8(&n.name[..n.name_len]).unwrap_or("");
        let Some(rest) = s.strip_prefix("acpi/") else {
            continue;
        };
        if rest.is_empty() || rest.contains('/') {
            continue;
        }
        let bytes = rest.as_bytes();
        if off + bytes.len() + 1 > buf.len() {
            break;
        }
        buf[off..off + bytes.len()].copy_from_slice(bytes);
        off += bytes.len();
        buf[off] = b'\n';
        off += 1;
    }
    for always in [b"tables".as_slice(), b"info".as_slice(), b"s5".as_slice()] {
        let mut present = false;
        let mut scan = 0usize;
        while scan < off {
            let end = buf[scan..off]
                .iter()
                .position(|&b| b == b'\n')
                .map(|i| scan + i)
                .unwrap_or(off);
            if buf[scan..end] == *always {
                present = true;
                break;
            }
            scan = end + 1;
        }
        if present {
            continue;
        }
        if off + always.len() + 1 > buf.len() {
            break;
        }
        buf[off..off + always.len()].copy_from_slice(always);
        off += always.len();
        buf[off] = b'\n';
        off += 1;
    }
    off
}

pub fn listdir_at(rel: &str, buf: &mut [u8]) -> usize {
    if rel.is_empty() || rel == "." {
        return list_root(buf);
    }
    if rel == "acpi" {
        return list_acpi(buf);
    }
    if parse_self(rel).is_some() {
        return list_self(rel, buf);
    }
    0
}

fn stub_acpi(name: &str) -> Option<&'static [u8]> {
    match name {
        "acpi/info" => Some(
            b"source: none\nnote: ACPI module not loaded or firmware has no RSDP\n",
        ),
        "acpi/tables" => Some(b"# no ACPI tables\n"),
        "acpi/s5" => Some(b"slp_typa: -\nslp_typb: -\nnote: AML _S5 not evaluated\n"),
        "pci" => Some(b"# PCI enumeration pending (pci_enum module)\n"),
        _ => None,
    }
}

pub fn stat(name: &str) -> Option<StatInfo> {
    if name.is_empty() || name == "." || name == ".." {
        return Some(StatInfo {
            mode: S_IFDIR | 0o555,
            size: 0,
            ino: 1,
            nlink: 2,
            dev: 0,
        });
    }
    if name == "acpi" {
        return Some(StatInfo {
            mode: S_IFDIR | 0o555,
            size: 0,
            ino: 3,
            nlink: 2,
            dev: 0,
        });
    }
    if let Some(node) = parse_self(name) {
        // The inodes of the links are 100 (`tty`) and 101 + the fd.
        let (mode, ino, size) = match node {
            SelfNode::Dir => (S_IFDIR | 0o555, 13, 0),
            SelfNode::FdDir => (S_IFDIR | 0o555, 14, 0),
            SelfNode::Tty | SelfNode::Fd(_) => {
                let len = self_link(&node)?.len();
                let ino = match node {
                    SelfNode::Fd(fd) => 101 + fd as u32,
                    _ => 100,
                };
                (S_IFLNK | 0o777, ino, u32::try_from(len).unwrap_or(u32::MAX))
            }
        };
        return Some(StatInfo {
            mode,
            size,
            ino,
            nlink: if mode & S_IFDIR != 0 { 2 } else { 1 },
            dev: 0,
        });
    }
    if name == "mounts" {
        let text = vfs::mounts_text();
        return Some(StatInfo {
            mode: S_IFREG | 0o444,
            size: u32::try_from(text.len()).unwrap_or(u32::MAX),
            ino: 2,
            nlink: 1,
            dev: 0,
        });
    }
    if name == "cpuinfo" {
        let text = cpuinfo_text();
        return Some(StatInfo {
            mode: S_IFREG | 0o444,
            size: u32::try_from(text.len()).unwrap_or(u32::MAX),
            ino: 4,
            nlink: 1,
            dev: 0,
        });
    }
    if name == "meminfo" {
        let text = crate::mm::meminfo_text();
        return Some(StatInfo {
            mode: S_IFREG | 0o444,
            size: u32::try_from(text.len()).unwrap_or(u32::MAX),
            ino: 10,
            nlink: 1,
            dev: 0,
        });
    }
    if name == "interrupts" {
        let text = crate::irq::interrupts_text();
        return Some(StatInfo {
            mode: S_IFREG | 0o444,
            size: u32::try_from(text.len()).unwrap_or(u32::MAX),
            ino: 11,
            nlink: 1,
            dev: 0,
        });
    }
    if name == "modules" {
        let text = crate::modules::modules_text();
        return Some(StatInfo {
            mode: S_IFREG | 0o444,
            size: u32::try_from(text.len()).unwrap_or(u32::MAX),
            ino: 12,
            nlink: 1,
            dev: 0,
        });
    }
    if let Some((ino, data)) = dyn_get(name) {
        let mode = if dyn_writable(name) {
            S_IFREG | 0o644
        } else {
            S_IFREG | 0o444
        };
        return Some(StatInfo {
            mode,
            size: u32::try_from(data.len()).unwrap_or(u32::MAX),
            ino,
            nlink: 1,
            dev: 0,
        });
    }
    if let Some(data) = stub_acpi(name) {
        let ino = match name {
            "pci" => 5,
            "acpi/info" => 6,
            "acpi/tables" => 7,
            "acpi/s5" => 8,
            _ => 9,
        };
        return Some(StatInfo {
            mode: S_IFREG | 0o444,
            size: u32::try_from(data.len()).unwrap_or(u32::MAX),
            ino,
            nlink: 1,
            dev: 0,
        });
    }
    None
}

fn copy_at(data: &[u8], pos: usize, out: &mut [u8]) -> usize {
    let n = out.len().min(data.len().saturating_sub(pos));
    if n != 0 {
        out[..n].copy_from_slice(&data[pos..pos + n]);
    }
    n
}
