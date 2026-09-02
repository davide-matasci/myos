//! In-kernel writable ext2 (rev1, 1024-byte blocks, one block group).
//!
//! Bound via `mount(2)` to a block device after userspace `mkfs.ext2`.
//! Direct blocks only; no journal, extents, or INCOMPAT_FILETYPE.

use spin::Mutex;

use crate::blk;
use crate::fs::StatInfo;
use crate::fs::vfs::MountOps;

const BLK: usize = 1024;
const INODE_SIZE: usize = 128;
const MAGIC: u16 = 0xEF53;
const ROOT_INO: u32 = 2;
const NDIRECT: usize = 12;

const S_IFMT: u16 = 0o170000;
const S_IFDIR: u16 = 0o040000;
const S_IFREG: u16 = 0o100000;
const S_IFDIR_MODE: u16 = S_IFDIR | 0o755;
const S_IFREG_MODE: u16 = S_IFREG | 0o644;

const SB_OFF: u64 = 1024;

#[derive(Clone, Copy)]
struct Super {
    inodes_count: u32,
    blocks_count: u32,
    first_ino: u32,
    inode_size: u16,
}

#[derive(Clone, Copy)]
struct Group {
    block_bitmap: u32,
    inode_bitmap: u32,
    inode_table: u32,
}

#[derive(Clone, Copy)]
struct Ext2 {
    dev: u32,
    sb: Super,
    gd: Group,
}

#[derive(Clone, Copy)]
struct Inode {
    mode: u16,
    size: u32,
    links: u16,
    blocks: u32,
    direct: [u32; NDIRECT],
}

static FS: Mutex<Option<Ext2>> = Mutex::new(None);

fn le_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn put_u16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

fn put_u32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

fn read_block(dev: u32, block: u32, buf: &mut [u8; BLK]) -> bool {
    blk::read_bytes(dev, block as u64 * BLK as u64, buf)
        .map(|n| n == BLK)
        .unwrap_or(false)
}

fn write_block(dev: u32, block: u32, buf: &[u8; BLK]) -> bool {
    blk::write_bytes(dev, block as u64 * BLK as u64, buf)
        .map(|n| n == BLK)
        .unwrap_or(false)
}

fn read_sb_bytes(dev: u32, buf: &mut [u8; BLK]) -> bool {
    blk::read_bytes(dev, SB_OFF, buf)
        .map(|n| n == BLK)
        .unwrap_or(false)
}

fn write_sb_bytes(dev: u32, buf: &[u8; BLK]) -> bool {
    blk::write_bytes(dev, SB_OFF, buf)
        .map(|n| n == BLK)
        .unwrap_or(false)
}

fn bit_get(bm: &[u8], i: u32) -> bool {
    let byte = (i / 8) as usize;
    let bit = (i % 8) as u8;
    if byte >= bm.len() {
        return true;
    }
    bm[byte] & (1 << bit) != 0
}

fn bit_set(bm: &mut [u8], i: u32, used: bool) {
    let byte = (i / 8) as usize;
    let bit = (i % 8) as u8;
    if byte >= bm.len() {
        return;
    }
    if used {
        bm[byte] |= 1 << bit;
    } else {
        bm[byte] &= !(1 << bit);
    }
}

fn parse_super(buf: &[u8; BLK]) -> Option<Super> {
    if le_u16(buf, 56) != MAGIC {
        return None;
    }
    if le_u32(buf, 24) != 0 {
        return None; // 1024-byte blocks only
    }
    if le_u32(buf, 96) != 0 {
        return None; // no INCOMPAT features
    }
    let rev = le_u32(buf, 76);
    let inode_size = if rev >= 1 { le_u16(buf, 88) } else { 128 };
    if inode_size != 128 {
        return None;
    }
    let inodes_count = le_u32(buf, 0);
    let blocks_count = le_u32(buf, 4);
    if inodes_count < ROOT_INO || blocks_count < 22 {
        return None;
    }
    let first_ino = if rev >= 1 { le_u32(buf, 84) } else { 11 };
    Some(Super {
        inodes_count,
        blocks_count,
        first_ino: if first_ino == 0 { 11 } else { first_ino },
        inode_size,
    })
}

fn parse_group(buf: &[u8; BLK]) -> Option<Group> {
    let block_bitmap = le_u32(buf, 0);
    let inode_bitmap = le_u32(buf, 4);
    let inode_table = le_u32(buf, 8);
    if block_bitmap == 0 || inode_bitmap == 0 || inode_table == 0 {
        return None;
    }
    Some(Group {
        block_bitmap,
        inode_bitmap,
        inode_table,
    })
}

