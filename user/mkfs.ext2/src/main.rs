#![no_std]
#![no_main]

use myos_user::{O_RDWR, SEEK_END, SEEK_SET, close, exit, lseek, open_flags, write, write_fd};

myos_user::x86_start!(main);

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: usize, argv: *const usize) -> ! {
    unsafe { myos_user::args::init_from_regs(argc, argv) };
    main()
}

const BLK: usize = 1024;
const MIN_BYTES: usize = 1024 * 1024;
const INODES: u32 = 128;
const INODE_SIZE: usize = 128;
const FIRST_INO: u32 = 11;
const MAGIC: u16 = 0xEF53;
const ROOT_INO: u32 = 2;
const ROOT_BLK: u32 = 21;

fn put_u16(buf: &mut [u8], off: usize, v: u16) {
    buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
}

fn put_u32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

fn die(msg: &[u8]) -> ! {
    write(msg);
    myos_user::exit_code(1);
}

fn write_at(fd: usize, off: usize, buf: &[u8]) -> bool {
    if lseek(fd, off, SEEK_SET) == usize::MAX {
        return false;
    }
    let mut n = 0usize;
    while n < buf.len() {
        let w = write_fd(fd, &buf[n..]);
        if w == 0 || w == usize::MAX {
            return false;
        }
        n += w;
    }
    true
}

fn set_bit(bm: &mut [u8], i: u32) {
    let byte = (i / 8) as usize;
    let bit = (i % 8) as u8;
    bm[byte] |= 1 << bit;
}

fn main() -> ! {
    if myos_user::argc() != 2 {
        die(b"usage: mkfs.ext2 <device>\n");
    }
    let dev = myos_user::arg(1).unwrap_or(b"");
    if dev.is_empty() {
        die(b"mkfs.ext2: missing device\n");
    }
    let Some(fd) = open_flags(dev, O_RDWR) else {
        die(b"mkfs.ext2: open failed\n");
    };
    let size = lseek(fd, 0, SEEK_END);
    if size == usize::MAX {
        close(fd);
        die(b"mkfs.ext2: seek failed\n");
    }
    if size < MIN_BYTES || size % BLK != 0 {
        close(fd);
        die(b"mkfs.ext2: device too small or not 1KiB aligned\n");
    }
    let blocks = (size / BLK) as u32;
    if blocks < ROOT_BLK + 1 {
        close(fd);
        die(b"mkfs.ext2: device too small\n");
    }

    let used_blocks = ROOT_BLK + 1; // 0..=21
    let free_blocks = blocks - used_blocks;
    if free_blocks > u16::MAX as u32 {
        close(fd);
        die(b"mkfs.ext2: device too large for one group\n");
    }
    let used_inodes = FIRST_INO - 1; // 1..10
    let free_inodes = INODES - used_inodes;

    let mut sb = [0u8; BLK];
    put_u32(&mut sb, 0, INODES);
    put_u32(&mut sb, 4, blocks);
    put_u32(&mut sb, 8, 0);
    put_u32(&mut sb, 12, free_blocks);
    put_u32(&mut sb, 16, free_inodes);
    put_u32(&mut sb, 20, 1); // s_first_data_block
    put_u32(&mut sb, 24, 0); // s_log_block_size
    put_u32(&mut sb, 28, 0); // s_log_frag_size
    put_u32(&mut sb, 32, blocks);
    put_u32(&mut sb, 36, blocks);
    put_u32(&mut sb, 40, INODES);
    put_u16(&mut sb, 52, 0); // s_mnt_count
    put_u16(&mut sb, 54, 0xFFFF); // s_max_mnt_count
    put_u16(&mut sb, 56, MAGIC);
    put_u16(&mut sb, 58, 1); // s_state valid
    put_u16(&mut sb, 60, 1); // s_errors
    put_u32(&mut sb, 76, 1); // s_rev_level
    put_u32(&mut sb, 84, FIRST_INO);
    put_u16(&mut sb, 88, INODE_SIZE as u16);
    put_u16(&mut sb, 90, 0); // s_block_group_nr
    sb[120..124].copy_from_slice(b"myos");

    let mut gd = [0u8; BLK];
    put_u32(&mut gd, 0, 3); // bg_block_bitmap
    put_u32(&mut gd, 4, 4); // bg_inode_bitmap
    put_u32(&mut gd, 8, 5); // bg_inode_table
    put_u16(&mut gd, 12, free_blocks as u16);
    put_u16(&mut gd, 14, free_inodes as u16);
    put_u16(&mut gd, 16, 1); // bg_used_dirs_count

    let mut bbm = [0u8; BLK];
    for i in 0..used_blocks {
        set_bit(&mut bbm, i);
    }
    // Bits past the device must not look free.
    let bitmap_bits = (BLK * 8) as u32;
    for i in blocks..bitmap_bits {
        set_bit(&mut bbm, i);
    }

    let mut ibm = [0u8; BLK];
    for i in 0..used_inodes {
        set_bit(&mut ibm, i); // bit 0 = inode 1
    }
    let inode_bits = (BLK * 8) as u32;
    for i in INODES..inode_bits {
        set_bit(&mut ibm, i);
    }

    // Inode table: 16 blocks, root inode at index 1 (inode 2).
    let mut it0 = [0u8; BLK];
    let io = INODE_SIZE; // inode 2
    put_u16(&mut it0, io, 0x41ED); // S_IFDIR|0755
    put_u32(&mut it0, io + 4, BLK as u32); // i_size
    put_u16(&mut it0, io + 26, 2); // i_links_count
    put_u32(&mut it0, io + 28, 2); // i_blocks (512-byte units)
    put_u32(&mut it0, io + 40, ROOT_BLK); // i_block[0]

    let mut dir = [0u8; BLK];
    // "." rec_len=12, name_len=1 (classic no-FILETYPE: name_len is u16)
    put_u32(&mut dir, 0, ROOT_INO);
    put_u16(&mut dir, 4, 12);
    put_u16(&mut dir, 6, 1);
    dir[8] = b'.';
    // ".." fills the rest of the block
    put_u32(&mut dir, 12, ROOT_INO);
    put_u16(&mut dir, 16, (BLK - 12) as u16);
    put_u16(&mut dir, 18, 2);
    dir[20] = b'.';
    dir[21] = b'.';

    let ok = write_at(fd, BLK, &sb)
        && write_at(fd, 2 * BLK, &gd)
        && write_at(fd, 3 * BLK, &bbm)
        && write_at(fd, 4 * BLK, &ibm)
        && write_at(fd, 5 * BLK, &it0);
    if !ok {
        close(fd);
        die(b"mkfs.ext2: write failed\n");
    }
    let zero = [0u8; BLK];
    for b in 1..16u32 {
        if !write_at(fd, (5 + b as usize) * BLK, &zero) {
            close(fd);
            die(b"mkfs.ext2: write failed\n");
        }
    }
    if !write_at(fd, ROOT_BLK as usize * BLK, &dir) {
        close(fd);
        die(b"mkfs.ext2: write failed\n");
    }
    close(fd);
    exit();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
