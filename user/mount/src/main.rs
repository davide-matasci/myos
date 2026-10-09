#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![no_main]

//! `mount` lists the mounts; `mount SOURCE TARGET FSTYPE` mounts a block
//! device (or `PARTUUID=<guid>`, the partition of that unique GUID in
//! `/proc/partitions`), or binds a path (`bind`). `mount -a`, which init
//! runs at boot (docs/install.md), mounts the boot disk's ESP at `/boot`
//! (the partition `/proc/boot/partuuid` names), binds `/tmp/mnt` over the
//! read-only `/mnt` so mount points can be made there, then mounts what
//! `/boot/fstab` lists that is not mounted yet.

use myos_user::{close, exit, mkdir, mount, open, read, stat_mode, write, write_fd};

myos_user::x86_start!(main);

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: usize, argv: *const usize) -> ! {
    unsafe { myos_user::args::init_from_regs(argc, argv) };
    main()
}

fn main() -> ! {
    match myos_user::argc() {
        1 => print_mounts(),
        2 if myos_user::arg(1) == Some(b"-a") => mount_all(),
        4 => do_mount(),
        _ => {
            write(b"usage: mount [-a | SOURCE TARGET FSTYPE]\n");
            myos_user::exit_code(1);
        }
    }
}

fn print_mounts() -> ! {
    let Some(fd) = open(b"/proc/mounts") else {
        write(b"mount: open /proc/mounts failed\n");
        myos_user::exit_code(1);
    };
    let mut buf = [0u8; 256];
    loop {
        let n = read(fd, &mut buf);
        if n == usize::MAX {
            write(b"mount: read /proc/mounts failed\n");
            close(fd);
            myos_user::exit_code(1);
        }
        if n == 0 {
            break;
        }
        write_fd(1, &buf[..n]);
    }
    close(fd);
    exit();
}

fn do_mount() -> ! {
    let mut dev = [0u8; 80];
    let mut src = myos_user::arg(1).unwrap_or(b"");
    let tgt = myos_user::arg(2).unwrap_or(b"");
    let fs = myos_user::arg(3).unwrap_or(b"");
    if let Some(guid) = src.strip_prefix(b"PARTUUID=") {
        let Some(n) = partuuid_dev(guid, &mut dev) else {
            fail(&[b"no partition has PARTUUID=", guid, b" (/proc/partitions)"]);
        };
        src = &dev[..n];
    }
    if mount(src, tgt, fs) {
        exit();
    }
    // The kernel says only that it failed: find out why.
    let mut mounts = [0u8; 4096];
    let mounts = read_mounts(&mut mounts);
    if fs == b"bind" {
        fail(&[b"cannot bind ", src, b" on ", tgt]);
    }
    match stat_mode(src) {
        None => fail(&[src, b": no such device"]),
        Some(mode) if mode & S_IFMT != S_IFBLK => fail(&[src, b": not a block device"]),
        _ => {}
    }
    match stat_mode(tgt) {
        None => fail(&[tgt, b": no such directory (create it first)"]),
        Some(mode) if mode & S_IFMT != S_IFDIR => fail(&[tgt, b": not a directory"]),
        _ => {}
    }
    let target = trim_slashes(tgt);
    if mounts.split(|&b| b == b'\n').any(|l| field(l, 1).is_some_and(|t| trim_slashes(t) == target)) {
        fail(&[tgt, b": already a mount point (umount it first)"]);
    }
    if mounts.split(|&b| b == b'\n').any(|l| field(l, 0) == Some(src)) {
        fail(&[src, b": already mounted"]);
    }
    fail(&[src, b": not a ", fs, b" filesystem, or no ", fs, b" driver is loaded"]);
}

/// The ESP this boot came from, at `/boot`; `/mnt` a writable directory
/// (`/tmp/mnt`, bound over it); then each line of `/boot/fstab`:
/// `PARTUUID=<guid> MOUNTPOINT FSTYPE [rw]`, `#` comments and blank lines
/// aside. A partition that is not there is skipped, a line that is wrong or
/// does not mount is reported: neither stops the others. A mount point
/// under `/mnt` is made when missing. Exit 1 when a line failed.
fn mount_all() -> ! {
    let mut mounts_buf = [0u8; 4096];
    let mut dev = [0u8; 80];
    let mut ok = true;
    let mut guid = [0u8; 64];
    if let Some(n) = read_file(b"/proc/boot/partuuid", &mut guid) {
        let guid = trim(&guid[..n]);
        let mounts = read_mounts(&mut mounts_buf);
        if !mounted_at(mounts, b"boot") {
            match partuuid_dev(guid, &mut dev) {
                Some(n) if mount(&dev[..n], b"/boot", b"fat") => {}
                _ => {
                    say(&[b"cannot mount the boot partition PARTUUID=", guid, b" on /boot"]);
                    ok = false;
                }
            }
        }
    }
    mkdir(b"/tmp/mnt");
    if !mount(b"/tmp/mnt", b"/mnt", b"bind") {
        say(&[b"cannot bind /tmp/mnt on /mnt"]);
        ok = false;
    }
    let mut fstab = [0u8; 4096];
    let Some(n) = read_file(b"/boot/fstab", &mut fstab) else {
        exit_ok(ok);
    };
    for (i, line) in fstab[..n].split(|&b| b == b'\n').enumerate() {
        let line = trim(line);
        if line.is_empty() || line[0] == b'#' {
            continue;
        }
        if !fstab_line(line, i + 1, &mut dev) {
            ok = false;
        }
    }
    exit_ok(ok)
}