fn inode_loc(fs: &Ext2, ino: u32) -> Option<(u32, usize)> {
    if ino == 0 || ino > fs.sb.inodes_count {
        return None;
    }
    let idx = (ino - 1) as usize;
    let off = idx * fs.sb.inode_size as usize;
    let block = fs.gd.inode_table + (off / BLK) as u32;
    let into = off % BLK;
    Some((block, into))
}

fn read_inode(fs: &Ext2, ino: u32) -> Option<Inode> {
    let (block, into) = inode_loc(fs, ino)?;
    let mut buf = [0u8; BLK];
    if !read_block(fs.dev, block, &mut buf) {
        return None;
    }
    let s = &buf[into..into + INODE_SIZE];
    let mut direct = [0u32; NDIRECT];
    for i in 0..NDIRECT {
        direct[i] = le_u32(s, 40 + i * 4);
    }
    Some(Inode {
        mode: le_u16(s, 0),
        size: le_u32(s, 4),
        links: le_u16(s, 26),
        blocks: le_u32(s, 28),
        direct,
    })
}

fn write_inode(fs: &Ext2, ino: u32, node: &Inode) -> bool {
    let Some((block, into)) = inode_loc(fs, ino) else {
        return false;
    };
    let mut buf = [0u8; BLK];
    if !read_block(fs.dev, block, &mut buf) {
        return false;
    }
    let s = &mut buf[into..into + INODE_SIZE];
    put_u16(s, 0, node.mode);
    put_u32(s, 4, node.size);
    put_u16(s, 26, node.links);
    put_u32(s, 28, node.blocks);
    for i in 0..NDIRECT {
        put_u32(s, 40 + i * 4, node.direct[i]);
    }
    write_block(fs.dev, block, &buf)
}

fn is_dir(node: &Inode) -> bool {
    node.mode & S_IFMT == S_IFDIR
}

fn is_reg(node: &Inode) -> bool {
    node.mode & S_IFMT == S_IFREG
}

fn adj_free_blocks(fs: &Ext2, delta: i32) -> bool {
    let mut sb = [0u8; BLK];
    let mut gd = [0u8; BLK];
    if !read_sb_bytes(fs.dev, &mut sb) || !read_block(fs.dev, 2, &mut gd) {
        return false;
    }
    let mut free_b = le_u32(&sb, 12) as i32 + delta;
    if free_b < 0 {
        free_b = 0;
    }
    put_u32(&mut sb, 12, free_b as u32);
    let mut bg_free = le_u16(&gd, 12) as i32 + delta;
    if bg_free < 0 {
        bg_free = 0;
    }
    put_u16(&mut gd, 12, bg_free as u16);
    write_sb_bytes(fs.dev, &sb) && write_block(fs.dev, 2, &gd)
}

fn adj_free_inodes(fs: &Ext2, delta: i32, dirs: i32) -> bool {
    let mut sb = [0u8; BLK];
    let mut gd = [0u8; BLK];
    if !read_sb_bytes(fs.dev, &mut sb) || !read_block(fs.dev, 2, &mut gd) {
        return false;
    }
    let mut free_i = le_u32(&sb, 16) as i32 + delta;
    if free_i < 0 {
        free_i = 0;
    }
    put_u32(&mut sb, 16, free_i as u32);
    let mut bg_free = le_u16(&gd, 14) as i32 + delta;
    if bg_free < 0 {
        bg_free = 0;
    }
    put_u16(&mut gd, 14, bg_free as u16);
    if dirs != 0 {
        let mut used = le_u16(&gd, 16) as i32 + dirs;
        if used < 0 {
            used = 0;
        }
        put_u16(&mut gd, 16, used as u16);
    }
    write_sb_bytes(fs.dev, &sb) && write_block(fs.dev, 2, &gd)
}

fn alloc_block(fs: &Ext2) -> Option<u32> {
    let mut bm = [0u8; BLK];
    if !read_block(fs.dev, fs.gd.block_bitmap, &mut bm) {
        return None;
    }
    for i in 0..fs.sb.blocks_count {
        if !bit_get(&bm, i) {
            bit_set(&mut bm, i, true);
            if !write_block(fs.dev, fs.gd.block_bitmap, &bm) {
                return None;
            }
            if !adj_free_blocks(fs, -1) {
                return None;
            }
            let mut z = [0u8; BLK];
            if !write_block(fs.dev, i, &z) {
                return None;
            }
            return Some(i);
        }
    }
    None
}

