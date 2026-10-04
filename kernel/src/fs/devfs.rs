//! devfs: device nodes at `/dev/…` (`null`, `tty`, `console/`, `vd*`, `nvme*n1`
//! and the modules' character devices).
//!
//! `/dev/console` is the hardware console (serial+fb), a directory like
//! every terminal (`docs/tty.md`): `data` is the terminal, `ctl` its control
//! file (termios, window size, `ctty`, `flush`, `keymap` as text). `/dev/tty`
//! is the process controlling terminal: open is gated in [`crate::fs::open`]
//! and aliases to the console's `data` when a ctty is set.
//!
//! A module's character device (`KernelApi::dev_register`) is a directory
//! too: `/dev/<name>/data` is the device, `/dev/<name>/ctl` its control file
//! when the module gives it one ([`myos_abi::ModuleChrOps`]).

use crate::blk;
use crate::fs::StatInfo;
use crate::input;
use crate::task;

const S_IFDIR: u32 = 0o040000;
const S_IFCHR: u32 = 0o020000;
const S_IFBLK: u32 = 0o060000;
const S_IFREG: u32 = 0o100000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Node {
    Null,
    Zero,
    Tty,
    /// `console`, the directory.
    ConsoleDir,
    /// `console/data`, the hardware console.
    Console,
    /// `console/ctl`, its control file.
    ConsoleCtl,
    Urandom,
    Block(u32),
    /// `<name>`, a module device's directory.
    ChrDir(usize),
    /// `<name>/data`, the device.
    ChrData(usize),
    /// `<name>/ctl`, its control file.
    ChrCtl(usize),
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

/// Register a module character device: the directory `/dev/<name>/` with
/// `data` and, when `ops` has a control file, `ctl`.
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

fn chr(i: usize) -> Option<ChrDev> {
    chr_table().get(i).and_then(|s| *s)
}

/// The module gave the device a control file.
fn chr_has_ctl(i: usize) -> bool {
    chr(i).is_some_and(|c| c.ops.ctl_read.is_some() || c.ops.ctl_write.is_some())
}

/// The text of a module device's `ctl`, from the module.
fn chr_ctl_text(i: usize) -> alloc::vec::Vec<u8> {
    let Some(f) = chr(i).and_then(|c| c.ops.ctl_read) else {
        return alloc::vec::Vec::new();
    };
    let mut buf = alloc::vec![0u8; 1024];
    let n = unsafe { f(buf.as_mut_ptr(), buf.len()) };
    let n = usize::try_from(n).unwrap_or(0).min(buf.len());
    buf.truncate(n);
    buf
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
        "console" => Some(Node::ConsoleDir),
        "console/data" => Some(Node::Console),
        "console/ctl" => Some(Node::ConsoleCtl),
        "urandom" | "random" => Some(Node::Urandom),
        _ => {
            if let Some((dir, member)) = name.split_once('/') {
                let i = parse_chr(dir)?;
                return match member {
                    "data" => Some(Node::ChrData(i)),
                    "ctl" if chr_has_ctl(i) => Some(Node::ChrCtl(i)),
                    _ => None,
                };
            }
            parse_chr(name)
                .map(Node::ChrDir)
                .or_else(|| parse_blk(name).map(Node::Block))
        }
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
    // O_TRUNC on char/block devices is a no-op; a control file takes the
    // shell's truncating open (`echo … > ctl`) the same way.
    parse(name).is_some_and(|n| !matches!(n, Node::ConsoleDir | Node::ChrDir(_)))
}

/// The text of `/dev/console/ctl`: the console termios, the screen's size
/// and the keyboard map (`keymap PATH`, or `keymap none`).
pub fn console_ctl_text() -> alloc::vec::Vec<u8> {
    let (rows, cols) = crate::console::winsize();
    let mut text = crate::tty::ctl_text(&input::termios(), rows, cols);
    text.extend_from_slice(b"keymap ");
    match crate::console::keymap_path() {
        Some(path) => text.extend_from_slice(path.as_bytes()),
        None => text.extend_from_slice(b"none"),
    }
    text.push(b'\n');
    text
}

/// A write to `/dev/console/ctl`. A `winsize` line is accepted and ignored:
/// the console is the size of the screen. The console has no output buffer,
/// so `flush out` has nothing to do. `keymap PATH` loads the keyboard map in
/// that file (`docs/keymap.md`).
pub fn console_ctl_write(text: &[u8]) -> Option<usize> {
    use crate::tty::CtlAction;
    let actions = crate::tty::ctl_parse(&input::termios(), text)?;
    // The keymap load is the one step that can fail, so it goes first and
    // the write stays all or nothing.
    for action in &actions {
        if let CtlAction::Keymap(path) = action {
            if !crate::console::keymap_load_file(path) {
                return None;
            }
        }
    }
    for action in actions {
        match action {
            CtlAction::Termios(t) => input::set_termios(t),
            CtlAction::Winsize(..) => {}
            CtlAction::Ctty => task::set_ctty(),
            CtlAction::Flush { input: true, .. } => input::flush_input(),
            CtlAction::Flush { .. } | CtlAction::Keymap(_) => {}
        }
    }
    Some(text.len())
}

fn copy_at(data: &[u8], pos: usize, out: &mut [u8]) -> usize {
    let n = out.len().min(data.len().saturating_sub(pos));
    if n != 0 {
        out[..n].copy_from_slice(&data[pos..pos + n]);
    }
    n
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
        Some(Node::ConsoleDir) => 0,
        Some(Node::ConsoleCtl) => copy_at(&console_ctl_text(), pos, out),
        // /dev/urandom and /dev/random share the kernel CSPRNG pool (phase-1
        // has no blocking distinction; pos is ignored — it is a stream).
        Some(Node::Urandom) => {
            let _ = pos;
            crate::rng::fill(out);
            out.len()
        }
        Some(Node::Block(id)) => blk::read_bytes(id, pos as u64, out).unwrap_or(0),
        Some(Node::ChrDir(_)) => 0,
        Some(Node::ChrData(i)) => match chr(i) {
            Some(c) => {
                let n = unsafe { (c.ops.read)(out.as_mut_ptr(), out.len()) };
                if n < 0 { 0 } else { n as usize }
            }
            None => 0,
        },
        Some(Node::ChrCtl(i)) => copy_at(&chr_ctl_text(i), pos, out),
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
        Some(Node::ConsoleDir) => None,
        Some(Node::ConsoleCtl) => console_ctl_write(buf),
        // Writes to the RNG pool are ignored (no RNDADDENTROPY ioctl yet).
        Some(Node::Urandom) => Some(buf.len()),
        Some(Node::Block(id)) => blk::write_bytes(id, pos as u64, buf).ok(),
        Some(Node::ChrDir(_)) => None,
        Some(Node::ChrData(i)) => {
            let n = unsafe { (chr(i)?.ops.write)(buf.as_ptr(), buf.len()) };
            if n < 0 { None } else { Some(n as usize) }
        }
        Some(Node::ChrCtl(i)) => {
            let n = unsafe { (chr(i)?.ops.ctl_write?)(buf.as_ptr(), buf.len()) };
            if n < 0 { None } else { Some(n as usize) }
        }
        None => None,
    }
}

