//! Formatting: a fresh ext2 filesystem over a whole device, laid out the
//! way `mke2fs -t ext2` would (without its optional extras): block groups of
//! `8 * block_size` blocks, each with its bitmaps and inode table,
//! superblock and descriptor backups in the `sparse_super` groups, a root
//! directory and `lost+found`.

use alloc::vec;

use crate::layout::*;
use crate::{Device, Error, Result};

/// Devices below this get 1 KiB blocks and an inode per 4 KiB (many small
/// files on a small disk), the others 4 KiB blocks and an inode per 16 KiB.
const SMALL: u64 = 512 * 1024 * 1024;
const INODE_SIZE: usize = 128;
const LOST_FOUND_INO: u32 = 11;
/// A last group smaller than its metadata plus this is left unused.
const MIN_LAST_GROUP_DATA: u32 = 50;

/// Format `dev`, `bytes` long.
pub fn mkfs<D: Device>(dev: &mut D, bytes: u64) -> Result<()> {
    let (bs, ratio) = if bytes < SMALL { (1024usize, 4096u64) } else { (4096, 16384) };
    let first_data = if bs == 1024 { 1u32 } else { 0 };
    let bpg = (bs * 8) as u32;
    let mut blocks = (bytes / bs as u64).min(u32::MAX as u64) as u32;
    if blocks < first_data + 64 {
        return Err(Error::NoSpace);
    }
    let mut groups = (blocks - first_data).div_ceil(bpg);
    let per_block = (bs / INODE_SIZE) as u32;
    let ipg = {
        let want = (blocks as u64 * bs as u64 / ratio).div_ceil(groups as u64) as u32;
        // Whole inode table blocks, at least 16 inodes, at most a bitmap's worth.
        want.max(16).div_ceil(per_block).saturating_mul(per_block).min(bpg)
    };
    let itable = ipg / per_block;
    let gdt_blocks = (groups as usize * GD_SIZE).div_ceil(bs) as u32;
    let meta = |g: u32| if has_super(g, true) { 1 + gdt_blocks } else { 0 } + 2 + itable;
    let last = blocks - first_data - (groups - 1) * bpg;
    if groups > 1 && last < meta(groups - 1) + MIN_LAST_GROUP_DATA {
        groups -= 1;
        blocks = first_data + groups * bpg;
    } else if last < meta(groups - 1) + 2 {
        return Err(Error::NoSpace);
    }
    let geo = Geometry {
        block_size: bs,
        first_data_block: first_data,
        blocks_count: blocks,
        inodes_count: groups * ipg,
        blocks_per_group: bpg,
        inodes_per_group: ipg,
        inode_size: INODE_SIZE,
        first_ino: GOOD_OLD_FIRST_INO,
        groups,
        filetype: true,
        large_file: true,
    };
    let now = dev.now();
    let write = |dev: &mut D, block: u32, data: &[u8]| {
        if dev.write(block as u64 * bs as u64, data) { Ok(()) } else { Err(Error::Io) }
    };

    // Group 0's first two data blocks: the root directory and lost+found.
    let data0 = geo.group_first(0) + meta(0);
    let (root_blk, lf_blk) = (data0, data0 + 1);

    let mut gdt = vec![0u8; gdt_blocks as usize * bs];
    let (mut free_blocks, mut free_inodes) = (0u32, 0u32);
    let zero = vec![0u8; bs];
    for g in 0..groups {
        let first = geo.group_first(g);
        let len = geo.group_blocks(g);
        let bb = first + if has_super(g, true) { 1 + gdt_blocks } else { 0 };
        let (ib, it) = (bb + 1, bb + 2);
        let mut used_blocks = meta(g);
        let mut used_inodes = 0;
        let mut dirs = 0;
        if g == 0 {
            used_blocks += 2;
            used_inodes = LOST_FOUND_INO;
            dirs = 2;
        }

        // The block bitmap: metadata (and group 0's two directory blocks)
        // in use, and the bits past a short last group set as padding.
        let mut bm = vec![0u8; bs];
        for i in (0..used_blocks).chain(len..bpg) {
            bm[(i / 8) as usize] |= 1 << (i % 8);
        }
        write(dev, bb, &bm)?;
        let mut im = vec![0u8; bs];
        for i in (0..used_inodes).chain(ipg..(bs * 8) as u32) {
            im[(i / 8) as usize] |= 1 << (i % 8);
        }
        write(dev, ib, &im)?;
        for b in it..it + itable {
            write(dev, b, &zero)?;
        }

        let o = g as usize * GD_SIZE;
        put32(&mut gdt, o + G_BLOCK_BITMAP, bb);
        put32(&mut gdt, o + G_INODE_BITMAP, ib);
        put32(&mut gdt, o + G_INODE_TABLE, it);
        put16(&mut gdt, o + G_FREE_BLOCKS, (len - used_blocks) as u16);
        put16(&mut gdt, o + G_FREE_INODES, (ipg - used_inodes) as u16);
        put16(&mut gdt, o + G_USED_DIRS, dirs);
        free_blocks += len - used_blocks;
        free_inodes += ipg - used_inodes;
    }

    // The two directories.
    let dirent = |blk: &mut [u8], off: usize, ino: u32, rec: usize, name: &[u8]| {
        put32(blk, off, ino);
        put16(blk, off + 4, rec as u16);
        blk[off + 6] = name.len() as u8;
        blk[off + 7] = FT_DIR;
        blk[off + 8..off + 8 + name.len()].copy_from_slice(name);
    };
    let mut root = vec![0u8; bs];
    dirent(&mut root, 0, ROOT_INO, 12, b".");
    dirent(&mut root, 12, ROOT_INO, 12, b"..");
    dirent(&mut root, 24, LOST_FOUND_INO, bs - 24, b"lost+found");
    write(dev, root_blk, &root)?;
    let mut lf = vec![0u8; bs];
    dirent(&mut lf, 0, LOST_FOUND_INO, 12, b".");
    dirent(&mut lf, 12, ROOT_INO, bs - 12, b"..");
    write(dev, lf_blk, &lf)?;
    let table0 = get32(&gdt, G_INODE_TABLE);
    let mut itable0 = vec![0u8; bs];
    for (ino, mode, links, blk) in [(ROOT_INO, S_IFDIR | 0o755, 3u16, root_blk), (LOST_FOUND_INO, S_IFDIR | 0o700, 2, lf_blk)] {
        let off = (ino - 1) as usize * INODE_SIZE;
        let block = table0 + (off / bs) as u32;
        if !dev.read(block as u64 * bs as u64, &mut itable0) {
            return Err(Error::Io);
        }
        let n = &mut itable0[off % bs..off % bs + INODE_SIZE];
        put16(n, 0, mode);
        put32(n, 4, bs as u32);
        for t in [8, 12, 16] {
            put32(n, t, now);
        }
        put16(n, 26, links);
        put32(n, 28, (bs / 512) as u32);
        put32(n, 40, blk);
        write(dev, block, &itable0)?;
    }

    // The superblock, its backups and the descriptor table copies.
    let mut sb = vec![0u8; SB_SIZE];
    put32(&mut sb, S_INODES_COUNT, geo.inodes_count);
    put32(&mut sb, S_BLOCKS_COUNT, blocks);
    put32(&mut sb, S_FREE_BLOCKS, free_blocks);
    put32(&mut sb, S_FREE_INODES, free_inodes);
    put32(&mut sb, S_FIRST_DATA_BLOCK, first_data);
    let log = bs.trailing_zeros() - 10;
    put32(&mut sb, S_LOG_BLOCK_SIZE, log);
    put32(&mut sb, 28, log); // s_log_frag_size
    put32(&mut sb, S_BLOCKS_PER_GROUP, bpg);
    put32(&mut sb, 36, bpg); // s_frags_per_group
    put32(&mut sb, S_INODES_PER_GROUP, ipg);
    put32(&mut sb, 44, now); // s_mtime
    put32(&mut sb, S_WTIME, now);
    put16(&mut sb, 54, 0xffff); // s_max_mnt_count: no forced checks
    put16(&mut sb, S_MAGIC, MAGIC);
    put16(&mut sb, S_STATE, 1); // clean
    put16(&mut sb, 60, 1); // s_errors: continue
    put32(&mut sb, 64, now); // s_lastcheck
    put32(&mut sb, S_REV_LEVEL, 1);
    put32(&mut sb, S_FIRST_INO, GOOD_OLD_FIRST_INO);
    put16(&mut sb, S_INODE_SIZE, INODE_SIZE as u16);
    put32(&mut sb, S_FEATURE_INCOMPAT, INCOMPAT_FILETYPE);
    put32(&mut sb, S_FEATURE_RO_COMPAT, RO_COMPAT_SPARSE_SUPER | RO_COMPAT_LARGE_FILE);
    // A volume UUID that differs per format.
    let seed = (bytes ^ (now as u64) << 32 ^ 0x6d796f73_65787432).to_le_bytes();
    for (i, b) in sb[104..120].iter_mut().enumerate() {
        *b = seed[i % 8].wrapping_mul(31).wrapping_add(i as u8 * 17);
    }
    sb[120..124].copy_from_slice(b"myos");
    for g in (0..groups).filter(|&g| has_super(g, true)) {
        let first = geo.group_first(g);
        put16(&mut sb, S_BLOCK_GROUP_NR, g as u16);
        let sb_at = if g == 0 { SB_OFFSET } else { first as u64 * bs as u64 };
        if !dev.write(sb_at, &sb) {
            return Err(Error::Io);
        }
        if !dev.write((first + 1) as u64 * bs as u64, &gdt) {
            return Err(Error::Io);
        }
    }
    Ok(())
}