fn free_block(fs: &Ext2, block: u32) -> bool {
    if block == 0 || block >= fs.sb.blocks_count {
        return false;
    }
    let mut bm = [0u8; BLK];
    if !read_block(fs.dev, fs.gd.block_bitmap, &mut bm) {
        return false;
    }
    if !bit_get(&bm, block) {
        return true;
    }
    bit_set(&mut bm, block, false);
    write_block(fs.dev, fs.gd.block_bitmap, &bm) && adj_free_blocks(fs, 1)
}

fn alloc_inode(fs: &Ext2) -> Option<u32> {
    let mut bm = [0u8; BLK];
    if !read_block(fs.dev, fs.gd.inode_bitmap, &mut bm) {
        return None;
    }
    let start = fs.sb.first_ino.max(1);
    for ino in start..=fs.sb.inodes_count {
        let bit = ino - 1;
        if !bit_get(&bm, bit) {
            bit_set(&mut bm, bit, true);
            if !write_block(fs.dev, fs.gd.inode_bitmap, &bm) {
                return None;
            }
            if !adj_free_inodes(fs, -1, 0) {
                return None;
            }
            return Some(ino);
        }
    }
    None
}

fn free_inode_bit(fs: &Ext2, ino: u32, was_dir: bool) -> bool {
    if ino < fs.sb.first_ino || ino > fs.sb.inodes_count {
        return false;
    }
    let mut bm = [0u8; BLK];
    if !read_block(fs.dev, fs.gd.inode_bitmap, &mut bm) {
        return false;
    }
    bit_set(&mut bm, ino - 1, false);
    let dirs = if was_dir { -1 } else { 0 };
    write_block(fs.dev, fs.gd.inode_bitmap, &bm) && adj_free_inodes(fs, 1, dirs)
}

fn free_inode_blocks(fs: &Ext2, node: &mut Inode) {
    for i in 0..NDIRECT {
        if node.direct[i] != 0 {
            let _ = free_block(fs, node.direct[i]);
            node.direct[i] = 0;
        }
    }
    node.blocks = 0;
    node.size = 0;
}

fn dirent_real_len(name_len: u8) -> usize {
    (8 + name_len as usize + 3) & !3
}

fn for_each_dirent(
    fs: &Ext2,
    node: &Inode,
    mut f: impl FnMut(u32, &[u8], u32, usize, usize) -> bool,
) -> bool {
    if !is_dir(node) {
        return false;
    }
    for bi in 0..NDIRECT {
        let bno = node.direct[bi];
        if bno == 0 {
            continue;
        }
        let mut blk = [0u8; BLK];
        if !read_block(fs.dev, bno, &mut blk) {
            return false;
        }
        let mut off = 0usize;
        while off + 8 <= BLK {
            let rec = le_u16(&blk, off + 4) as usize;
            if rec < 8 || off + rec > BLK {
                break;
            }
            let ino = le_u32(&blk, off);
            let name_len = le_u16(&blk, off + 6) as usize;
            if ino != 0 && name_len > 0 && 8 + name_len <= rec {
                let name = &blk[off + 8..off + 8 + name_len];
                if f(ino, name, bno, off, rec) {
                    return true;
                }
            }
            if rec == 0 {
                break;
            }
            off += rec;
        }
    }
    false
}

fn dir_lookup(fs: &Ext2, dir: &Inode, name: &[u8]) -> Option<u32> {
    let mut found = None;
    for_each_dirent(fs, dir, |ino, n, _, _, _| {
        if n == name {
            found = Some(ino);
            true
        } else {
            false
        }
    });
    found
}

