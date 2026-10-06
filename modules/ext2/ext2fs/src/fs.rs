//! A mounted filesystem: the superblock and group descriptors in memory,
//! block and inode allocation, and the public operations.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::cache::Cache;
use crate::inode::Inode;
use crate::layout::*;
use crate::{Device, Error, Kind, Result, Stat};

pub struct Fs<D: Device> {
    pub(crate) cache: Cache<D>,
    pub(crate) geo: Geometry,
    /// The primary superblock (1024 bytes at byte 1024).
    sb: Vec<u8>,
    sb_dirty: bool,
    /// The group descriptor table, and which of its blocks changed.
    gdt: Vec<u8>,
    gdt_dirty: Vec<bool>,
    /// Just after the last block allocated, where the next search starts.
    last_alloc: u32,
    /// Paths resolved since the tree last changed, with their inodes: a
    /// file's every read names it, and a lookup scans whole directories.
    resolved: BTreeMap<String, u32>,
}

/// Most paths [`Fs::resolve`] remembers (it forgets them all past that).
const RESOLVED_MAX: usize = 1024;

impl<D: Device> Fs<D> {
    /// Mount the filesystem on `dev`.
    pub fn mount(mut dev: D) -> Result<Self> {
        let mut sb = vec![0u8; SB_SIZE];
        if !dev.read(SB_OFFSET, &mut sb) {
            return Err(Error::Io);
        }
        let geo = Geometry::parse(&sb)?;
        let gdt_blocks = geo.gdt_blocks();
        let mut gdt = vec![0u8; gdt_blocks as usize * geo.block_size];
        if !dev.read((geo.first_data_block as u64 + 1) * geo.block_size as u64, &mut gdt) {
            return Err(Error::Io);
        }
        let fs = Fs {
            cache: Cache::new(dev, geo.block_size),
            geo,
            sb,
            sb_dirty: false,
            gdt,
            gdt_dirty: vec![false; gdt_blocks as usize],
            last_alloc: 0,
            resolved: BTreeMap::new(),
        };
        for g in 0..geo.groups {
            let (bb, ib, it) = (fs.gd32(g, G_BLOCK_BITMAP), fs.gd32(g, G_INODE_BITMAP), fs.gd32(g, G_INODE_TABLE));
            if [bb, ib, it].iter().any(|&b| b < geo.first_data_block || b >= geo.blocks_count) {
                return Err(Error::Unsupported);
            }
        }
        Ok(fs)
    }

    pub fn device(&self) -> &D {
        &self.cache.dev
    }

    /// The device back (after a final flush).
    pub fn unmount(mut self) -> Result<D> {
        self.flush()?;
        Ok(self.cache.dev)
    }

    // ---- superblock and group descriptors --------------------------------

    pub(crate) fn gd32(&self, g: u32, field: usize) -> u32 {
        get32(&self.gdt, g as usize * GD_SIZE + field)
    }

    fn gd16(&self, g: u32, field: usize) -> u16 {
        get16(&self.gdt, g as usize * GD_SIZE + field)
    }

    fn gd16_add(&mut self, g: u32, field: usize, delta: i32) {
        let o = g as usize * GD_SIZE + field;
        let v = (get16(&self.gdt, o) as i32 + delta) as u16;
        put16(&mut self.gdt, o, v);
        self.gdt_dirty[o / self.geo.block_size] = true;
    }

    fn sb32_add(&mut self, field: usize, delta: i32) {
        let v = (get32(&self.sb, field) as i64 + delta as i64) as u32;
        put32(&mut self.sb, field, v);
        self.sb_dirty = true;
    }

    /// Write every change: cached blocks, then the descriptors and the
    /// superblock that count them.
    pub(crate) fn flush(&mut self) -> Result<()> {
        self.cache.flush()?;
        let bs = self.geo.block_size;
        for (i, dirty) in self.gdt_dirty.iter_mut().enumerate() {
            if *dirty {
                let off = (self.geo.first_data_block as u64 + 1 + i as u64) * bs as u64;
                if !self.cache.dev.write(off, &self.gdt[i * bs..(i + 1) * bs]) {
                    return Err(Error::Io);
                }
                *dirty = false;
            }
        }
        if self.sb_dirty {
            let now = self.cache.dev.now();
            put32(&mut self.sb, S_WTIME, now);
            if !self.cache.dev.write(SB_OFFSET, &self.sb) {
                return Err(Error::Io);
            }
            self.sb_dirty = false;
        }
        Ok(())
    }

