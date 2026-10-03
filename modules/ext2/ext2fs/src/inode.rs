//! Inodes: reading and writing them, and the block map from file offsets to
//! disk blocks (12 direct pointers, then single, double and triple indirect
//! blocks).

use alloc::vec::Vec;

use crate::fs::Fs;
use crate::layout::*;
use crate::{Device, Error, Kind, Result};

/// The block map's pointer slots in `i_block`.
const NDIRECT: usize = 12;
const IND: usize = 12;
const NBLOCKS: usize = 15;

/// The inode fields this code uses (the rest of the on-disk inode is kept
/// as it is when one is written back).
#[derive(Clone, Copy, Debug)]
pub struct Inode {
    pub mode: u16,
    pub size: u64,
    pub atime: u32,
    pub ctime: u32,
    pub mtime: u32,
    pub dtime: u32,
    pub links: u16,
    /// 512-byte sectors in use, indirect blocks included.
    pub sectors: u32,
    pub flags: u32,
    pub block: [u32; NBLOCKS],
    pub file_acl: u32,
}

impl Inode {
    pub fn new(mode: u16, now: u32) -> Self {
        Inode {
            mode,
            size: 0,
            atime: now,
            ctime: now,
            mtime: now,
            dtime: 0,
            links: 1,
            sectors: 0,
            flags: 0,
            block: [0; NBLOCKS],
            file_acl: 0,
        }
    }

    fn parse(b: &[u8]) -> Self {
        let mode = get16(b, 0);
        let mut block = [0u32; NBLOCKS];
        for (i, p) in block.iter_mut().enumerate() {
            *p = get32(b, 40 + i * 4);
        }
        // `i_size_high` (offset 108) is the size's top half for regular
        // files; for directories it is the old `i_dir_acl`.
        let high = if mode & S_IFMT == S_IFREG { get32(b, 108) as u64 } else { 0 };
        Inode {
            mode,
            size: get32(b, 4) as u64 | high << 32,
            atime: get32(b, 8),
            ctime: get32(b, 12),
            mtime: get32(b, 16),
            dtime: get32(b, 20),
            links: get16(b, 26),
            sectors: get32(b, 28),
            flags: get32(b, 32),
            block,
            file_acl: get32(b, 104),
        }
    }

    fn store(&self, b: &mut [u8]) {
        put16(b, 0, self.mode);
        put32(b, 4, self.size as u32);
        put32(b, 8, self.atime);
        put32(b, 12, self.ctime);
        put32(b, 16, self.mtime);
        put32(b, 20, self.dtime);
        put16(b, 26, self.links);
        put32(b, 28, self.sectors);
        put32(b, 32, self.flags);
        for (i, p) in self.block.iter().enumerate() {
            put32(b, 40 + i * 4, *p);
        }
        put32(b, 104, self.file_acl);
        if self.kind() == Kind::File {
            put32(b, 108, (self.size >> 32) as u32);
        }
    }

    pub fn kind(&self) -> Kind {
        match self.mode & S_IFMT {
            S_IFREG => Kind::File,
            S_IFDIR => Kind::Dir,
            S_IFLNK => Kind::Symlink,
            _ => Kind::Other,
        }
    }

    pub fn is_dir(&self) -> bool {
        self.kind() == Kind::Dir
    }

    /// The directory entry type for this inode.
    pub fn file_type(&self) -> u8 {
        match self.kind() {
            Kind::Dir => FT_DIR,
            Kind::Symlink => FT_SYMLINK,
            _ => FT_REG,
        }
    }

    /// A fast symlink keeps its target in `i_block`.
    pub fn inline_capacity(&self) -> usize {
        NBLOCKS * 4
    }

    pub fn is_fast_symlink(&self, block_size: usize) -> bool {
        // As Linux decides: no data blocks other than an xattr block.
        let acl = if self.file_acl != 0 { (block_size / 512) as u32 } else { 0 };
        self.kind() == Kind::Symlink && self.sectors == acl
    }

    pub fn inline(&self) -> [u8; NBLOCKS * 4] {
        let mut out = [0u8; NBLOCKS * 4];
        for (i, p) in self.block.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&p.to_le_bytes());
        }
        out
    }

    pub fn set_inline(&mut self, data: &[u8]) {
        let mut raw = [0u8; NBLOCKS * 4];
        raw[..data.len()].copy_from_slice(data);
        for (i, p) in self.block.iter_mut().enumerate() {
            *p = get32(&raw, i * 4);
        }
        self.size = data.len() as u64;
    }
}

impl<D: Device> Fs<D> {
    /// Where inode `ino` is: its table block and the offset in it.
    fn inode_at(&self, ino: u32) -> Result<(u32, usize)> {
        if ino == 0 || ino > self.geo.inodes_count {
            return Err(Error::Invalid);
        }
        let ipg = self.geo.inodes_per_group;
        let (g, i) = ((ino - 1) / ipg, (ino - 1) % ipg);
        let byte = i as usize * self.geo.inode_size;
        let table = self.gd32(g, G_INODE_TABLE);
        Ok((table + (byte / self.geo.block_size) as u32, byte % self.geo.block_size))
    }

    pub(crate) fn inode(&mut self, ino: u32) -> Result<Inode> {
        let (block, off) = self.inode_at(ino)?;
        self.cache.read(block, |b| Inode::parse(&b[off..off + GOOD_OLD_INODE_SIZE as usize]))
    }