/// One line of `/boot/fstab` (number `no`): false when it is wrong or its
/// partition does not mount.
fn fstab_line(line: &[u8], no: usize, dev: &mut [u8; 80]) -> bool {
    let mut num = [0u8; 20];
    let no = decimal(no, &mut num);
    let mut fields = line.split(|&b| b == b' ' || b == b'\t').filter(|f| !f.is_empty());
    let (Some(spec), Some(dir), Some(fs)) = (fields.next(), fields.next(), fields.next()) else {
        say(&[b"/boot/fstab:", no, b": not PARTUUID=<guid> MOUNTPOINT FSTYPE [rw]"]);
        return false;
    };
    match (fields.next(), fields.next()) {
        (None, _) | (Some(b"rw" | b"defaults"), None) => {}
        (Some(opt), None) => {
            say(&[b"/boot/fstab:", no, b": unknown option ", opt, b" (only rw)"]);
            return false;
        }
        _ => {
            say(&[b"/boot/fstab:", no, b": too many fields"]);
            return false;
        }
    }
    let Some(guid) = spec.strip_prefix(b"PARTUUID=") else {
        say(&[b"/boot/fstab:", no, b": ", spec, b": a partition is named PARTUUID=<guid> (/proc/partitions)"]);
        return false;
    };
    if dir.first() != Some(&b'/') {
        say(&[b"/boot/fstab:", no, b": ", dir, b": not an absolute path"]);
        return false;
    }
    let Some(n) = partuuid_dev(guid, dev) else {
        say(&[b"/boot/fstab:", no, b": no partition PARTUUID=", guid, b", skipped"]);
        return true;
    };
    let dev = &dev[..n];
    let mut mounts_buf = [0u8; 4096];
    let mounts = read_mounts(&mut mounts_buf);
    if mounts.split(|&b| b == b'\n').any(|l| field(l, 0) == Some(dev)) {
        return true;
    }
    if dir.starts_with(b"/mnt/") && stat_mode(dir).is_none() {
        mkdirs(dir);
    }
    if mount(dev, dir, fs) {
        return true;
    }
    say(&[b"/boot/fstab:", no, b": cannot mount ", dev, b" (", fs, b") on ", dir]);
    false
}

/// `/dev/<partition>` into `out` for the partition whose unique GUID (the
/// fifth field of `/proc/partitions`) is `guid`, in any case; its length.
fn partuuid_dev(guid: &[u8], out: &mut [u8; 80]) -> Option<usize> {
    let mut buf = [0u8; 8192];
    let n = read_file(b"/proc/partitions", &mut buf)?;
    let line = buf[..n].split(|&b| b == b'\n').find(|l| field(l, 4).is_some_and(|g| g.eq_ignore_ascii_case(guid)))?;
    let name = field(line, 0)?;
    if name.len() + 5 > out.len() {
        return None;
    }
    out[..5].copy_from_slice(b"/dev/");
    out[5..5 + name.len()].copy_from_slice(name);
    Some(5 + name.len())
}

/// A mount's target (`/proc/mounts`) is `/<target>`.
fn mounted_at(mounts: &[u8], target: &[u8]) -> bool {
    mounts.split(|&b| b == b'\n').any(|l| field(l, 1).is_some_and(|t| trim_slashes(t) == target))
}

/// Each directory of `path` that is missing, made.
fn mkdirs(path: &[u8]) {
    for (i, &b) in path.iter().enumerate().skip(1) {
        if b == b'/' {
            mkdir(&path[..i]);
        }
    }
    mkdir(path);
}

/// The whole file at `path` into `buf` (what fits); `None` when it cannot be
/// opened.
fn read_file(path: &[u8], buf: &mut [u8]) -> Option<usize> {
    let fd = open(path)?;
    let mut n = 0;
    while n < buf.len() {
        let got = read(fd, &mut buf[n..]);
        if got == 0 || got == usize::MAX {
            break;
        }
        n += got;
    }
    close(fd);
    Some(n)
}

fn trim(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(s.len());
    let end = s.iter().rposition(|b| !b.is_ascii_whitespace()).map_or(start, |e| e + 1);
    &s[start..end]
}

/// `n` in decimal, in `buf`.
fn decimal(mut n: usize, buf: &mut [u8; 20]) -> &[u8] {
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            return &buf[i..];
        }
    }
}

/// `mount: ` and the parts on stderr.
fn say(parts: &[&[u8]]) {
    write_fd(2, b"mount: ");
    for p in parts {
        write_fd(2, p);
    }
    write_fd(2, b"\n");
}

fn exit_ok(ok: bool) -> ! {
    if ok {
        exit();
    }
    myos_user::exit_code(1);
}

const S_IFMT: u32 = 0o170000;
const S_IFBLK: u32 = 0o060000;
const S_IFDIR: u32 = 0o040000;

/// `mount: ` and the parts on stderr, exit 1.
fn fail(parts: &[&[u8]]) -> ! {
    say(parts);
    myos_user::exit_code(1);
}

/// `/proc/mounts` into `buf`: `source target fstype opts 0 0` per line.
fn read_mounts(buf: &mut [u8]) -> &[u8] {
    let Some(fd) = open(b"/proc/mounts") else {
        return &[];
    };
    let mut n = 0;
    while n < buf.len() {
        let got = read(fd, &mut buf[n..]);
        if got == 0 || got == usize::MAX {
            break;
        }
        n += got;
    }
    close(fd);
    &buf[..n]
}

/// The `i`th space-separated field of a `/proc/mounts` line.
fn field(line: &[u8], i: usize) -> Option<&[u8]> {
    line.split(|&b| b == b' ').filter(|f| !f.is_empty()).nth(i)
}

fn trim_slashes(path: &[u8]) -> &[u8] {
    let start = path.iter().position(|&b| b != b'/').unwrap_or(path.len());
    let end = path.iter().rposition(|&b| b != b'/').map_or(start, |e| e + 1);
    &path[start..end.max(start)]
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