    /// Write every change to the device: what [`Fs::write`] left in the
    /// cache, and the counts in the descriptors and the superblock.
    pub fn sync(&mut self) -> Result<()> {
        self.flush()
    }

    /// Run a public operation that changes the tree and flush what it
    /// changed, even if it failed half way (what it did is consistent on
    /// disk). The paths resolved before may name other inodes now.
    fn op<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let r = f(self);
        self.resolved.clear();
        let flushed = self.flush();
        let v = r?;
        flushed?;
        Ok(v)
    }

    /// Run a public operation that reads, or only writes file data: it
    /// stays in the cache until the next flush (a metadata change, `sync`,
    /// an eviction or `unmount`).
    fn run<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        f(self)
    }

    pub(crate) fn now(&mut self) -> u32 {
        self.cache.dev.now()
    }

    // ---- allocation ------------------------------------------------------

    /// Find and set a clear bit in the bitmap `block`, among its first
    /// `bits`, from `start` on and then from 0.
    fn take_bit(&mut self, block: u32, bits: u32, start: u32) -> Result<Option<u32>> {
        self.cache.modify(block, |bm| {
            let free = |bm: &[u8], i: u32| bm[(i / 8) as usize] & (1 << (i % 8)) == 0;
            let mut found = None;
            for (lo, hi) in [(start.min(bits), bits), (0, start.min(bits))] {
                let mut i = lo;
                while i < hi {
                    if i % 8 == 0 && i + 8 <= hi && bm[(i / 8) as usize] == 0xff {
                        i += 8;
                        continue;
                    }
                    if free(bm, i) {
                        found = Some(i);
                        break;
                    }
                    i += 1;
                }
                if found.is_some() {
                    break;
                }
            }
            if let Some(i) = found {
                bm[(i / 8) as usize] |= 1 << (i % 8);
            }
            found
        })
    }

    fn clear_bit(&mut self, block: u32, i: u32) -> Result<bool> {
        self.cache.modify(block, |bm| {
            let was = bm[(i / 8) as usize] & (1 << (i % 8)) != 0;
            bm[(i / 8) as usize] &= !(1 << (i % 8));
            was
        })
    }

    /// Allocate a block, zeroed: the first free one after the previous
    /// allocation (so a file written front to back is contiguous).
    pub(crate) fn alloc_block(&mut self) -> Result<u32> {
        let goal = self.last_alloc;
        let g0 = self.geo.group_of_block(goal).min(self.geo.groups - 1);
        for k in 0..self.geo.groups {
            let g = (g0 + k) % self.geo.groups;
            if self.gd16(g, G_FREE_BLOCKS) == 0 {
                continue;
            }
            let first = self.geo.group_first(g);
            let start = if k == 0 { goal.saturating_sub(first) } else { 0 };
            let bitmap = self.gd32(g, G_BLOCK_BITMAP);
            if let Some(i) = self.take_bit(bitmap, self.geo.group_blocks(g), start)? {
                self.gd16_add(g, G_FREE_BLOCKS, -1);
                self.sb32_add(S_FREE_BLOCKS, -1);
                let b = first + i;
                self.cache.zero(b)?;
                self.last_alloc = b + 1;
                return Ok(b);
            }
        }
        Err(Error::NoSpace)
    }

    pub(crate) fn free_block(&mut self, b: u32) -> Result<()> {
        if b < self.geo.first_data_block || b >= self.geo.blocks_count {
            return Err(Error::Invalid);
        }
        let g = self.geo.group_of_block(b);
        let bitmap = self.gd32(g, G_BLOCK_BITMAP);
        if self.clear_bit(bitmap, b - self.geo.group_first(g))? {
            self.gd16_add(g, G_FREE_BLOCKS, 1);
            self.sb32_add(S_FREE_BLOCKS, 1);
        }
        Ok(())
    }

    /// Allocate an inode, preferably in the group of `near` (the parent
    /// directory).
    pub(crate) fn alloc_inode(&mut self, near: u32, dir: bool) -> Result<u32> {
        let ipg = self.geo.inodes_per_group;
        let g0 = (near.max(1) - 1) / ipg;
        for k in 0..self.geo.groups {
            let g = (g0 + k) % self.geo.groups;
            if self.gd16(g, G_FREE_INODES) == 0 {
                continue;
            }
            // Group 0 starts with the reserved inodes.
            let start = if g == 0 { self.geo.first_ino - 1 } else { 0 };
            let bitmap = self.gd32(g, G_INODE_BITMAP);
            let Some(i) = self.take_bit(bitmap, ipg, start)? else { continue };
            if g == 0 && i < start {
                // Only reserved ones were left; give the bit back.
                self.clear_bit(bitmap, i)?;
                continue;
            }
            self.gd16_add(g, G_FREE_INODES, -1);
            self.sb32_add(S_FREE_INODES, -1);
            if dir {
                self.gd16_add(g, G_USED_DIRS, 1);
            }
            return Ok(g * ipg + i + 1);
        }
        Err(Error::NoSpace)
    }

    pub(crate) fn free_inode(&mut self, ino: u32, dir: bool) -> Result<()> {
        let ipg = self.geo.inodes_per_group;
        let (g, i) = ((ino - 1) / ipg, (ino - 1) % ipg);
        let bitmap = self.gd32(g, G_INODE_BITMAP);
        if self.clear_bit(bitmap, i)? {
            self.gd16_add(g, G_FREE_INODES, 1);
            self.sb32_add(S_FREE_INODES, 1);
            if dir {
                self.gd16_add(g, G_USED_DIRS, -1);
            }
        }
        Ok(())
    }

    // ---- paths -----------------------------------------------------------

    /// The inode at `path` (symlinks are not followed).
    pub(crate) fn resolve(&mut self, path: &str) -> Result<u32> {
        if let Some(&ino) = self.resolved.get(path) {
            return Ok(ino);
        }
        let ino = self.walk(path)?;
        if self.resolved.len() >= RESOLVED_MAX {
            self.resolved.clear();
        }
        self.resolved.insert(String::from(path), ino);
        Ok(ino)
    }

    fn walk(&mut self, path: &str) -> Result<u32> {
        let mut ino = ROOT_INO;
        for name in path.split('/').filter(|c| !c.is_empty() && *c != ".") {
            let dir = self.inode(ino)?;
            if !dir.is_dir() {
                return Err(Error::NotDir);
            }
            ino = self.lookup(&dir, name.as_bytes())?.ok_or(Error::NotFound)?.0;
        }
        Ok(ino)
    }

    fn inode_at_path(&mut self, path: &str) -> Result<Inode> {
        let ino = self.resolve(path)?;
        self.inode(ino)
    }

    /// The directory holding `path` and the last component's name.
    fn parent<'p>(&mut self, path: &'p str) -> Result<(u32, Inode, &'p str)> {
        let path = path.trim_end_matches('/');
        let (dir, name) = match path.rfind('/') {
            Some(i) => (&path[..i], &path[i + 1..]),
            None => ("", path),
        };
        if name.is_empty() || name == "." || name == ".." || name.len() > 255 {
            return Err(Error::Invalid);
        }
        let ino = self.resolve(dir)?;
        let node = self.inode(ino)?;
        if !node.is_dir() {
            return Err(Error::NotDir);
        }
        Ok((ino, node, name))
    }

    /// A new inode of `mode` named `name` in directory `dir_ino`.
    fn make(&mut self, dir_ino: u32, name: &str, mode: u16) -> Result<(u32, Inode)> {
        let is_dir = mode & S_IFMT == S_IFDIR;
        let ino = self.alloc_inode(dir_ino, is_dir)?;
        let now = self.now();
        let node = Inode::new(mode, now);
        self.write_new_inode(ino, &node)?;
        self.dir_add(dir_ino, name.as_bytes(), ino, node.file_type())?;
        Ok((ino, node))
    }

    /// Drop one link to the non-directory `ino`; free it with its last.
    fn drop_link(&mut self, ino: u32) -> Result<()> {
        let mut node = self.inode(ino)?;
        node.links = node.links.saturating_sub(1);
        if node.links == 0 {
            self.free_data(&mut node)?;
            node.dtime = self.now().max(1);
            self.write_inode(ino, &node)?;
            self.free_inode(ino, false)
        } else {
            self.write_inode(ino, &node)
        }
    }

    // ---- the public operations --------------------------------------------

    pub fn stat(&mut self, path: &str) -> Result<Stat> {
        self.run(|fs| {
            let ino = fs.resolve(path)?;
            let n = fs.inode(ino)?;
            Ok(Stat { kind: n.kind(), mode: n.mode, size: n.size, ino, links: n.links, mtime: n.mtime, atime: n.atime })
        })
    }

    /// Set the access and modification times of `path` (seconds since the
    /// epoch; `None` keeps one).
    pub fn set_times(&mut self, path: &str, atime: Option<u32>, mtime: Option<u32>) -> Result<()> {
        self.op(|fs| {
            let ino = fs.resolve(path)?;
            let mut node = fs.inode(ino)?;
            if let Some(t) = atime {
                node.atime = t;
            }
            if let Some(t) = mtime {
                node.mtime = t;
            }
            fs.write_inode(ino, &node)
        })
    }

    /// Call `f` with each name in the directory `path` (not `.` and `..`)
    /// until it returns false.
    pub fn list(&mut self, path: &str, mut f: impl FnMut(&[u8]) -> bool) -> Result<()> {
        self.run(|fs| {
            let dir = fs.inode_at_path(path)?;
            if !dir.is_dir() {
                return Err(Error::NotDir);
            }
            fs.for_each_entry(&dir, |name, _| name == b"." || name == b".." || f(name))
        })
    }

    /// Read the file `path` at `pos` into `out`: the bytes read (0 at the end).
    pub fn read(&mut self, path: &str, pos: u64, out: &mut [u8]) -> Result<usize> {
        self.run(|fs| {
            let node = fs.inode_at_path(path)?;
            match node.kind() {
                Kind::Dir => Err(Error::IsDir),
                Kind::File => fs.read_data(&node, pos, out),
                _ => Err(Error::Invalid),
            }
        })
    }

    /// Write `buf` at `pos` of the file `path` (a gap before `pos` reads as
    /// zeros): the bytes written.
    pub fn write(&mut self, path: &str, pos: u64, buf: &[u8]) -> Result<usize> {
        self.run(|fs| {
            let ino = fs.resolve(path)?;
            fs.write_at(ino, pos, buf)
        })
    }

    fn write_at(&mut self, ino: u32, pos: u64, buf: &[u8]) -> Result<usize> {
        let mut node = self.inode(ino)?;
        match node.kind() {
            Kind::Dir => return Err(Error::IsDir),
            Kind::File => {}
            _ => return Err(Error::Invalid),
        }
        let n = self.write_data(&mut node, pos, buf)?;
        node.mtime = self.now();
        self.write_inode(ino, &node)?;
        Ok(n)
    }

    /// Create the empty regular file `path` (fine if one is there already).
    pub fn create(&mut self, path: &str) -> Result<()> {
        self.op(|fs| {
            let (dir_ino, dir, name) = fs.parent(path)?;
            if let Some((ino, _)) = fs.lookup(&dir, name.as_bytes())? {
                return match fs.inode(ino)?.kind() {
                    Kind::File => Ok(()),
                    Kind::Dir => Err(Error::IsDir),
                    _ => Err(Error::Exists),
                };
            }
            fs.make(dir_ino, name, S_IFREG | 0o755).map(|_| ())
        })
    }

    /// Cut the file `path` to length 0.
    pub fn truncate(&mut self, path: &str) -> Result<()> {
        self.op(|fs| {
            let ino = fs.resolve(path)?;
            let mut node = fs.inode(ino)?;
            if node.kind() != Kind::File {
                return Err(Error::Invalid);
            }
            fs.free_data(&mut node)?;
            node.mtime = fs.now();
            fs.write_inode(ino, &node)
        })
    }

    /// Make the file `path` `size` bytes long: cut, or grown with zeros.
    pub fn set_size(&mut self, path: &str, size: u64) -> Result<()> {
        self.op(|fs| {
            let ino = fs.resolve(path)?;
            fs.resize(ino, size)
        })
    }

    /// [`Fs::set_size`] of the file with inode `ino`.
    pub fn set_size_ino(&mut self, ino: u32, size: u64) -> Result<()> {
        self.op(|fs| fs.resize(ino, size))
    }

    fn resize(&mut self, ino: u32, size: u64) -> Result<()> {
        let mut node = self.inode(ino)?;
        match node.kind() {
            Kind::File => {}
            Kind::Dir => return Err(Error::IsDir),
            _ => return Err(Error::Invalid),
        }
        self.resize_data(&mut node, size)?;
        node.mtime = self.now();
        self.write_inode(ino, &node)
    }

    pub fn mkdir(&mut self, path: &str) -> Result<()> {
        self.op(|fs| {
            let (dir_ino, dir, name) = fs.parent(path)?;
            if fs.lookup(&dir, name.as_bytes())?.is_some() {
                return Err(Error::Exists);
            }
            let (ino, _) = fs.make(dir_ino, name, S_IFDIR | 0o755)?;
            fs.dir_init(ino, dir_ino)?;
            let mut dir = fs.inode(dir_ino)?;
            dir.links += 1;
            fs.write_inode(dir_ino, &dir)
        })
    }

    pub fn rmdir(&mut self, path: &str) -> Result<()> {
        self.op(|fs| {
            let (dir_ino, dir, name) = fs.parent(path)?;
            let (ino, _) = fs.lookup(&dir, name.as_bytes())?.ok_or(Error::NotFound)?;
            let mut node = fs.inode(ino)?;
            if !node.is_dir() {
                return Err(Error::NotDir);
            }
            if !fs.dir_is_empty(&node)? {
                return Err(Error::NotEmpty);
            }
            fs.dir_remove(dir_ino, name.as_bytes())?;
            fs.free_data(&mut node)?;
            node.links = 0;
            node.dtime = fs.now().max(1);
            fs.write_inode(ino, &node)?;
            fs.free_inode(ino, true)?;
            let mut dir = fs.inode(dir_ino)?;
            dir.links -= 1;
            fs.write_inode(dir_ino, &dir)
        })
    }

    /// Remove `path` (not a directory).
    pub fn unlink(&mut self, path: &str) -> Result<()> {
        self.op(|fs| {
            let (dir_ino, dir, name) = fs.parent(path)?;
            let (ino, _) = fs.lookup(&dir, name.as_bytes())?.ok_or(Error::NotFound)?;
            if fs.inode(ino)?.is_dir() {
                return Err(Error::IsDir);
            }
            fs.dir_remove(dir_ino, name.as_bytes())?;
            fs.drop_link(ino)
        })
    }

    /// Remove the name `path` (not a directory) but keep its inode for
    /// whoever still holds the file: its number, for the `*_ino` calls
    /// until [`Fs::forget`]. An inode no name links and nothing forgets
    /// (the system stopped first) is lost space `e2fsck` gives back.
    pub fn unlink_keep(&mut self, path: &str) -> Result<u32> {
        self.op(|fs| {
            let (dir_ino, dir, name) = fs.parent(path)?;
            let (ino, _) = fs.lookup(&dir, name.as_bytes())?.ok_or(Error::NotFound)?;
            let mut node = fs.inode(ino)?;
            if node.is_dir() {
                return Err(Error::IsDir);
            }
            fs.dir_remove(dir_ino, name.as_bytes())?;
            node.links = node.links.saturating_sub(1);
            fs.write_inode(ino, &node)?;
            Ok(ino)
        })
    }

    /// The inode [`Fs::unlink_keep`] kept is let go: freed when no name
    /// links it any more.
    pub fn forget(&mut self, ino: u32) -> Result<()> {
        self.op(|fs| {
            let mut node = fs.inode(ino)?;
            if node.links > 0 || node.is_dir() {
                return Ok(());
            }
            fs.free_data(&mut node)?;
            node.dtime = fs.now().max(1);
            fs.write_inode(ino, &node)?;
            fs.free_inode(ino, false)
        })
    }

    /// [`Fs::stat`] of the inode `ino`.
    pub fn stat_ino(&mut self, ino: u32) -> Result<Stat> {
        self.run(|fs| {
            let n = fs.inode(ino)?;
            Ok(Stat { kind: n.kind(), mode: n.mode, size: n.size, ino, links: n.links, mtime: n.mtime, atime: n.atime })
        })
    }

    /// [`Fs::read`] of the file with inode `ino`.
    pub fn read_ino(&mut self, ino: u32, pos: u64, out: &mut [u8]) -> Result<usize> {
        self.run(|fs| {
            let node = fs.inode(ino)?;
            match node.kind() {
                Kind::File => fs.read_data(&node, pos, out),
                Kind::Dir => Err(Error::IsDir),
                _ => Err(Error::Invalid),
            }
        })
    }

    /// [`Fs::write`] to the file with inode `ino`.
    pub fn write_ino(&mut self, ino: u32, pos: u64, buf: &[u8]) -> Result<usize> {
        self.run(|fs| fs.write_at(ino, pos, buf))
    }

    /// Move `old` to `new`, replacing what `new` names (an empty directory
    /// by a directory, anything else by a non-directory).
    pub fn rename(&mut self, old: &str, new: &str) -> Result<()> {
        self.op(|fs| {
            let (old_dir, odir, old_name) = fs.parent(old)?;
            let (ino, ft) = fs.lookup(&odir, old_name.as_bytes())?.ok_or(Error::NotFound)?;
            let (new_dir, ndir, new_name) = fs.parent(new)?;
            let is_dir = fs.inode(ino)?.is_dir();
            if is_dir && fs.is_within(new_dir, ino)? {
                return Err(Error::Invalid);
            }
            if let Some((target, _)) = fs.lookup(&ndir, new_name.as_bytes())? {
                if target == ino {
                    return Ok(());
                }
                let mut t = fs.inode(target)?;
                match (is_dir, t.is_dir()) {
                    (false, true) => return Err(Error::IsDir),
                    (true, false) => return Err(Error::NotDir),
                    (true, true) if !fs.dir_is_empty(&t)? => return Err(Error::NotEmpty),
                    _ => {}
                }
                fs.dir_remove(new_dir, new_name.as_bytes())?;
                if t.is_dir() {
                    fs.free_data(&mut t)?;
                    t.links = 0;
                    t.dtime = fs.now().max(1);
                    fs.write_inode(target, &t)?;
                    fs.free_inode(target, true)?;
                    let mut nd = fs.inode(new_dir)?;
                    nd.links -= 1;
                    fs.write_inode(new_dir, &nd)?;
                } else {
                    fs.drop_link(target)?;
                }
            }
            fs.dir_add(new_dir, new_name.as_bytes(), ino, ft)?;
            fs.dir_remove(old_dir, old_name.as_bytes())?;
            if is_dir && old_dir != new_dir {
                fs.dir_set_parent(ino, new_dir)?;
                let mut od = fs.inode(old_dir)?;
                od.links -= 1;
                fs.write_inode(old_dir, &od)?;
                let mut nd = fs.inode(new_dir)?;
                nd.links += 1;
                fs.write_inode(new_dir, &nd)?;
            }
            Ok(())
        })
    }

    /// Whether directory `dir` is `ancestor` or below it.
    fn is_within(&mut self, mut dir: u32, ancestor: u32) -> Result<bool> {
        for _ in 0..4096 {
            if dir == ancestor {
                return Ok(true);
            }
            if dir == ROOT_INO {
                return Ok(false);
            }
            let node = self.inode(dir)?;
            dir = self.lookup(&node, b"..")?.ok_or(Error::Invalid)?.0;
        }
        Err(Error::Invalid)
    }

    /// Make `link` a symlink to `target`.
    pub fn symlink(&mut self, target: &str, link: &str) -> Result<()> {
        self.op(|fs| {
            if target.is_empty() || target.len() >= fs.geo.block_size {
                return Err(Error::Invalid);
            }
            let (dir_ino, dir, name) = fs.parent(link)?;
            if fs.lookup(&dir, name.as_bytes())?.is_some() {
                return Err(Error::Exists);
            }
            let (ino, mut node) = fs.make(dir_ino, name, S_IFLNK | 0o777)?;
            if target.len() < node.inline_capacity() {
                // A fast symlink: the target in place of the block map.
                node.set_inline(target.as_bytes());
            } else {
                fs.write_data(&mut node, 0, target.as_bytes())?;
            }
            fs.write_inode(ino, &node)
        })
    }

    /// The target of the symlink `path` into `buf`: its length.
    pub fn readlink(&mut self, path: &str, buf: &mut [u8]) -> Result<usize> {
        self.run(|fs| {
            let node = fs.inode_at_path(path)?;
            if node.kind() != Kind::Symlink {
                return Err(Error::Invalid);
            }
            let len = (node.size as usize).min(buf.len());
            if node.is_fast_symlink(fs.geo.block_size) {
                buf[..len].copy_from_slice(&node.inline()[..len]);
                Ok(len)
            } else {
                fs.read_data(&node, 0, &mut buf[..len])
            }
        })
    }
}