fn dir_add(fs: &Ext2, dir_ino: u32, dir: &mut Inode, name: &[u8], ino: u32) -> bool {
    if name.is_empty() || name.len() > 255 {
        return false;
    }
    let need = dirent_real_len(name.len() as u8);
    for bi in 0..NDIRECT {
        let mut bno = dir.direct[bi];
        if bno == 0 {
            let Some(nb) = alloc_block(fs) else {
                return false;
            };
            dir.direct[bi] = nb;
            dir.blocks = dir.blocks.saturating_add(2);
            if dir.size < ((bi as u32) + 1) * BLK as u32 {
                dir.size = ((bi as u32) + 1) * BLK as u32;
            }
            bno = nb;
            let mut blk = [0u8; BLK];
            put_u32(&mut blk, 0, ino);
            put_u16(&mut blk, 4, BLK as u16);
            put_u16(&mut blk, 6, name.len() as u16);
            blk[8..8 + name.len()].copy_from_slice(name);
            return write_block(fs.dev, bno, &blk) && write_inode(fs, dir_ino, dir);
        }
        let mut blk = [0u8; BLK];
        if !read_block(fs.dev, bno, &mut blk) {
            return false;
        }
        let mut off = 0usize;
        while off + 8 <= BLK {
            let rec = le_u16(&blk, off + 4) as usize;
            if rec < 8 || off + rec > BLK {
                break;
            }
            let e_ino = le_u32(&blk, off);
            let e_nlen = le_u16(&blk, off + 6) as u8;
            let real = if e_ino == 0 {
                0
            } else {
                dirent_real_len(e_nlen)
            };
            if e_ino == 0 && rec >= need {
                put_u32(&mut blk, off, ino);
                put_u16(&mut blk, off + 6, name.len() as u16);
                blk[off + 8..off + 8 + name.len()].copy_from_slice(name);
                return write_block(fs.dev, bno, &blk);
            }
            if rec >= real + need && e_ino != 0 {
                put_u16(&mut blk, off + 4, real as u16);
                let noff = off + real;
                put_u32(&mut blk, noff, ino);
                put_u16(&mut blk, noff + 4, (rec - real) as u16);
                put_u16(&mut blk, noff + 6, name.len() as u16);
                blk[noff + 8..noff + 8 + name.len()].copy_from_slice(name);
                return write_block(fs.dev, bno, &blk);
            }
            if rec == 0 {
                break;
            }
            off += rec;
        }
    }
    false
}

fn dir_remove(fs: &Ext2, dir: &Inode, name: &[u8]) -> Option<u32> {
    let mut removed = None;
    for bi in 0..NDIRECT {
        let bno = dir.direct[bi];
        if bno == 0 {
            continue;
        }
        let mut blk = [0u8; BLK];
        if !read_block(fs.dev, bno, &mut blk) {
            return None;
        }
        let mut off = 0usize;
        while off + 8 <= BLK {
            let rec = le_u16(&blk, off + 4) as usize;
            if rec < 8 || off + rec > BLK {
                break;
            }
            let e_ino = le_u32(&blk, off);
            let e_nlen = le_u16(&blk, off + 6) as usize;
            if e_ino != 0 && e_nlen == name.len() && &blk[off + 8..off + 8 + e_nlen] == name {
                put_u32(&mut blk, off, 0);
                if write_block(fs.dev, bno, &blk) {
                    removed = Some(e_ino);
                }
                return removed;
            }
            if rec == 0 {
                break;
            }
            off += rec;
        }
    }
    None
}

fn parent_name(path: &str) -> Option<(&str, &str)> {
    if path.is_empty() || path == "." || path == ".." {
        return None;
    }
    if path.starts_with('/') || path.ends_with('/') || path.contains("//") {
        return None;
    }
    match path.rfind('/') {
        Some(i) => {
            let parent = &path[..i];
            let name = &path[i + 1..];
            if name.is_empty() || name == "." || name == ".." {
                None
            } else {
                Some((parent, name))
            }
        }
        None => Some(("", path)),
    }
}

fn resolve(fs: &Ext2, path: &str) -> Option<u32> {
    if path.is_empty() || path == "." {
        return Some(ROOT_INO);
    }
    if path.starts_with('/') || path.contains("//") {
        return None;
    }
    let mut ino = ROOT_INO;
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        let node = read_inode(fs, ino)?;
        if !is_dir(&node) {
            return None;
        }
        ino = dir_lookup(fs, &node, comp.as_bytes())?;
    }
    Some(ino)
}

fn ensure_block(fs: &Ext2, node: &mut Inode, idx: usize) -> Option<u32> {
    if idx >= NDIRECT {
        return None;
    }
    if node.direct[idx] == 0 {
        let b = alloc_block(fs)?;
        node.direct[idx] = b;
        node.blocks = node.blocks.saturating_add(2);
    }
    Some(node.direct[idx])
}

fn read_data(fs: &Ext2, node: &Inode, pos: usize, out: &mut [u8]) -> usize {
    if pos >= node.size as usize || out.is_empty() {
        return 0;
    }
    let want = out.len().min(node.size as usize - pos);
    let mut done = 0usize;
    while done < want {
        let abs = pos + done;
        let idx = abs / BLK;
        let into = abs % BLK;
        if idx >= NDIRECT {
            break;
        }
        let bno = node.direct[idx];
        if bno == 0 {
            break;
        }
        let mut blk = [0u8; BLK];
        if !read_block(fs.dev, bno, &mut blk) {
            break;
        }
        let take = (BLK - into).min(want - done);
        out[done..done + take].copy_from_slice(&blk[into..into + take]);
        done += take;
    }
    done
}

