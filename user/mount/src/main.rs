#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![no_main]

use myos_user::{close, exit, mount, open, read, stat_mode, write, write_fd};

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
        4 => do_mount(),
        _ => {
            write(b"usage: mount [SOURCE TARGET FSTYPE]\n");
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
    let src = myos_user::arg(1).unwrap_or(b"");
    let tgt = myos_user::arg(2).unwrap_or(b"");
    let fs = myos_user::arg(3).unwrap_or(b"");
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

const S_IFMT: u32 = 0o170000;
const S_IFBLK: u32 = 0o060000;
const S_IFDIR: u32 = 0o040000;

/// `mount: ` and the parts on stderr, exit 1.
fn fail(parts: &[&[u8]]) -> ! {
    write_fd(2, b"mount: ");
    for p in parts {
        write_fd(2, p);
    }
    write_fd(2, b"\n");
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
