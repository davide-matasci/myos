#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![no_main]

//! `mkfs.fat DEVICE`: format a whole block device as FAT32 (the `fatvol`
//! crate's `format`, fstool's formatter, as the host makes the boot disk's
//! ESP; the fat module mounts it, UEFI firmware and Linux read it too).

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

const SECTOR: usize = 512;

/// The device, through its fd: `sectors` of 512 bytes.
struct Fd {
    fd: usize,
    sectors: u64,
}

impl fatvol::SectorDriver for Fd {
    type Error = ();

    fn sector_size(&self) -> u32 {
        SECTOR as u32
    }

    fn sector_count(&self) -> u64 {
        self.sectors
    }

    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
        if lseek(self.fd, lba as usize * SECTOR, SEEK_SET) == usize::MAX {
            return Err(());
        }
        let mut n = 0;
        while n < buf.len() {
            match read(self.fd, &mut buf[n..]) {
                0 | usize::MAX => return Err(()),
                got => n += got,
            }
        }
        Ok(())
    }

    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), ()> {
        if lseek(self.fd, lba as usize * SECTOR, SEEK_SET) == usize::MAX {
            return Err(());
        }
        let mut n = 0;
        while n < buf.len() {
            match write_fd(self.fd, &buf[n..]) {
                0 | usize::MAX => return Err(()),
                put => n += put,
            }
        }
        Ok(())
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
        _ => die(b"usage: mkfs.fat DEVICE\n"),
    };
    let Some(fd) = open_flags(dev, O_RDWR) else {
        die(b"mkfs.fat: cannot open the device\n");
    };
    let size = lseek(fd, 0, SEEK_END);
    if size == usize::MAX {
        die(b"mkfs.fat: cannot size the device\n");
    }
    // A serial number for the volume: the time it was made.
    let id = myos_user::gettimeofday().map_or(0, |(s, _)| s as u32);
    let mut dev = Fd { fd, sectors: (size / SECTOR) as u64 };
    match fatvol::format(&mut dev, b"NO NAME    ", id) {
        Ok(()) => {}
        Err(fatvol::Error::TooBig) => die(b"mkfs.fat: the device is too big for FAT32\n"),
        Err(_) => die(b"mkfs.fat: the device is too small for FAT32, or a write failed\n"),
    }
    close(fd);
    exit();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
