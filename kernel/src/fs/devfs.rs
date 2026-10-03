//! devfs: device nodes at `/dev/…` (`null`, `tty`, `console`, `fb0`, `vd*`, `nvme*n1`).
//!
//! `/dev/console` is the hardware console (serial+fb). `/dev/tty` is the
//! process controlling terminal: open is gated in [`crate::fs::open`] and
//! aliases to console when a ctty is set. `/dev/fb0` is the boot
//! framebuffer ([`crate::fb`]), present when Limine gave us one.

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
    Zero,
    Tty,
    Console,
    Ptmx,
    Urandom,
    Fb,
    Block(u32),
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

/// `/dev/<name>` of a registered block device (`vda`, `nvme0n1`, …).
fn parse_blk(name: &str) -> Option<u32> {
    blk::by_name(name)
}

fn parse(name: &str) -> Option<Node> {
    match name {
        "null" => Some(Node::Null),
        "zero" => Some(Node::Zero),
        "tty" => Some(Node::Tty),
        "console" => Some(Node::Console),
        "ptmx" => Some(Node::Ptmx),
        "urandom" | "random" => Some(Node::Urandom),
        "fb0" if crate::fb::present() => Some(Node::Fb),
        _ => parse_chr(name)
            .map(Node::Chr)
            .or_else(|| parse_blk(name).map(Node::Block)),
    }
}

/// Block-device id for `/dev/<name>` (`vdX`, `nvmeXn1`, …).
pub fn blk_id(name: &str) -> Option<u32> {
    parse_blk(name)
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
        Some(Node::Zero) => {
            out.fill(0);
            out.len()
        }
        Some(Node::Tty) | Some(Node::Console) => {
            if out.is_empty() {
                0
            } else {
                // May yield; callers must not hold TASKS across this.
                input::read(out)
            }
        }
        // ptmx I/O routes through FdEntry::PtyMaster in crate::task; a plain
        // VFS read on the node itself has no peer session — report EIO-ish 0.
        Some(Node::Ptmx) => 0,
        // /dev/urandom and /dev/random share the kernel CSPRNG pool (phase-1
        // has no blocking distinction; pos is ignored — it is a stream).
        Some(Node::Urandom) => {
            let _ = pos;
            crate::rng::fill(out);
            out.len()
        }
        Some(Node::Fb) => crate::fb::read(pos, out),
        Some(Node::Block(id)) => blk::read_bytes(id, pos as u64, out).unwrap_or(0),
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
        Some(Node::Zero) => Some(buf.len()),
        Some(Node::Tty) | Some(Node::Console) => {
            task::print_bytes(buf);
            Some(buf.len())
        }
        Some(Node::Ptmx) => None,
        // Writes to the RNG pool are ignored (no RNDADDENTROPY ioctl yet).
        Some(Node::Urandom) => Some(buf.len()),
        Some(Node::Fb) => crate::fb::write(pos, buf),
        Some(Node::Block(id)) => blk::write_bytes(id, pos as u64, buf).ok(),
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
    const NAMES: &[&[u8]] = &[b"null", b"zero", b"tty", b"console", b"ptmx", b"urandom", b"random"];
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
    if crate::fb::present() && n + 4 <= buf.len() {
        buf[n..n + 4].copy_from_slice(b"fb0\n");
        n += 4;
    }
    for id in 0..blk::count() {
        let mut name = [0u8; 16];
        let Some(len) = blk::name(id, &mut name) else {
            continue;
        };
        let need = len + 1;
        if n + need > buf.len() {
            break;
        }
        buf[n..n + len].copy_from_slice(&name[..len]);
        n += len;
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
            dev: 0,
        });
    }
    let node = parse(name)?;
    match node {
        Node::Null | Node::Zero => Some(StatInfo {
            mode: S_IFCHR | 0o666,
            size: 0,
            ino: if matches!(node, Node::Zero) { 7 } else { 2 },
            nlink: 1,
            dev: 0,
        }),
        Node::Tty => Some(StatInfo {
            mode: S_IFCHR | 0o666,
            size: 0,
            ino: 3,
            nlink: 1,
            dev: 0,
        }),
        Node::Console => Some(StatInfo {
            mode: S_IFCHR | 0o666,
            size: 0,
            ino: 4,
            nlink: 1,
            dev: 0,
        }),
        Node::Urandom => Some(StatInfo {
            mode: S_IFCHR | 0o666,
            size: 0,
            ino: 6,
            nlink: 1,
            dev: 0,
        }),
        Node::Ptmx => Some(StatInfo {
            mode: S_IFCHR | 0o666,
            size: 0,
            ino: 5,
            nlink: 1,
            dev: 0,
        }),
        Node::Fb => Some(StatInfo {
            mode: S_IFCHR | 0o666,
            size: crate::fb::len() as u32,
            ino: 8,
            nlink: 1,
            dev: 0,
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
                dev: 0,
            })
        }
        Node::Chr(i) => Some(StatInfo {
            mode: S_IFCHR | 0o666,
            size: 0,
            ino: 30 + i as u32,
            nlink: 1,
            dev: 0,
        }),
    }
}

/// Linux-compatible tty ioctls for the hardware console (`/dev/console`).
///
/// `TIOCSCTTY` is handled in [`crate::task::fd_ioctl`] (sets `Task.has_ctty`).
/// `/dev/tty` open is gated there too; when allowed it aliases to console.
pub fn tty_ioctl(request: usize) -> IoctlResult {
    const TCGETS: usize = 0x5401;
    const TCSETS: usize = 0x5402;
    const TCFLSH: usize = 0x540B;
    const TIOCGWINSZ: usize = 0x5413;
    const TIOCSWINSZ: usize = 0x5414;

    match request {
        // TCGETS/TCSETS and KDSKMAP/KDGKMAP are handled in `task::fd_ioctl`.
        TCGETS | TCSETS | TCFLSH | TIOCSWINSZ => IoctlResult::Ok,
        x if x == crate::console::KDSKMAP || x == crate::console::KDGKMAP => IoctlResult::Ok,
        TIOCGWINSZ => {
            let (row, col) = crate::console::winsize();
            IoctlResult::Winsize { row, col }
        }
        _ => IoctlResult::Notty,
    }
}

/// MountOps ioctl callback for `/dev/*`.
///
/// Module chrdevs may register [`myos_abi::ModuleChrOps::ioctl`]; `None` → ENOTTY.
/// Modules must not deref userspace `arg` — use `KernelApi::copy_to_user`.
pub fn ioctl(name: &str, request: usize, arg: usize) -> IoctlResult {
    match parse(name) {
        // `/dev/tty` open aliases to console; keep both for leftover/stat paths.
        Some(Node::Tty) | Some(Node::Console) => tty_ioctl(request),
        // pty pair ioctls are handled per-fd in crate::task (they need
        // userspace copies); the bare node has no pair attached.
        Some(Node::Ptmx) => IoctlResult::Notty,
        Some(Node::Urandom) => IoctlResult::Notty,
        // fbdev and KDSETMODE ioctls copy structs: `task::fd_ioctl` routes
        // them to `crate::fb` before this.
        Some(Node::Fb) => IoctlResult::Notty,
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
        Some(Node::Null) | Some(Node::Zero) | Some(Node::Block(_)) | None => IoctlResult::Notty,
    }
}
