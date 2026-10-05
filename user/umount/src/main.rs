#![no_std]
#![no_main]

use myos_user::{close, exit, open, read, write, write_fd};

myos_user::x86_start!(main);

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: usize, argv: *const usize) -> ! {
    unsafe { myos_user::args::init_from_regs(argc, argv) };
    main()
}

fn main() -> ! {
    if myos_user::argc() != 2 {
        write(b"usage: umount DIRECTORY   (a mount point, see /proc/mounts)\n");
        myos_user::exit_code(1);
    }
    let dir = myos_user::arg(1).unwrap_or(b"");
    if myos_user::umount(dir) {
        exit();
    }
    // The kernel says only that it failed: not a mount point, or busy.
    let msg: &[u8] = if disk_mounted(dir) {
        b": busy (a file on it is open, or something is mounted below it)\n"
    } else {
        b": not a mount point of a disk\n"
    };
    write_fd(2, b"umount: ");
    write_fd(2, dir);
    write_fd(2, msg);
    myos_user::exit_code(1);
}

/// A `/proc/mounts` line names `dir` as the target of a disk (a `/dev/`
/// source; the kernel's own trees cannot be unmounted).
fn disk_mounted(dir: &[u8]) -> bool {
    let mut buf = [0u8; 4096];
    let Some(fd) = open(b"/proc/mounts") else {
        return false;
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
    let dir = dir.strip_suffix(b"/").unwrap_or(dir);
    buf[..n].split(|&b| b == b'\n').any(|line| {
        let mut fields = line.split(|&b| b == b' ');
        fields.next().is_some_and(|source| source.starts_with(b"/dev/")) && fields.next() == Some(dir)
    })
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