pub fn listdir_at(rel: &str, buf: &mut [u8]) -> usize {
    const NAMES: &[&[u8]] = &[b"null", b"zero", b"tty", b"console", b"urandom", b"random"];
    const DATA_CTL: &[&[u8]] = &[b"data", b"ctl"];
    const DATA: &[&[u8]] = &[b"data"];
    let names = match rel {
        "" | "." => NAMES,
        "console" => DATA_CTL,
        _ => match parse_chr(rel) {
            Some(i) if chr_has_ctl(i) => DATA_CTL,
            Some(_) => DATA,
            None => return 0,
        },
    };
    let mut n = 0;
    for name in names {
        let need = name.len() + 1;
        if n + need > buf.len() {
            break;
        }
        buf[n..n + name.len()].copy_from_slice(name);
        n += name.len();
        buf[n] = b'\n';
        n += 1;
    }
    if rel != "" && rel != "." {
        return n;
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
            mtime: 0,
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
            mtime: 0,
        }),
        Node::Tty => Some(StatInfo {
            mode: S_IFCHR | 0o666,
            size: 0,
            ino: 3,
            nlink: 1,
            dev: 0,
            mtime: 0,
        }),
        Node::ConsoleDir => Some(StatInfo {
            mode: S_IFDIR | 0o755,
            size: 0,
            ino: 8,
            nlink: 2,
            dev: 0,
            mtime: 0,
        }),
        Node::Console => Some(StatInfo {
            mode: S_IFCHR | 0o666,
            size: 0,
            ino: 4,
            nlink: 1,
            dev: 0,
            mtime: 0,
        }),
        Node::ConsoleCtl => Some(StatInfo {
            mode: S_IFREG | 0o644,
            size: u32::try_from(console_ctl_text().len()).unwrap_or(u32::MAX),
            ino: 9,
            nlink: 1,
            dev: 0,
            mtime: 0,
        }),
        Node::Urandom => Some(StatInfo {
            mode: S_IFCHR | 0o666,
            size: 0,
            ino: 6,
            nlink: 1,
            dev: 0,
            mtime: 0,
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
                mtime: 0,
            })
        }
        Node::ChrDir(i) => Some(StatInfo {
            mode: S_IFDIR | 0o755,
            size: 0,
            ino: 30 + i as u32,
            nlink: 2,
            dev: 0,
            mtime: 0,
        }),
        Node::ChrData(i) => Some(StatInfo {
            mode: S_IFCHR | 0o666,
            size: 0,
            ino: 40 + i as u32,
            nlink: 1,
            dev: 0,
            mtime: 0,
        }),
        Node::ChrCtl(i) => Some(StatInfo {
            mode: S_IFREG | 0o644,
            size: u32::try_from(chr_ctl_text(i).len()).unwrap_or(u32::MAX),
            ino: 50 + i as u32,
            nlink: 1,
            dev: 0,
            mtime: 0,
        }),
    }
}

/// `poll` readiness of a device ([`crate::fs::poll`]): a module device's
/// `data` asks its module; everything else is always ready (`None`).
pub fn poll(name: &str) -> Option<u32> {
    match parse(name)? {
        Node::ChrData(i) => {
            let f = chr(i)?.ops.poll?;
            Some(unsafe { f() })
        }
        _ => None,
    }
}
