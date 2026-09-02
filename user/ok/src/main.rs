#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use myos_user::{
    Heap, O_CREAT, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY, close, exec, exit, exit_code, fork,
    heap_init, listdir, mkdir, mount, open, open_flags, read, readlink, rename, rmdir, symlink,
    unlink, wait_status, write, write_fd,
};

#[global_allocator]
static GLOBAL: Heap = Heap;

#[inline(never)]
fn do_msg() {
    let Some(fd) = open(b"/msg") else {
        miss(b"fat nofd\n");
    };
    let mut buf = [0u8; 8];
    let n = read(fd, &mut buf);
    close(fd);
    if n == usize::MAX {
        miss(b"fat nread\n");
    }
    if n == 0 {
        miss(b"fat empty\n");
    }
    write(&buf[..n]);
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
    let mut msg = [0u8; 8];
    let nr = read(fd, &mut msg);
    close(fd);
    nr >= 7 && &msg[..7] == b"fat ok\n"
}

fn smoke_vfs() {
    let mut buf = [0u8; myos_user::LISTDIR_BUF];
    let n = listdir(b"/disk", &mut buf);
    if n != usize::MAX && n > 0 && buf_has(&buf[..n], b"ping") {
        write(b"disk ls ok\n");
    }

    let n = listdir(b"/dev", &mut buf);
    if n == usize::MAX || n == 0 || !buf_has(&buf[..n], b"vda") {
        write(b"vda missing\n");
        return;
    }
    write(b"vda ok\n");
    if buf_has(&buf[..n], b"net0") {
        write(b"net0 ok\n");
    }
    if buf_has(&buf[..n], b"vdb") {
        write(b"vdb ok\n");
    }
    if buf_has(&buf[..n], b"nvme0n1") {
        write(b"nvme ok\n");
        if let Some(fd) = open(b"/dev/nvme0n1") {
            let mut sec = [0u8; 512];
            let _ = read(fd, &mut sec);
            close(fd);
        }
        smoke_ext2();
    } else {
        write(b"nvme missing\n");
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
        write(b"fat mount fail\n");
        return;
    }

    let n = listdir(b"/fat", &mut buf);
    if n != usize::MAX && n > 0 && buf_has(&buf[..n], b"msg") {
        write(b"fat ls ok\n");
    }
    let Some(fd) = open(b"/fat/msg") else {
        write(b"fat open fail\n");
        return;
    };
    let mut msg = [0u8; 8];
    let nr = read(fd, &mut msg);
    close(fd);
    if nr >= 7 && &msg[..7] == b"fat ok\n" {
        write(b"fat read ok\n");
    }
}

fn smoke_ext2() {
    match fork() {
        Some(0) => {
            exec(b"/mkfs.ext2", &[b"mkfs.ext2", b"/dev/nvme0n1"]);
            write(b"ext2 mkfs exec fail\n");
            exit_code(1);
        }
        Some(_) => match wait_status() {
            Some((_, 0)) => {}
            _ => {
                write(b"ext2 mkfs fail\n");
                return;
            }
        },
        None => {
            write(b"ext2 fork fail\n");
            return;
        }
    }
    if !mount(b"/dev/nvme0n1", b"/ext2", b"ext2") {
        write(b"ext2 mount fail\n");
        return;
    }
    let Some(fd) = open_flags(b"/ext2/msg", O_WRONLY | O_CREAT | O_TRUNC) else {
        write(b"ext2 open fail\n");
        return;
    };
    if write_fd(fd, b"ext2 ok\n") == usize::MAX {
        write(b"ext2 write fail\n");
        close(fd);
        return;
    }
    close(fd);
    let Some(fd) = open_flags(b"/ext2/msg", O_RDONLY) else {
        write(b"ext2 reopen fail\n");
        return;
    };
    let mut msg = [0u8; 16];
    let nr = read(fd, &mut msg);
    close(fd);
    if nr >= 8 && &msg[..8] == b"ext2 ok\n" {
        write(b"ext2 ok\n");
    } else {
        write(b"ext2 read fail\n");
    }
}

