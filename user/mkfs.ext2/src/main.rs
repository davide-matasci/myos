#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![no_main]

//! `mkfs.ext2 DEVICE`: format a whole block device as ext2 (the `ext2fs`
//! crate's `mkfs`, which the ext2 module mounts; Linux reads it too).

use myos_user::{Heap, O_RDWR, SEEK_END, SEEK_SET, close, exit, lseek, open_flags, read, write, write_fd};

#[global_allocator]
static GLOBAL: Heap = Heap;

myos_user::x86_start!(main);

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: usize, argv: *const usize) -> ! {
    unsafe { myos_user::args::init_from_regs(argc, argv) };
    main()
}

/// The device, through its fd.
struct Fd(usize);

impl ext2fs::Device for Fd {
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> bool {
        if lseek(self.0, offset as usize, SEEK_SET) == usize::MAX {
            return false;
        }
        let mut n = 0;
        while n < buf.len() {
            match read(self.0, &mut buf[n..]) {
                0 | usize::MAX => return false,
                got => n += got,
            }
        }
        true
    }

    fn write(&mut self, offset: u64, buf: &[u8]) -> bool {
        if lseek(self.0, offset as usize, SEEK_SET) == usize::MAX {
            return false;
        }
        let mut n = 0;
        while n < buf.len() {
            match write_fd(self.0, &buf[n..]) {
                0 | usize::MAX => return false,
                put => n += put,
            }
        }
        true
    }
}

fn die(msg: &[u8]) -> ! {
    write(msg);
    myos_user::exit_code(1);
}

fn main() -> ! {
    myos_user::heap_init();
    let dev = match (myos_user::argc(), myos_user::arg(1)) {
        (2, Some(dev)) if !dev.is_empty() => dev,
        _ => die(b"usage: mkfs.ext2 DEVICE\n"),
    };
    let Some(fd) = open_flags(dev, O_RDWR) else {
        die(b"mkfs.ext2: cannot open the device\n");
    };
    let size = lseek(fd, 0, SEEK_END);
    if size == usize::MAX {
        die(b"mkfs.ext2: cannot size the device\n");
    }
    match ext2fs::mkfs(&mut Fd(fd), size as u64) {
        Ok(()) => {}
        Err(ext2fs::Error::NoSpace) => die(b"mkfs.ext2: the device is too small\n"),
        Err(_) => die(b"mkfs.ext2: write failed\n"),
    }
    close(fd);
    exit();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
