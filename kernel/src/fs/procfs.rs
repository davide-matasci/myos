//! procfs: generated nodes at `/proc/…` (`mounts`, `pci`, `cpuinfo`, `acpi/…`).

use crate::fs::StatInfo;
use crate::fs::vfs;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;

const MAX_DYNAMIC: usize = 16;
const MAX_NAME: usize = 32;

struct DynNode {
    name: [u8; MAX_NAME],
    name_len: usize,
    data: &'static [u8],
    ino: u32,
}

static DYN: Mutex<[Option<DynNode>; MAX_DYNAMIC]> = Mutex::new([const { None }; MAX_DYNAMIC]);
static NEXT_INO: AtomicUsize = AtomicUsize::new(10);

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

/// Register or replace a generated text node under `/proc/<name>`.
/// `name` may be `pci`, `acpi/tables`, etc. (at most one `/`).
pub fn register_dynamic(name: &str, data: &'static [u8]) -> bool {
    if name.is_empty() || name.len() > MAX_NAME || name == "mounts" || name == "cpuinfo" {
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

pub fn read(name: &str, pos: usize, out: &mut [u8]) -> usize {
    if name == "mounts" {
        return copy_at(&vfs::mounts_text(), pos, out);
    }
    if name == "cpuinfo" {
        return copy_at(&cpuinfo_text(), pos, out);
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
    let mut names: alloc::vec::Vec<&str> = alloc::vec!["mounts", "cpuinfo", "pci", "acpi"];
    // Also surface any other top-level dynamic names (no slash).
    {
        let nodes = DYN.lock();
        for n in nodes.iter().flatten() {
            let s = core::str::from_utf8(&n.name[..n.name_len]).unwrap_or("");
            if !s.contains('/') && !names.contains(&s) {
                // Leak a stable name is awkward; only emit known fixed names
                // plus pci/acpi which modules own. Skip unknown top-level
                // extras from the listing to keep this simple — they remain
                // openable by path.
            }
            let _ = s;
        }
    }
    let _ = names;
    const FIXED: &[&[u8]] = &[b"mounts", b"cpuinfo", b"pci", b"acpi"];
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
    // Always advertise stub names even before the module loads.
    for always in [b"tables".as_slice(), b"info".as_slice(), b"s5".as_slice()] {
        // Avoid duplicates if already listed.
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
    if let Some((ino, data)) = dyn_get(name) {
        return Some(StatInfo {
            mode: S_IFREG | 0o444,
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
