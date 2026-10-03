//! Directories: a list of variable-length entries (inode, record length,
//! name length, file type, name) filling whole blocks. A removed entry is
//! merged into the one before it, or zeroed when it starts a block.

use alloc::vec::Vec;

use crate::fs::Fs;
use crate::inode::Inode;
use crate::layout::*;
use crate::{Device, Error, Result};

/// The space an entry with an `n`-byte name needs.
fn entry_size(n: usize) -> usize {
    (8 + n + 3) & !3
}

/// One entry of a directory block.
struct Entry {
    off: usize,
    ino: u32,
    rec_len: usize,
    name_len: usize,
}

/// The entries of one directory block (stops at a malformed one).
fn entries(blk: &[u8], filetype: bool) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut off = 0;
    while off + 8 <= blk.len() {
        let rec_len = get16(blk, off + 4) as usize;
        let name_len = if filetype { blk[off + 6] as usize } else { get16(blk, off + 6) as usize };
        if rec_len < 8 || rec_len % 4 != 0 || off + rec_len > blk.len() || 8 + name_len > rec_len {
            break;
        }
        out.push(Entry { off, ino: get32(blk, off), rec_len, name_len });
        off += rec_len;
    }
    out
}

fn put_entry(blk: &mut [u8], off: usize, ino: u32, rec_len: usize, name: &[u8], ft: u8, filetype: bool) {
    put32(blk, off, ino);
    put16(blk, off + 4, rec_len as u16);
    if filetype {
        blk[off + 6] = name.len() as u8;
        blk[off + 7] = ft;
    } else {
        put16(blk, off + 6, name.len() as u16);
    }
    blk[off + 8..off + 8 + name.len()].copy_from_slice(name);
}

impl<D: Device> Fs<D> {
    /// The disk blocks of directory `dir`, in order (holes skipped).
    fn dir_blocks(&mut self, dir: &Inode) -> Result<Vec<u32>> {
        let n = dir.size.div_ceil(self.geo.block_size as u64);
        let mut node = *dir;
        let mut out = Vec::new();
        for i in 0..n {
            let b = self.bmap(&mut node, i, false)?;
            if b != 0 {
                out.push(b);
            }
        }
        Ok(out)
    }

    /// Call `f(name, ino)` for every entry of `dir` until it returns false.
    pub(crate) fn for_each_entry(&mut self, dir: &Inode, mut f: impl FnMut(&[u8], u32) -> bool) -> Result<()> {
        let ft = self.geo.filetype;
        for b in self.dir_blocks(dir)? {
            let go_on = self.cache.read(b, |blk| {
                entries(blk, ft)
                    .iter()
                    .filter(|e| e.ino != 0)
                    .all(|e| f(&blk[e.off + 8..e.off + 8 + e.name_len], e.ino))
            })?;
            if !go_on {
                break;
            }
        }
        Ok(())
    }

    /// The inode `name` names in `dir`, with its entry's file type.
    pub(crate) fn lookup(&mut self, dir: &Inode, name: &[u8]) -> Result<Option<(u32, u8)>> {
        let ft = self.geo.filetype;
        for b in self.dir_blocks(dir)? {
            let hit = self.cache.read(b, |blk| {
                entries(blk, ft).iter().find(|e| e.ino != 0 && &blk[e.off + 8..e.off + 8 + e.name_len] == name).map(
                    |e| (e.ino, if ft { blk[e.off + 7] } else { 0 }),
                )
            })?;
            if hit.is_some() {
                return Ok(hit);
            }
        }
        Ok(None)
    }