    pub(crate) fn write_inode(&mut self, ino: u32, node: &Inode) -> Result<()> {
        let (block, off) = self.inode_at(ino)?;
        self.cache.modify(block, |b| node.store(&mut b[off..off + GOOD_OLD_INODE_SIZE as usize]))
    }

    /// Write a newly allocated inode over whatever the slot held.
    pub(crate) fn write_new_inode(&mut self, ino: u32, node: &Inode) -> Result<()> {
        let (block, off) = self.inode_at(ino)?;
        let size = self.geo.inode_size;
        self.cache.modify(block, |b| {
            b[off..off + size].fill(0);
            node.store(&mut b[off..off + size]);
        })
    }

    /// The pointer slot path to logical block `n`: the `i_block` index and
    /// the indexes into each level of indirect blocks below it.
    fn map_path(&self, n: u64) -> Result<(usize, Vec<usize>)> {
        let p = self.geo.ptrs();
        if n < NDIRECT as u64 {
            return Ok((n as usize, Vec::new()));
        }
        let mut rest = n - NDIRECT as u64;
        let mut span = p;
        for level in 0..3 {
            if rest < span {
                let mut idx = Vec::with_capacity(level + 1);
                let mut div = span / p;
                for _ in 0..=level {
                    idx.push((rest / div % p) as usize);
                    div /= p;
                }
                return Ok((IND + level, idx));
            }
            rest -= span;
            span *= p;
        }
        Err(Error::TooBig)
    }

    /// The disk block of logical block `n` of `node`: 0 for a hole, unless
    /// `alloc`, which fills the hole (and any missing indirect block) with a
    /// zeroed new block.
    pub(crate) fn bmap(&mut self, node: &mut Inode, n: u64, alloc: bool) -> Result<u32> {
        let (slot, path) = self.map_path(n)?;
        let per = (self.geo.block_size / 512) as u32;
        if node.block[slot] == 0 {
            if !alloc {
                return Ok(0);
            }
            node.block[slot] = self.alloc_block()?;
            node.sectors += per;
        }
        let mut cur = node.block[slot];
        for i in path {
            let next = self.cache.read(cur, |b| get32(b, i * 4))?;
            if next != 0 {
                cur = next;
                continue;
            }
            if !alloc {
                return Ok(0);
            }
            let b = self.alloc_block()?;
            self.cache.modify(cur, |blk| put32(blk, i * 4, b))?;
            node.sectors += per;
            cur = b;
        }
        Ok(cur)
    }

    pub(crate) fn read_data(&mut self, node: &Inode, pos: u64, out: &mut [u8]) -> Result<usize> {
        if pos >= node.size {
            return Ok(0);
        }
        let want = (out.len() as u64).min(node.size - pos) as usize;
        let bs = self.geo.block_size;
        let mut node = *node;
        let mut done = 0;
        while done < want {
            let at = pos + done as u64;
            let (n, into) = (at / bs as u64, (at % bs as u64) as usize);
            let take = (bs - into).min(want - done);
            let b = self.bmap(&mut node, n, false)?;
            let dst = &mut out[done..done + take];
            if b == 0 {
                dst.fill(0);
            } else {
                self.cache.read(b, |blk| dst.copy_from_slice(&blk[into..into + take]))?;
            }
            done += take;
        }
        Ok(done)
    }

    /// Write `buf` at `pos`, growing the file; the caller writes the inode.
    pub(crate) fn write_data(&mut self, node: &mut Inode, pos: u64, buf: &[u8]) -> Result<usize> {
        let end = pos.checked_add(buf.len() as u64).ok_or(Error::TooBig)?;
        if end > u32::MAX as u64 && !(self.geo.large_file && node.kind() == Kind::File) {
            return Err(Error::TooBig);
        }
        let bs = self.geo.block_size;
        let mut done = 0;
        while done < buf.len() {
            let at = pos + done as u64;
            let (n, into) = (at / bs as u64, (at % bs as u64) as usize);
            let take = (bs - into).min(buf.len() - done);
            let b = match self.bmap(node, n, true) {
                Ok(b) => b,
                // Keep what was written: the size covers it.
                Err(e) if done == 0 => return Err(e),
                Err(_) => break,
            };
            self.cache.modify(b, |blk| blk[into..into + take].copy_from_slice(&buf[done..done + take]))?;
            done += take;
            node.size = node.size.max(pos + done as u64);
        }
        Ok(done)
    }

    /// Free every block of `node` (data and indirect) and set its size to 0;
    /// the caller writes the inode.
    pub(crate) fn free_data(&mut self, node: &mut Inode) -> Result<()> {
        let fast = node.is_fast_symlink(self.geo.block_size);
        if !fast {
            for slot in 0..NBLOCKS {
                let depth = slot.saturating_sub(NDIRECT - 1);
                if node.block[slot] != 0 {
                    self.free_tree(node.block[slot], depth)?;
                }
                node.block[slot] = 0;
            }
        } else {
            node.block = [0; NBLOCKS];
        }
        node.sectors = if node.file_acl != 0 { (self.geo.block_size / 512) as u32 } else { 0 };
        node.size = 0;
        Ok(())
    }

    /// Free block `b` and, `depth` levels down, the blocks it points to.
    fn free_tree(&mut self, b: u32, depth: usize) -> Result<()> {
        if depth > 0 {
            let ptrs: Vec<u32> = self.cache.read(b, |blk| {
                blk.chunks_exact(4).map(|c| get32(c, 0)).filter(|&p| p != 0).collect()
            })?;
            for p in ptrs {
                self.free_tree(p, depth - 1)?;
            }
        }
        self.free_block(b)
    }
}
