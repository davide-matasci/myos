#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use myos_user::{
    Heap, O_CREAT, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY, SIGTERM, close, exec, exit, exit_code, fork,
    heap_init, kill, listdir, mkdir, mount, open, open_flags, pipe, read, readlink, rename,
    rmdir, stat_mode, status_fail, status_ok, status_warn, symlink, umount, unlink, wait_status,
    write_fd,
};

#[global_allocator]
static GLOBAL: Heap = Heap;

#[inline(never)]
fn do_msg() {
    let Some(fd) = open(b"/msg") else {
        miss("fat nofd");
    };
    let mut buf = [0u8; 16];
    let n = read(fd, &mut buf);
    close(fd);
    if n == usize::MAX {
        miss("fat nread");
    }
    const WANT: &[u8] = b"fat-msg\n";
    if n < WANT.len() || &buf[..WANT.len()] != WANT {
        miss("fat badmsg");
    }
    status_ok("msg");
}

fn buf_has(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn is_vd(name: &[u8]) -> bool {
    name.len() == 3 && name[0] == b'v' && name[1] == b'd' && (b'a'..=b'z').contains(&name[2])
}

fn fat_msg_ok() -> bool {
    let Some(fd) = open(b"/tmp/fat/msg") else {
        return false;
    };
    let mut msg = [0u8; 16];
    let nr = read(fd, &mut msg);
    close(fd);
    const WANT: &[u8] = b"fat-msg\n";
    nr >= WANT.len() && &msg[..WANT.len()] == WANT
}

fn smoke_vfs() {
    let mut buf = [0u8; myos_user::LISTDIR_BUF];
    let n = listdir(b"/dev", &mut buf);
    if n == usize::MAX || n == 0 || !buf_has(&buf[..n], b"vda") {
        status_warn("vda missing");
        return;
    }
    status_ok("vda");
    if buf_has(&buf[..n], b"net0") {
        status_ok("net0");
        // The NIC is a directory: `data` is the device, `ctl` names its MAC
        // (`mac 52:54:00:12:34:56`), a usable one being unicast and non-zero.
        if let Some(fd) = open(b"/dev/net0/ctl") {
            let mut text = [0u8; 128];
            let m = read(fd, &mut text);
            close(fd);
            let hex = |c: u8| (c as char).to_digit(16);
            if m != usize::MAX
                && m >= 21
                && &text[..4] == b"mac "
                && text[4..21].iter().enumerate().all(|(i, &c)| {
                    if i % 3 == 2 { c == b':' } else { hex(c).is_some() }
                })
                && text[4..21] != *b"00:00:00:00:00:00"
                && hex(text[5]).is_some_and(|d| d & 1 == 0)
            {
                status_ok("netmac");
            }
        }
    }
    if buf_has(&buf[..n], b"vdb") {
        status_ok("vdb");
    }
    let nvme = buf_has(&buf[..n], b"nvme0n1");
    if nvme {
        status_ok("nvme");
        if let Some(fd) = open(b"/dev/nvme0n1") {
            let mut sec = [0u8; 512];
            let _ = read(fd, &mut sec);
            close(fd);
        }
    } else {
        status_warn("nvme missing");
    }

    // ESP/empty can be vda on aarch64/riscv; try every vd* until /tmp/fat/msg
    // is ours, unmounting the others. A second run finds the first one's.
    let mut found = fat_msg_ok();
    if !found && !mkdir(b"/tmp/fat") {
        status_fail("fat mkdir fail");
        return;
    }
    let mut vds = [[0u8; 3]; 8];
    let mut nv = 0usize;
    for name in buf[..n].split(|&b| b == b'\n') {
        if is_vd(name) && nv < vds.len() {
            vds[nv].copy_from_slice(name);
            nv += 1;
        }
    }

    if !found {
        found = mount_test_fat(&vds[..nv]);
    }
    if !found {
        // Not a test boot (a VM's own virtio disks): nothing to check.
        status_warn("no test FAT volume");
        return;
    }

    let n = listdir(b"/tmp/fat", &mut buf);
    if n != usize::MAX && n > 0 && buf_has(&buf[..n], b"msg") {
        status_ok("fat ls");
    }
    let Some(fd) = open(b"/tmp/fat/msg") else {
        status_fail("fat open fail");
        return;
    };
    let mut msg = [0u8; 16];
    let nr = read(fd, &mut msg);
    close(fd);
    const WANT: &[u8] = b"fat-msg\n";
    if nr >= WANT.len() && &msg[..WANT.len()] == WANT {
        status_ok("fat read");
    }
    // The launcher's FAT volume says this is a test boot, whose NVMe disk
    // is a scratch image: formatting it anywhere else (a VM's own disks)
    // would wipe them on every boot.
    if nvme {
        smoke_ext2();
    }
}

fn smoke_ext2() {
    // A second run checks the disk the first one formatted and mounted.
    if stat_mode(b"/tmp/ext2").is_none() && !ext2_format() {
        return;
    }
    let Some(fd) = open_flags(b"/tmp/ext2/msg", O_WRONLY | O_CREAT | O_TRUNC) else {
        status_fail("ext2 open fail");
        return;
    };
    if write_fd(fd, b"ext2-msg\n") == usize::MAX {
        status_fail("ext2 write fail");
        close(fd);
        return;
    }
    close(fd);
    let Some(fd) = open_flags(b"/tmp/ext2/msg", O_RDONLY) else {
        status_fail("ext2 reopen fail");
        return;
    };
    let mut msg = [0u8; 16];
    let nr = read(fd, &mut msg);
    close(fd);
    const WANT: &[u8] = b"ext2-msg\n";
    if nr >= WANT.len() && &msg[..WANT.len()] == WANT {
        status_ok("ext2 rw");
    } else {
        status_fail("ext2 read fail");
    }
}

/// Mount the `vd*` disks on /tmp/fat in turn until one is the launcher's
/// volume; the others are unmounted again.
fn mount_test_fat(vds: &[[u8; 3]]) -> bool {
    for name in vds {
        let mut src = [0u8; 8];
        src[..5].copy_from_slice(b"/dev/");
        src[5..8].copy_from_slice(name);
        if !mount(&src, b"/tmp/fat", b"fat") {
            continue;
        }
        if fat_msg_ok() {
            return true;
        }
        umount(b"/tmp/fat");
    }
    false
}

/// `mkfs.ext2` the scratch disk and mount it on /tmp/ext2.
fn ext2_format() -> bool {
    match fork() {
        Some(0) => {
            exec(b"/bin/custom/mkfs.ext2", &[b"mkfs.ext2", b"/dev/nvme0n1"]);
            status_fail("ext2 mkfs exec fail");
            exit_code(1);
        }
        Some(_) => match wait_status() {
            Some((_, 0)) => {}
            _ => {
                status_fail("ext2 mkfs fail");
                return false;
            }
        },
        None => {
            status_fail("ext2 fork fail");
            return false;
        }
    }
    if !mkdir(b"/tmp/ext2") || !mount(b"/dev/nvme0n1", b"/tmp/ext2", b"ext2") {
        status_fail("ext2 mount fail");
        return false;
    }
    true
}

fn smoke_tmp_dev() {
    let mut buf = [0u8; myos_user::LISTDIR_BUF];
    let n = listdir(b"/", &mut buf);
    if n == usize::MAX
        || !buf_has(&buf[..n], b"tmp")
        || !buf_has(&buf[..n], b"dev")
        || !buf_has(&buf[..n], b"proc")
    {
        status_fail("tmpdev ls fail");
        return;
    }

    // /dev/null: writes discarded, reads return EOF.
    let Some(dn) = open_flags(b"/dev/null", O_RDWR) else {
        status_fail("devnull open fail");
        return;
    };
    if write_fd(dn, b"discard") == usize::MAX {
        status_fail("devnull write fail");
        close(dn);
        return;
    }
    let mut scratch = [0u8; 8];
    let nr = read(dn, &mut scratch);
    close(dn);
    if nr != 0 {
        status_fail("devnull read fail");
        return;
    }
    status_ok("devnull");

    // /tmp: create, write, read back.
    let Some(fd) = open_flags(b"/tmp/ci", O_WRONLY | O_CREAT | O_TRUNC) else {
        status_fail("tmp open fail");
        return;
    };
    if write_fd(fd, b"hi\n") == usize::MAX {
        status_fail("tmp write fail");
        close(fd);
        return;
    }
    close(fd);
    let Some(fd) = open_flags(b"/tmp/ci", O_RDONLY) else {
        status_fail("tmp reopen fail");
        return;
    };
    let mut out = [0u8; 8];
    let n = read(fd, &mut out);
    close(fd);
    if n >= 3 && &out[..3] == b"hi\n" {
        status_ok("tmp");
    } else {
        status_fail("tmp read fail");
        return;
    }

    // mkdir / rename / symlink / readlink / unlink / rmdir on tmpfs.
    if !mkdir(b"/tmp/d") {
        status_fail("mkdir fail");
        return;
    }
    let Some(fd) = open_flags(b"/tmp/d/f", O_WRONLY | O_CREAT | O_TRUNC) else {
        status_fail("mkdir file fail");
        return;
    };
    if write_fd(fd, b"x") == usize::MAX {
        status_fail("mkdir write fail");
        close(fd);
        return;
    }
    close(fd);
    if !rename(b"/tmp/d/f", b"/tmp/d/g") {
        status_fail("rename fail");
        return;
    }
    if !symlink(b"g", b"/tmp/d/l") {
        status_fail("symlink fail");
        return;
    }
    let mut linkbuf = [0u8; 8];
    let Some(ln) = readlink(b"/tmp/d/l", &mut linkbuf) else {
        status_fail("readlink fail");
        return;
    };
    if ln != 1 || linkbuf[0] != b'g' {
        status_fail("readlink bad");
        return;
    }
    if !unlink(b"/tmp/d/l") || !unlink(b"/tmp/d/g") {
        status_fail("unlink fail");
        return;
    }
    if !rmdir(b"/tmp/d") {
        status_fail("rmdir fail");
        return;
    }
    status_ok("tmpops");

    smoke_proc(&mut buf);
}


fn smoke_signal() {
    match fork() {
        None => {
            status_fail("signal fork fail");
            return;
        }
        Some(0) => {
            // Block in stdin read until SIGTERM; input::read wakes on pending.
            let mut b = [0u8; 1];
            let _ = read(0, &mut b);
            exit_code(99);
        }
        Some(child) => {
            if !kill(child, SIGTERM) {
                status_fail("signal kill fail");
                return;
            }
            match wait_status() {
                Some((_, status)) if status == (128 + SIGTERM as u8) => {
                    status_ok("signal");
                }
                Some((_, status)) => {
                    status_fail("signal bad status");
                    let _ = status;
                }
                None => status_fail("signal wait fail"),
            }
        }
    }
}

fn smoke_tty() {
    // The console through its files (docs/tty.md): init has no controlling
    // terminal yet, but its fd 1 is the console, so /proc/self/fd/1 names
    // /dev/console/data; the console's ctl has a window size, and /dev/null
    // is not a terminal.
    let mut path = [0u8; 80];
    let Some(mut n) = readlink(b"/proc/self/fd/1", &mut path[..64]) else {
        status_fail("tty link");
        return;
    };
    if !path[..n].ends_with(b"/data") {
        status_fail("tty link data");
        return;
    }
    n -= 4;
    path[n..n + 3].copy_from_slice(b"ctl");
    n += 3;
    let Some(fd) = open(&path[..n]) else {
        status_fail("tty ctl open");
        return;
    };
    let mut text = [0u8; 512];
    let mut len = 0;
    loop {
        let got = read(fd, &mut text[len..]);
        if got == 0 || got == usize::MAX || len + got >= text.len() {
            break;
        }
        len += got;
    }
    close(fd);
    // `winsize ROWS COLS`, both nonzero.
    let Some(line) = text[..len]
        .split(|&b| b == b'\n')
        .find_map(|l| l.strip_prefix(b"winsize "))
    else {
        status_fail("tty ctl winsize");
        return;
    };
    let mut nums = line.split(|&b| b == b' ').map(|w| {
        w.iter().fold(Some(0u32), |acc, &d| {
            acc.and_then(|v| d.is_ascii_digit().then(|| v * 10 + (d - b'0') as u32))
        })
    });
    match (nums.next(), nums.next()) {
        (Some(Some(rows)), Some(Some(cols))) if rows > 0 && cols > 0 => {}
        _ => {
            status_fail("tty ctl winsize");
            return;
        }
    }

    // The keyboard map init loaded is a line of the console's ctl too
    // (`keymap PATH`, docs/keymap.md).
    if !text[..len]
        .split(|&b| b == b'\n')
        .any(|l| l.starts_with(b"keymap /lib/kbd/"))
    {
        status_fail("tty ctl keymap");
        return;
    }

    let Some(dn) = open(b"/dev/null") else {
        status_fail("tty null open");
        return;
    };

    let mut link = *b"/proc/self/fd/\0\0\0";
    link[14] = b'0' + (dn / 10) as u8;
    link[15] = b'0' + (dn % 10) as u8;
    let mut target = [0u8; 64];
    let m = readlink(&link[..16], &mut target).unwrap_or(0);
    close(dn);
    if m == 0 || target[..m].ends_with(b"/data") {
        status_fail("tty null is no terminal");
        return;
    }

    status_ok("tty");
}

fn smoke_proc(buf: &mut [u8]) {
    let n = listdir(b"/proc", buf);
    if n == usize::MAX || !buf_has(&buf[..n], b"mounts") {
        status_fail("proc ls fail");
        return;
    }
    let Some(fd) = open(b"/proc/mounts") else {
        status_fail("proc open fail");
        return;
    };
    // fd_read copies at most 128 bytes per syscall; concatenate until EOF.
    let mut nr = 0usize;
    loop {
        if nr >= buf.len() {
            break;
        }
        let n = read(fd, &mut buf[nr..]);
        if n == usize::MAX {
            close(fd);
            status_fail("proc read fail");
            return;
        }
        if n == 0 {
            break;
        }
        nr += n;
    }
    close(fd);
    // The FAT mount is there only when the boot carries the launcher's
    // volume (smoke_vfs); another VM's disks are not mounted.
    let fat = fat_msg_ok();
    if !buf_has(&buf[..nr], b"tmpfs")
        || !buf_has(&buf[..nr], b"devfs")
        || !buf_has(&buf[..nr], b"procfs")
        || fat && (!buf_has(&buf[..nr], b"fat") || !buf_has(&buf[..nr], b"/dev/vd"))
    {
        status_fail("proc read fail");
        return;
    }
    status_ok("proc");
}

fn main() -> ! {
    heap_init();
    let mut v = Vec::new();
    v.extend_from_slice(b"probe");
    let _ = v;
    status_ok("alloc");

    status_ok("user");
    // Echoes bootfs /msg (`[ OK ] fat`); no duplicate status line.
    do_msg();

    smoke_vfs();
    smoke_tmp_dev();
    smoke_tty();
    smoke_signal();
    exit();
}

#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    main()
}

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_argc: usize, _argv: *const usize) -> ! {
    main()
}

fn miss(label: &str) -> ! {
    status_fail(label);
    exit();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