    /// Add the entry `name` -> `ino` to directory `dir_ino`, in the first
    /// gap big enough, else in a new block at its end.
    pub(crate) fn dir_add(&mut self, dir_ino: u32, name: &[u8], ino: u32, file_type: u8) -> Result<()> {
        let filetype = self.geo.filetype;
        let ft = if filetype { file_type } else { 0 };
        let need = entry_size(name.len());
        let mut dir = self.inode(dir_ino)?;
        for b in self.dir_blocks(&dir)? {
            // Look before changing: only the block that gets the entry is
            // written back.
            let fits = self.cache.read(b, |blk| {
                entries(blk, filetype)
                    .iter()
                    .any(|e| e.rec_len - if e.ino == 0 { 0 } else { entry_size(e.name_len) } >= need)
            })?;
            if !fits {
                continue;
            }
            let placed = self.cache.modify(b, |blk| {
                for e in entries(blk, filetype) {
                    let used = if e.ino == 0 { 0 } else { entry_size(e.name_len) };
                    if e.rec_len - used >= need {
                        if e.ino != 0 {
                            put16(blk, e.off + 4, used as u16);
                        }
                        put_entry(blk, e.off + used, ino, e.rec_len - used, name, ft, filetype);
                        return true;
                    }
                }
                false
            })?;
            if placed {
                // A changed hashed directory is a plain one now.
                if dir.flags & INDEX_FL != 0 {
                    dir.flags &= !INDEX_FL;
                    self.write_inode(dir_ino, &dir)?;
                }
                return Ok(());
            }
        }
        let bs = self.geo.block_size;
        let n = dir.size / bs as u64;
        let b = self.bmap(&mut dir, n, true)?;
        self.cache.modify(b, |blk| put_entry(blk, 0, ino, bs, name, ft, filetype))?;
        dir.size = (n + 1) * bs as u64;
        dir.flags &= !INDEX_FL;
        dir.mtime = self.now();
        self.write_inode(dir_ino, &dir)
    }

    /// Remove the entry `name` from directory `dir_ino`: the inode it named.
    pub(crate) fn dir_remove(&mut self, dir_ino: u32, name: &[u8]) -> Result<u32> {
        let filetype = self.geo.filetype;
        let mut dir = self.inode(dir_ino)?;
        for b in self.dir_blocks(&dir)? {
            let here = self.cache.read(b, |blk| {
                entries(blk, filetype).iter().any(|e| e.ino != 0 && &blk[e.off + 8..e.off + 8 + e.name_len] == name)
            })?;
            if !here {
                continue;
            }
            let removed = self.cache.modify(b, |blk| {
                let es = entries(blk, filetype);
                let i = es.iter().position(|e| e.ino != 0 && &blk[e.off + 8..e.off + 8 + e.name_len] == name)?;
                let ino = es[i].ino;
                if i == 0 {
                    put32(blk, es[i].off, 0);
                } else {
                    let prev = &es[i - 1];
                    put16(blk, prev.off + 4, (prev.rec_len + es[i].rec_len) as u16);
                }
                Some(ino)
            })?;
            if let Some(ino) = removed {
                dir.flags &= !INDEX_FL;
                dir.mtime = self.now();
                self.write_inode(dir_ino, &dir)?;
                return Ok(ino);
            }
        }
        Err(Error::NotFound)
    }

    /// Give the new directory `ino` its first block: `.` and `..`.
    pub(crate) fn dir_init(&mut self, ino: u32, parent: u32) -> Result<()> {
        let filetype = self.geo.filetype;
        let ft = if filetype { FT_DIR } else { 0 };
        let bs = self.geo.block_size;
        let mut node = self.inode(ino)?;
        let b = self.bmap(&mut node, 0, true)?;
        self.cache.modify(b, |blk| {
            put_entry(blk, 0, ino, 12, b".", ft, filetype);
            put_entry(blk, 12, parent, bs - 12, b"..", ft, filetype);
        })?;
        node.size = bs as u64;
        node.links = 2;
        self.write_inode(ino, &node)
    }

    /// Point the `..` of directory `ino` at `parent`.
    pub(crate) fn dir_set_parent(&mut self, ino: u32, parent: u32) -> Result<()> {
        let filetype = self.geo.filetype;
        let node = self.inode(ino)?;
        for b in self.dir_blocks(&node)? {
            let done = self.cache.modify(b, |blk| {
                match entries(blk, filetype).iter().find(|e| e.ino != 0 && &blk[e.off + 8..e.off + 8 + e.name_len] == b"..") {
                    Some(e) => {
                        put32(blk, e.off, parent);
                        true
                    }
                    None => false,
                }
            })?;
            if done {
                return Ok(());
            }
        }
        Err(Error::Invalid)
    }

    /// Whether directory `dir` holds nothing but `.` and `..`.
    pub(crate) fn dir_is_empty(&mut self, dir: &Inode) -> Result<bool> {
        let mut empty = true;
        self.for_each_entry(dir, |name, _| {
            empty = name == b"." || name == b"..";
            empty
        })?;
        Ok(empty)
    }
}
