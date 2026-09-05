//! devfs: device nodes at `/dev/…` (`null`, `tty`, `console`, `vd*`, `nvme*n1`).

use crate::blk;
use crate::fs::{IoctlResult, StatInfo};
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
    Nvme(u32),
    Chr(usize),
}

const MAX_CHR: usize = 4;
const CHR_NAME_MAX: usize = 16;

#[derive(Clone, Copy)]
struct ChrDev {
    name: [u8; CHR_NAME_MAX],
    name_len: u8,
    ops: myos_abi::ModuleChrOps,
}

static mut CHR: [Option<ChrDev>; MAX_CHR] = [None; MAX_CHR];

fn chr_table() -> &'static mut [Option<ChrDev>; MAX_CHR] {
    unsafe { &mut *core::ptr::addr_of_mut!(CHR) }
}

/// Register a module character device as `/dev/<name>`.
pub fn register_chrdev(name: &str, ops: myos_abi::ModuleChrOps) -> bool {
    if name.is_empty() || name.len() > CHR_NAME_MAX || name.contains('/') {
        return false;
    }
    let table = chr_table();
    for slot in table.iter() {
        if let Some(c) = slot {
            if &c.name[..c.name_len as usize] == name.as_bytes() {
                return false;
            }
        }
    }
    for slot in table.iter_mut() {
        if slot.is_none() {
            let mut n = [0u8; CHR_NAME_MAX];
            n[..name.len()].copy_from_slice(name.as_bytes());
            *slot = Some(ChrDev {
                name: n,
                name_len: name.len() as u8,
                ops,
            });
            return true;
        }
    }
    false
}

fn parse_chr(name: &str) -> Option<usize> {
    for (i, slot) in chr_table().iter().enumerate() {
        if let Some(c) = slot {
            if &c.name[..c.name_len as usize] == name.as_bytes() {
                return Some(i);
            }
        }
    }
    None
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

fn parse_nvme(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("nvme")?;
    let (ctrl, ns) = rest.split_once('n')?;
    if ns != "1" || ctrl.is_empty() {
        return None;
    }
    let mut id = 0u32;
    for b in ctrl.bytes() {
        if !b.is_ascii_digit() {
            return None;
        }
        id = id.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    if id < crate::nvme::count() {
        Some(id)
    } else {
        None
    }
}

fn parse(name: &str) -> Option<Node> {
    match name {
        "null" => Some(Node::Null),
        "tty" => Some(Node::Tty),
        "console" => Some(Node::Console),
        _ => parse_chr(name)
            .map(Node::Chr)
            .or_else(|| parse_vd(name).map(Node::Block))
            .or_else(|| parse_nvme(name).map(Node::Nvme)),
    }
}

/// Block-device id for `/dev/vdX` or `/dev/nvmeXn1`.
pub fn blk_id(name: &str) -> Option<u32> {
    if let Some(id) = parse_vd(name) {
        return Some(id);
    }
    parse_nvme(name).map(|c| crate::blk::NVME_ID_BASE + c)
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
        Some(Node::Nvme(ctrl)) => {
            blk::read_bytes(blk::NVME_ID_BASE + ctrl, pos as u64, out).unwrap_or(0)
        }
        Some(Node::Chr(i)) => {
            let _ = pos;
            match chr_table().get(i).and_then(|s| *s) {
                Some(c) => {
                    let n = unsafe { (c.ops.read)(out.as_mut_ptr(), out.len()) };
                    if n < 0 { 0 } else { n as usize }
                }
                None => 0,
            }
        }
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
        Some(Node::Nvme(ctrl)) => blk::write_bytes(blk::NVME_ID_BASE + ctrl, pos as u64, buf).ok(),
        Some(Node::Chr(i)) => {
            let _ = pos;
            match chr_table().get(i).and_then(|s| *s) {
                Some(c) => {
                    let n = unsafe { (c.ops.write)(buf.as_ptr(), buf.len()) };
                    if n < 0 { None } else { Some(n as usize) }
                }
                None => None,
            }
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
    let nvme = crate::nvme::count();
    for id in 0..nvme {
        // nvme{id}n1, id < 10 (MAX_CTRL is 4)
        let mut name = [0u8; 7];
        name[..4].copy_from_slice(b"nvme");
        name[4] = b'0' + id as u8;
        name[5] = b'n';
        name[6] = b'1';
        let need = name.len() + 1;
        if n + need > buf.len() {
            break;
        }
        buf[n..n + name.len()].copy_from_slice(&name);
        n += name.len();
        buf[n] = b'\n';
        n += 1;
    }
    for slot in chr_table().iter() {
        let Some(c) = slot else {
            continue;
        };
        let name = &c.name[..c.name_len as usize];
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
        Node::Nvme(ctrl) => {
            let bytes = blk::nvme_capacity_bytes(ctrl).unwrap_or(0);
            let size = if bytes > u32::MAX as u64 {
                u32::MAX
            } else {
                bytes as u32
            };
            Some(StatInfo {
                mode: S_IFBLK | 0o666,
                size,
                ino: 20 + ctrl,
                nlink: 1,
            })
        }
        Node::Chr(i) => Some(StatInfo {
            mode: S_IFCHR | 0o666,
            size: 0,
            ino: 30 + i as u32,
            nlink: 1,
        }),
    }
}

/// Linux-compatible tty ioctls shared by Stdin/Console and `/dev/tty`/`console`.
pub fn tty_ioctl(request: usize) -> IoctlResult {
    const TCGETS: usize = 0x5401;
    const TCSETS: usize = 0x5402;
    const TCFLSH: usize = 0x540B;
    const TIOCSCTTY: usize = 0x540E;
    const TIOCGWINSZ: usize = 0x5413;
    const TIOCSWINSZ: usize = 0x5414;

    match request {
        TCGETS | TCSETS | TCFLSH | TIOCSCTTY | TIOCSWINSZ => IoctlResult::Ok,
        TIOCGWINSZ => IoctlResult::Winsize { row: 24, col: 80 },
        _ => IoctlResult::Notty,
    }
}

/// MountOps ioctl callback for `/dev/*`.
///
/// Module chrdevs may register [`myos_abi::ModuleChrOps::ioctl`]; `None` → ENOTTY.
/// Modules must not deref userspace `arg` — use `KernelApi::copy_to_user`.
pub fn ioctl(name: &str, request: usize, arg: usize) -> IoctlResult {
    match parse(name) {
        Some(Node::Tty) | Some(Node::Console) => tty_ioctl(request),
        Some(Node::Chr(i)) => {
            match chr_table().get(i).and_then(|s| *s) {
                Some(c) => match c.ops.ioctl {
                    Some(f) => {
                        let rc = unsafe { f(request as u64, arg) };
                        if rc < 0 {
                            IoctlResult::Bad
                        } else {
                            IoctlResult::Ok
                        }
                    }
                    None => IoctlResult::Notty,
                },
                None => IoctlResult::Notty,
            }
        }
        Some(Node::Null) | Some(Node::Block(_)) | Some(Node::Nvme(_)) | None => IoctlResult::Notty,
    }
}