fn write_data(fs: &Ext2, ino: u32, node: &mut Inode, pos: usize, buf: &[u8]) -> Option<usize> {
    if buf.is_empty() {
        return Some(0);
    }
    if pos > node.size as usize {
        return None;
    }
    let mut done = 0usize;
    while done < buf.len() {
        let abs = pos + done;
        let idx = abs / BLK;
        let into = abs % BLK;
        let bno = ensure_block(fs, node, idx)?;
        let mut blk = [0u8; BLK];
        if into != 0 || (buf.len() - done) < BLK {
            if !read_block(fs.dev, bno, &mut blk) {
                break;
            }
        }
        let take = (BLK - into).min(buf.len() - done);
        blk[into..into + take].copy_from_slice(&buf[done..done + take]);
        if !write_block(fs.dev, bno, &blk) {
            break;
        }
        done += take;
    }
    let end = pos + done;
    if end as u32 > node.size {
        node.size = end as u32;
    }
    if !write_inode(fs, ino, node) {
        return None;
    }
    if done == 0 { None } else { Some(done) }
}

fn locked<T>(f: impl FnOnce(&Ext2) -> T) -> Option<T> {
    let g = FS.lock();
    g.as_ref().map(f)
}

/// Parse the on-disk superblock and bind this device as the active ext2 mount.
pub fn bind(dev: u32) -> Option<MountOps> {
    let mut sb = [0u8; BLK];
    if !read_sb_bytes(dev, &mut sb) {
        return None;
    }
    let super_ = parse_super(&sb)?;
    let mut gd = [0u8; BLK];
    if !read_block(dev, 2, &mut gd) {
        return None;
    }
    let group = parse_group(&gd)?;
    *FS.lock() = Some(Ext2 {
        dev,
        sb: super_,
        gd: group,
    });
    Some(MountOps {
        lookup,
        stat,
        listdir,
        register,
        create,
        truncate,
        read,
        write,
        mkdir,
        rmdir,
        unlink,
        rename,
        symlink,
        readlink,
        writable: true,
    })
}

pub fn lookup(_path: &str) -> Option<&'static [u8]> {
    None
}

pub fn register(_name: &str, _bytes: &'static [u8]) -> bool {
    false
}

pub fn rmdir(_path: &str) -> bool {
    false
}

pub fn rename(_old: &str, _new: &str) -> bool {
    false
}

pub fn symlink(_target: &str, _linkpath: &str) -> bool {
    false
}

pub fn readlink(_path: &str, _buf: &mut [u8]) -> Option<usize> {
    None
}

pub fn stat(path: &str) -> Option<StatInfo> {
    locked(|fs| {
        let ino = resolve(fs, path)?;
        let node = read_inode(fs, ino)?;
        let mode = if is_dir(&node) {
            (S_IFDIR as u32) | (node.mode as u32 & 0o777)
        } else {
            (S_IFREG as u32) | (node.mode as u32 & 0o777)
        };
        Some(StatInfo {
            mode,
            size: node.size,
            ino,
            nlink: node.links as u32,
        })
    })?
}

pub fn listdir(path: &str, buf: &mut [u8]) -> usize {
    locked(|fs| {
        let Some(ino) = resolve(fs, path) else {
            return 0;
        };
        let Some(node) = read_inode(fs, ino) else {
            return 0;
        };
        if !is_dir(&node) {
            return 0;
        }
        let mut n = 0usize;
        for_each_dirent(fs, &node, |_ino, name, _, _, _| {
            if name == b"." || name == b".." {
                return false;
            }
            let need = name.len() + 1;
            if n + need > buf.len() {
                return true;
            }
            buf[n..n + name.len()].copy_from_slice(name);
            n += name.len();
            buf[n] = b'\n';
            n += 1;
            false
        });
        n
    })
    .unwrap_or(0)
}

