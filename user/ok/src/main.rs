#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use myos_user::{
    Heap, O_CREAT, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY, SIGTERM, close, exec, exit, exit_code, fork,
    heap_init, ioctl, kill, listdir, mkdir, mount, open, open_flags, pipe, read, readlink, rename,
    rmdir, status_fail, status_ok, status_warn, symlink, unlink, wait_status, write_fd,
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
    let Some(fd) = open(b"/fat/msg") else {
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
    let n = listdir(b"/disk", &mut buf);
    if n != usize::MAX && n > 0 && buf_has(&buf[..n], b"ping") {
        status_ok("disk ls");
    }

    let n = listdir(b"/dev", &mut buf);
    if n == usize::MAX || n == 0 || !buf_has(&buf[..n], b"vda") {
        status_warn("vda missing");
        return;
    }
    status_ok("vda");
    if buf_has(&buf[..n], b"net0") {
        status_ok("net0");
        if let Some(fd) = open_flags(b"/dev/net0", O_RDWR) {
            let mut mac = [0u8; 6];
            // Keep in sync with myos_abi::MYOS_IOCTL_NET_GETMAC.
            const MYOS_IOCTL_NET_GETMAC: usize = 0x4d01;
            if ioctl(fd, MYOS_IOCTL_NET_GETMAC, mac.as_mut_ptr() as usize) != usize::MAX
                && mac.iter().any(|&b| b != 0)
                && mac[0] & 1 == 0
            {
                status_ok("netmac");
            }
            close(fd);
        }
    }
    if buf_has(&buf[..n], b"vdb") {
        status_ok("vdb");
    }
    if buf_has(&buf[..n], b"nvme0n1") {
        status_ok("nvme");
        if let Some(fd) = open(b"/dev/nvme0n1") {
            let mut sec = [0u8; 512];
            let _ = read(fd, &mut sec);
            close(fd);
        }
        smoke_ext2();
    } else {
        status_warn("nvme missing");
    }

    // ESP/empty can be vda on aarch64/riscv; try every vd* until /fat/msg is ours.
    let mut vds = [[0u8; 3]; 8];
    let mut nv = 0usize;
    for name in buf[..n].split(|&b| b == b'\n') {
        if is_vd(name) && nv < vds.len() {
            vds[nv].copy_from_slice(name);
            nv += 1;
        }
    }

    let mut found = false;
    for i in 0..nv {
        let name = &vds[i];
        let mut src = [0u8; 8];
        src[..5].copy_from_slice(b"/dev/");
        src[5..8].copy_from_slice(name);
        if !mount(&src, b"/fat", b"fat") {
            continue;
        }
        if fat_msg_ok() {
            found = true;
            break;
        }
    }
    if !found {
        status_fail("fat mount fail");
        return;
    }

    let n = listdir(b"/fat", &mut buf);
    if n != usize::MAX && n > 0 && buf_has(&buf[..n], b"msg") {
        status_ok("fat ls");
    }
    let Some(fd) = open(b"/fat/msg") else {
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
}

fn smoke_ext2() {
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
                return;
            }
        },
        None => {
            status_fail("ext2 fork fail");
            return;
        }
    }
    if !mount(b"/dev/nvme0n1", b"/ext2", b"ext2") {
        status_fail("ext2 mount fail");
        return;
    }
    let Some(fd) = open_flags(b"/ext2/msg", O_WRONLY | O_CREAT | O_TRUNC) else {
        status_fail("ext2 open fail");
        return;
    };
    if write_fd(fd, b"ext2-msg\n") == usize::MAX {
        status_fail("ext2 write fail");
        close(fd);
        return;
    }
    close(fd);
    let Some(fd) = open_flags(b"/ext2/msg", O_RDONLY) else {
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

fn smoke_disk() {
    let Some(fd) = open(b"/disk/ping") else {
        status_fail("disk open fail");
        return;
    };
    let mut buf = [0u8; 16];
    let n = read(fd, &mut buf);
    close(fd);
    const WANT: &[u8] = b"disk-msg\n";
    if n >= WANT.len() && &buf[..WANT.len()] == WANT {
        status_ok("disk");
    }
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

fn smoke_ioctl() {
    const TIOCGWINSZ: usize = 0x5413;
    let mut ws = [0u16; 4];

    // Prefer /dev/tty; also exercise Console fd 1.
    let tty_fd = open(b"/dev/tty");
    let fd = tty_fd.unwrap_or(1);
    if ioctl(fd, TIOCGWINSZ, ws.as_mut_ptr() as usize) == usize::MAX
        || ws[0] != 24
        || ws[1] != 80
    {
        if tty_fd.is_some() {
            close(fd);
        }
        status_fail("ioctl tty fail");
        return;
    }
    if let Some(fd) = tty_fd {
        close(fd);
    }

    let mut ws1 = [0u16; 4];
    if ioctl(1, TIOCGWINSZ, ws1.as_mut_ptr() as usize) == usize::MAX
        || ws1[0] != 24
        || ws1[1] != 80
    {
        status_fail("ioctl fd1 fail");
        return;
    }

    let Some(dn) = open(b"/dev/null") else {
        status_fail("ioctl null open fail");
        return;
    };
    let r = ioctl(dn, TIOCGWINSZ, ws.as_mut_ptr() as usize);
    close(dn);
    if r != usize::MAX {
        status_fail("ioctl null should fail");
        return;
    }

    let Some((rfd, wfd)) = pipe() else {
        status_fail("ioctl pipe open fail");
        return;
    };
    let r = ioctl(rfd, TIOCGWINSZ, ws.as_mut_ptr() as usize);
    close(rfd);
    close(wfd);
    if r != usize::MAX {
        status_fail("ioctl pipe should fail");
        return;
    }

    status_ok("ioctl");
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
    if !buf_has(&buf[..nr], b"tmpfs")
        || !buf_has(&buf[..nr], b"devfs")
        || !buf_has(&buf[..nr], b"procfs")
        || !buf_has(&buf[..nr], b"fat")
        || !buf_has(&buf[..nr], b"/dev/vd")
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

    smoke_disk();
    smoke_vfs();
    smoke_tmp_dev();
    smoke_ioctl();
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