fn smoke_disk() {
    let Some(fd) = open(b"/disk/ping") else {
        write(b"disk open fail\n");
        return;
    };
    let mut buf = [0u8; 16];
    let n = read(fd, &mut buf);
    close(fd);
    if n >= 8 && &buf[..8] == b"disk ok\n" {
        write(b"disk ok\n");
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
        write(b"tmpdev ls fail\n");
        return;
    }

    // /dev/null: writes discarded, reads return EOF.
    let Some(dn) = open_flags(b"/dev/null", O_RDWR) else {
        write(b"devnull open fail\n");
        return;
    };
    if write_fd(dn, b"discard") == usize::MAX {
        write(b"devnull write fail\n");
        close(dn);
        return;
    }
    let mut scratch = [0u8; 8];
    let nr = read(dn, &mut scratch);
    close(dn);
    if nr != 0 {
        write(b"devnull read fail\n");
        return;
    }
    write(b"devnull ok\n");

    // /tmp: create, write, read back.
    let Some(fd) = open_flags(b"/tmp/ci", O_WRONLY | O_CREAT | O_TRUNC) else {
        write(b"tmp open fail\n");
        return;
    };
    if write_fd(fd, b"hi\n") == usize::MAX {
        write(b"tmp write fail\n");
        close(fd);
        return;
    }
    close(fd);
    let Some(fd) = open_flags(b"/tmp/ci", O_RDONLY) else {
        write(b"tmp reopen fail\n");
        return;
    };
    let mut out = [0u8; 8];
    let n = read(fd, &mut out);
    close(fd);
    if n >= 3 && &out[..3] == b"hi\n" {
        write(b"tmp ok\n");
    } else {
        write(b"tmp read fail\n");
        return;
    }

    // mkdir / rename / symlink / readlink / unlink / rmdir on tmpfs.
    if !mkdir(b"/tmp/d") {
        write(b"mkdir fail\n");
        return;
    }
    let Some(fd) = open_flags(b"/tmp/d/f", O_WRONLY | O_CREAT | O_TRUNC) else {
        write(b"mkdir file fail\n");
        return;
    };
    if write_fd(fd, b"x") == usize::MAX {
        write(b"mkdir write fail\n");
        close(fd);
        return;
    }
    close(fd);
    if !rename(b"/tmp/d/f", b"/tmp/d/g") {
        write(b"rename fail\n");
        return;
    }
    if !symlink(b"g", b"/tmp/d/l") {
        write(b"symlink fail\n");
        return;
    }
    let mut linkbuf = [0u8; 8];
    let Some(ln) = readlink(b"/tmp/d/l", &mut linkbuf) else {
        write(b"readlink fail\n");
        return;
    };
    if ln != 1 || linkbuf[0] != b'g' {
        write(b"readlink bad\n");
        return;
    }
    if !unlink(b"/tmp/d/l") || !unlink(b"/tmp/d/g") {
        write(b"unlink fail\n");
        return;
    }
    if !rmdir(b"/tmp/d") {
        write(b"rmdir fail\n");
        return;
    }
    write(b"tmpops ok\n");

    smoke_proc(&mut buf);
}

fn smoke_proc(buf: &mut [u8]) {
    let n = listdir(b"/proc", buf);
    if n == usize::MAX || !buf_has(&buf[..n], b"mounts") {
        write(b"proc ls fail\n");
        return;
    }
    let Some(fd) = open(b"/proc/mounts") else {
        write(b"proc open fail\n");
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
            write(b"proc read fail\n");
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
        write(b"proc read fail\n");
        return;
    }
    write(b"proc ok\n");
}

fn main() -> ! {
    heap_init();
    let mut v = Vec::new();
    v.extend_from_slice(b"alloc ok\n");
    write(&v);

    write(b"user ok\n");
    do_msg();
    write(b"fat ok\n");

    smoke_disk();
    smoke_vfs();
    smoke_tmp_dev();
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

fn miss(m: &[u8]) -> ! {
    write(m);
    exit();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