pub fn create(path: &str) -> bool {
    locked(|fs| {
        let Some((parent, name)) = parent_name(path) else {
            return false;
        };
        if let Some(ino) = resolve(fs, path) {
            return read_inode(fs, ino).map(|n| is_reg(&n)).unwrap_or(false);
        }
        let Some(pino) = resolve(fs, parent) else {
            return false;
        };
        let Some(mut pnode) = read_inode(fs, pino) else {
            return false;
        };
        if !is_dir(&pnode) {
            return false;
        }
        let Some(ino) = alloc_inode(fs) else {
            return false;
        };
        let node = Inode {
            mode: S_IFREG_MODE,
            size: 0,
            links: 1,
            blocks: 0,
            direct: [0; NDIRECT],
        };
        if !write_inode(fs, ino, &node) {
            return false;
        }
        dir_add(fs, pino, &mut pnode, name.as_bytes(), ino)
    })
    .unwrap_or(false)
}

pub fn mkdir(path: &str) -> bool {
    locked(|fs| {
        let Some((parent, name)) = parent_name(path) else {
            return false;
        };
        if resolve(fs, path).is_some() {
            return false;
        }
        let Some(pino) = resolve(fs, parent) else {
            return false;
        };
        let Some(mut pnode) = read_inode(fs, pino) else {
            return false;
        };
        if !is_dir(&pnode) {
            return false;
        }
        let Some(ino) = alloc_inode(fs) else {
            return false;
        };
        let Some(db) = alloc_block(fs) else {
            return false;
        };
        let mut dir = [0u8; BLK];
        put_u32(&mut dir, 0, ino);
        put_u16(&mut dir, 4, 12);
        put_u16(&mut dir, 6, 1);
        dir[8] = b'.';
        put_u32(&mut dir, 12, pino);
        put_u16(&mut dir, 16, (BLK - 12) as u16);
        put_u16(&mut dir, 18, 2);
        dir[20] = b'.';
        dir[21] = b'.';
        if !write_block(fs.dev, db, &dir) {
            return false;
        }
        let node = Inode {
            mode: S_IFDIR_MODE,
            size: BLK as u32,
            links: 2,
            blocks: 2,
            direct: {
                let mut d = [0u32; NDIRECT];
                d[0] = db;
                d
            },
        };
        if !write_inode(fs, ino, &node) {
            return false;
        }
        pnode.links = pnode.links.saturating_add(1);
        if !dir_add(fs, pino, &mut pnode, name.as_bytes(), ino) {
            return false;
        }
        if !write_inode(fs, pino, &pnode) {
            return false;
        }
        adj_free_inodes(fs, 0, 1)
    })
    .unwrap_or(false)
}

pub fn truncate(path: &str) -> bool {
    locked(|fs| {
        let Some(ino) = resolve(fs, path) else {
            return false;
        };
        let Some(mut node) = read_inode(fs, ino) else {
            return false;
        };
        if !is_reg(&node) {
            return false;
        }
        free_inode_blocks(fs, &mut node);
        write_inode(fs, ino, &node)
    })
    .unwrap_or(false)
}

pub fn unlink(path: &str) -> bool {
    locked(|fs| {
        let Some((parent, name)) = parent_name(path) else {
            return false;
        };
        if name == "." || name == ".." {
            return false;
        }
        let Some(pino) = resolve(fs, parent) else {
            return false;
        };
        let Some(pnode) = read_inode(fs, pino) else {
            return false;
        };
        if !is_dir(&pnode) {
            return false;
        }
        let Some(ino) = dir_remove(fs, &pnode, name.as_bytes()) else {
            return false;
        };
        let Some(mut node) = read_inode(fs, ino) else {
            return false;
        };
        if is_dir(&node) {
            return false;
        }
        free_inode_blocks(fs, &mut node);
        let _ = write_inode(
            fs,
            ino,
            &Inode {
                mode: 0,
                size: 0,
                links: 0,
                blocks: 0,
                direct: [0; NDIRECT],
            },
        );
        free_inode_bit(fs, ino, false)
    })
    .unwrap_or(false)
}

pub fn read(path: &str, pos: usize, out: &mut [u8]) -> usize {
    locked(|fs| {
        let Some(ino) = resolve(fs, path) else {
            return 0;
        };
        let Some(node) = read_inode(fs, ino) else {
            return 0;
        };
        if !is_reg(&node) {
            return 0;
        }
        read_data(fs, &node, pos, out)
    })
    .unwrap_or(0)
}

pub fn write(path: &str, pos: usize, buf: &[u8]) -> Option<usize> {
    locked(|fs| {
        let ino = resolve(fs, path)?;
        let mut node = read_inode(fs, ino)?;
        if !is_reg(&node) {
            return None;
        }
        write_data(fs, ino, &mut node, pos, buf)
    })?
}
