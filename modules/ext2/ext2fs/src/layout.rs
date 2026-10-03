//! On-disk constants and field offsets, and the geometry a superblock
//! describes.

use crate::{Error, Result};

pub const MAGIC: u16 = 0xEF53;
/// The primary superblock: 1024 bytes at byte 1024, whatever the block size.
pub const SB_OFFSET: u64 = 1024;
pub const SB_SIZE: usize = 1024;
pub const ROOT_INO: u32 = 2;
/// The first inode number that is not reserved (revision 0; revision 1
/// stores it, `s_first_ino`).
pub const GOOD_OLD_FIRST_INO: u32 = 11;
pub const GOOD_OLD_INODE_SIZE: u16 = 128;
/// A group descriptor (no `64bit` feature).
pub const GD_SIZE: usize = 32;

pub const S_IFMT: u16 = 0o170000;
pub const S_IFREG: u16 = 0o100000;
pub const S_IFDIR: u16 = 0o040000;
pub const S_IFLNK: u16 = 0o120000;

/// Directory entry file types (`filetype`).
pub const FT_REG: u8 = 1;
pub const FT_DIR: u8 = 2;
pub const FT_SYMLINK: u8 = 7;

/// `i_flags`: a hashed (`dir_index`) directory.
pub const INDEX_FL: u32 = 0x1000;

pub const INCOMPAT_FILETYPE: u32 = 0x0002;
pub const RO_COMPAT_SPARSE_SUPER: u32 = 0x0001;
pub const RO_COMPAT_LARGE_FILE: u32 = 0x0002;

// Superblock fields.
pub const S_INODES_COUNT: usize = 0;
pub const S_BLOCKS_COUNT: usize = 4;
pub const S_FREE_BLOCKS: usize = 12;
pub const S_FREE_INODES: usize = 16;
pub const S_FIRST_DATA_BLOCK: usize = 20;
pub const S_LOG_BLOCK_SIZE: usize = 24;
pub const S_BLOCKS_PER_GROUP: usize = 32;
pub const S_INODES_PER_GROUP: usize = 40;
pub const S_WTIME: usize = 48;
pub const S_MAGIC: usize = 56;
pub const S_STATE: usize = 58;
pub const S_REV_LEVEL: usize = 76;
pub const S_FIRST_INO: usize = 84;
pub const S_INODE_SIZE: usize = 88;
pub const S_BLOCK_GROUP_NR: usize = 90;
pub const S_FEATURE_INCOMPAT: usize = 96;
pub const S_FEATURE_RO_COMPAT: usize = 100;

// Group descriptor fields.
pub const G_BLOCK_BITMAP: usize = 0;
pub const G_INODE_BITMAP: usize = 4;
pub const G_INODE_TABLE: usize = 8;
pub const G_FREE_BLOCKS: usize = 12;
pub const G_FREE_INODES: usize = 14;
pub const G_USED_DIRS: usize = 16;

pub fn get16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

pub fn get32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

pub fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

pub fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// The shape of a filesystem, from its superblock.
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub block_size: usize,
    pub first_data_block: u32,
    pub blocks_count: u32,
    pub inodes_count: u32,
    pub blocks_per_group: u32,
    pub inodes_per_group: u32,
    pub inode_size: usize,
    pub first_ino: u32,
    pub groups: u32,
    /// Directory entries carry a file type.
    pub filetype: bool,
    pub large_file: bool,
}

impl Geometry {
    pub fn parse(sb: &[u8]) -> Result<Self> {
        if get16(sb, S_MAGIC) != MAGIC {
            return Err(Error::Unsupported);
        }
        let log = get32(sb, S_LOG_BLOCK_SIZE);
        if log > 2 {
            return Err(Error::Unsupported);
        }
        let rev = get32(sb, S_REV_LEVEL);
        let (incompat, ro_compat) = if rev >= 1 {
            (get32(sb, S_FEATURE_INCOMPAT), get32(sb, S_FEATURE_RO_COMPAT))
        } else {
            (0, 0)
        };
        // What can be mounted (and written) safely: anything else changes
        // the layout this code reads or writes.
        if incompat & !INCOMPAT_FILETYPE != 0
            || ro_compat & !(RO_COMPAT_SPARSE_SUPER | RO_COMPAT_LARGE_FILE) != 0
        {
            return Err(Error::Unsupported);
        }
        let block_size = 1024usize << log;
        let inode_size = if rev >= 1 { get16(sb, S_INODE_SIZE) } else { GOOD_OLD_INODE_SIZE };
        let first_ino = if rev >= 1 { get32(sb, S_FIRST_INO) } else { GOOD_OLD_FIRST_INO };
        let g = Geometry {
            block_size,
            first_data_block: get32(sb, S_FIRST_DATA_BLOCK),
            blocks_count: get32(sb, S_BLOCKS_COUNT),
            inodes_count: get32(sb, S_INODES_COUNT),
            blocks_per_group: get32(sb, S_BLOCKS_PER_GROUP),
            inodes_per_group: get32(sb, S_INODES_PER_GROUP),
            inode_size: inode_size as usize,
            first_ino,
            groups: 0,
            filetype: incompat & INCOMPAT_FILETYPE != 0,
            large_file: ro_compat & RO_COMPAT_LARGE_FILE != 0,
        };
        if g.blocks_per_group == 0
            || g.blocks_per_group as usize > block_size * 8
            || g.inodes_per_group == 0
            || g.inodes_per_group as usize > block_size * 8
            || !(128..=block_size).contains(&g.inode_size)
            || !g.inode_size.is_power_of_two()
            || g.first_ino <= ROOT_INO
            || g.blocks_count <= g.first_data_block
        {
            return Err(Error::Unsupported);
        }
        let groups = (g.blocks_count - g.first_data_block).div_ceil(g.blocks_per_group);
        if groups as u64 * g.inodes_per_group as u64 != g.inodes_count as u64 {
            return Err(Error::Unsupported);
        }
        Ok(Geometry { groups, ..g })
    }

    /// The first block of group `g`.
    pub fn group_first(&self, g: u32) -> u32 {
        self.first_data_block + g * self.blocks_per_group
    }

    /// Blocks in group `g` (the last one may be short).
    pub fn group_blocks(&self, g: u32) -> u32 {
        (self.blocks_count - self.group_first(g)).min(self.blocks_per_group)
    }

    pub fn group_of_block(&self, b: u32) -> u32 {
        (b.max(self.first_data_block) - self.first_data_block) / self.blocks_per_group
    }

    /// Blocks the group descriptor table takes.
    pub fn gdt_blocks(&self) -> u32 {
        (self.groups as usize * GD_SIZE).div_ceil(self.block_size) as u32
    }

    /// Block pointers in one indirect block.
    pub fn ptrs(&self) -> u64 {
        (self.block_size / 4) as u64
    }
}

/// Group `g` holds a superblock backup (and a GDT copy) under
/// `sparse_super`: groups 0, 1 and the powers of 3, 5 and 7.
pub fn has_super(g: u32, sparse: bool) -> bool {
    if !sparse || g <= 1 {
        return true;
    }
    [3u32, 5, 7].iter().any(|&p| {
        let mut n = p;
        while n < g {
            n = n.saturating_mul(p);
        }
        n == g
    })
}
