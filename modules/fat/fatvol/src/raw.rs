//! Directory entries edited on the disk itself, for what fstool has no call
//! for: moving an entry (`rename`) and setting its times. The volume is
//! unmounted meanwhile (`Fat::edit_entries`), so this reads and writes the
//! device directly with the volume's geometry.
//!
//! An entry is found the way FAT names it: by its long name when a valid
//! long-name run (VFAT LFN, checksum matching the short name) precedes it,
//! else by its 8.3 name; either ignoring ASCII case.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use fstool::device::SectorDriver;
use fstool::fs::fat::{FatKind, Geometry, Timestamp};

use crate::{Error, Result};

const ENTRY: usize = 32;
const ATTR_LFN: u8 = 0x0F;
const ATTR_VOLUME_ID: u8 = 0x08;
const ATTR_DIR: u8 = 0x10;
const ATTR_ARCHIVE: u8 = 0x20;
const DELETED: u8 = 0xE5;

/// Where an entry is: a volume-relative sector and the byte offset in it.
#[derive(Clone, Copy)]
struct Loc {
    sector: u64,
    offset: usize,
}

/// Move what `old` names to the entry `new` (just created, empty): the new
/// entry takes the old one's attributes, times, first cluster and size,
/// the old one is left an empty file for fstool to remove, and a directory
/// moved to another parent gets its `..` pointed there.
pub(crate) fn move_entry<D: SectorDriver>(dev: &mut D, geo: &Geometry, old: &str, new: &str, is_dir: bool) -> Result<()> {
    let (old_parent, old_loc, o) = resolve(dev, geo, old)?;
    let (new_parent, new_loc, _) = resolve(dev, geo, new)?;
    // Byte 12 holds the case flags of the new 8.3 name: those stay.
    patch(dev, geo, new_loc, |e| {
        e[11] = o[11];
        e[13..ENTRY].copy_from_slice(&o[13..ENTRY]);
    })?;
    patch(dev, geo, old_loc, |e| {
        e[11] = ATTR_ARCHIVE;
        e[20..22].fill(0);
        e[26..ENTRY].fill(0);
    })?;
    if is_dir && old_parent != new_parent {
        // `..` is the second entry of the directory's first cluster; it
        // names the root as cluster 0, on FAT32 too.
        let first = cluster(&o);
        let dotdot = Loc { sector: cluster_sector(geo, first), offset: ENTRY };
        let parent = if new_parent == root(geo) { 0 } else { new_parent };
        let mut ok = false;
        patch(dev, geo, dotdot, |e| {
            ok = &e[..11] == b"..         ";
            if ok {
                e[20..22].copy_from_slice(&((parent >> 16) as u16).to_le_bytes());
                e[26..28].copy_from_slice(&(parent as u16).to_le_bytes());
            }
        })?;
        if !ok {
            return Err(Error::Io);
        }
    }
    Ok(())
}

/// Set the modification date and time, and the access date, of `path`'s entry.
pub(crate) fn set_times<D: SectorDriver>(
    dev: &mut D,
    geo: &Geometry,
    path: &str,
    atime: Option<Timestamp>,
    mtime: Option<Timestamp>,
) -> Result<()> {
    let (_, loc, _) = resolve(dev, geo, path)?;
    patch(dev, geo, loc, |e| {
        if let Some(t) = atime {
            e[18..20].copy_from_slice(&t.date.to_le_bytes());
        }
        if let Some(t) = mtime {
            e[22..24].copy_from_slice(&t.time.to_le_bytes());
            e[24..26].copy_from_slice(&t.date.to_le_bytes());
        }
    })
}

/// Whether `name` could be an 8.3 name alone, in upper case (`MSG`,
/// `BOOTX64.EFI`): one the entry records without a long name.
pub(crate) fn is_short_upper(name: &str) -> bool {
    let (base, ext) = match name.rsplit_once('.') {
        Some((b, e)) => (b, e),
        None => (name, ""),
    };
    let ok = |s: &str| s.bytes().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || b"!#$%&'()-@^_`{}~".contains(&c));
    (1..=8).contains(&base.len()) && ext.len() <= 3 && ok(base) && ok(ext) && !(name.ends_with('.'))
}

/// The parent directory's first cluster, and the location and bytes of the
/// entry `path` (absolute, not the root) names.
fn resolve<D: SectorDriver>(dev: &mut D, geo: &Geometry, path: &str) -> Result<(u32, Loc, [u8; ENTRY])> {
    let mut dir = root(geo);
    let mut parts = path.trim_matches('/').split('/').peekable();
    while let Some(name) = parts.next() {
        let (loc, e) = find(dev, geo, dir, name)?.ok_or(Error::NotFound)?;
        if parts.peek().is_none() {
            return Ok((dir, loc, e));
        }
        if e[11] & ATTR_DIR == 0 {
            return Err(Error::NotDir);
        }
        dir = cluster(&e);
    }
    Err(Error::Invalid)
}

/// The entry named `name` in the directory starting at `dir` (0: the
/// FAT12/16 fixed root).
fn find<D: SectorDriver>(dev: &mut D, geo: &Geometry, dir: u32, name: &str) -> Result<Option<(Loc, [u8; ENTRY])>> {
    let mut sec = vec![0u8; geo.bytes_per_sector as usize];
    // The long-name run being read: its parts by order, the checksum it
    // gives the short name, and the order the next part must have.
    let mut lfn: Vec<[u16; 13]> = Vec::new();
    let mut sum = 0u8;
    let mut next = 0u8;
    for sector in dir_sectors(dev, geo, dir)? {
        read(dev, geo, sector, &mut sec)?;
        for offset in (0..sec.len()).step_by(ENTRY) {
            let e: [u8; ENTRY] = sec[offset..offset + ENTRY].try_into().unwrap();
            if e[0] == 0 {
                return Ok(None);
            }
            if e[0] == DELETED {
                lfn.clear();
                continue;
            }
            if e[11] & 0x3F == ATTR_LFN {
                let ord = e[0] & 0x1F;
                if e[0] & 0x40 != 0 {
                    lfn = vec![[0xFFFF; 13]; usize::from(ord)];
                    sum = e[13];
                } else if ord != next || e[13] != sum {
                    lfn.clear();
                }
                if ord == 0 || usize::from(ord) > lfn.len() {
                    lfn.clear();
                    continue;
                }
                let part = &mut lfn[usize::from(ord) - 1];
                for (i, at) in [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30].into_iter().enumerate() {
                    part[i] = u16::from_le_bytes([e[at], e[at + 1]]);
                }
                next = ord - 1;
                continue;
            }
            let long = (!lfn.is_empty() && next == 0 && checksum(&e) == sum).then(|| long_name(&lfn));
            lfn.clear();
            if e[11] & ATTR_VOLUME_ID != 0 {
                continue;
            }
            let matches = match &long {
                Some(l) => l.eq_ignore_ascii_case(name),
                None => short_name(&e).eq_ignore_ascii_case(name),
            };
            if matches {
                return Ok(Some((Loc { sector, offset }, e)));
            }
        }
    }
    Ok(None)
}

fn long_name(parts: &[[u16; 13]]) -> String {
    let units: Vec<u16> = parts.iter().flatten().copied().take_while(|&u| u != 0 && u != 0xFFFF).collect();
    String::from_utf16_lossy(&units)
}

/// The 8.3 name as `NAME.EXT`, with the lower-case flags of byte 12.
fn short_name(e: &[u8; ENTRY]) -> String {
    let mut base: Vec<u8> = e[..8].iter().copied().take_while(|&c| c != b' ').collect();
    let mut ext: Vec<u8> = e[8..11].iter().copied().take_while(|&c| c != b' ').collect();
    if base.first() == Some(&0x05) {
        base[0] = DELETED;
    }
    if e[12] & 0x08 != 0 {
        base.make_ascii_lowercase();
    }
    if e[12] & 0x10 != 0 {
        ext.make_ascii_lowercase();
    }
    let mut s: String = base.iter().map(|&c| char::from(c)).collect();
    if !ext.is_empty() {
        s.push('.');
        s.extend(ext.iter().map(|&c| char::from(c)));
    }
    s
}

/// The checksum of an 8.3 name that its long-name parts carry.
fn checksum(e: &[u8; ENTRY]) -> u8 {
    e[..11].iter().fold(0u8, |s, &c| s.rotate_right(1).wrapping_add(c))
}

fn cluster(e: &[u8; ENTRY]) -> u32 {
    u32::from(u16::from_le_bytes([e[20], e[21]])) << 16 | u32::from(u16::from_le_bytes([e[26], e[27]]))
}

/// The root directory: its first cluster on FAT32, 0 (the fixed region) otherwise.
fn root(geo: &Geometry) -> u32 {
    if geo.kind == FatKind::Fat32 { geo.root_cluster } else { 0 }
}

fn cluster_sector(geo: &Geometry, c: u32) -> u64 {
    u64::from(geo.first_data_sector) + u64::from(c - 2) * u64::from(geo.sectors_per_cluster)
}

/// The sectors of the directory starting at `dir`, in order.
fn dir_sectors<D: SectorDriver>(dev: &mut D, geo: &Geometry, dir: u32) -> Result<Vec<u64>> {
    if dir == 0 {
        let start = u64::from(geo.reserved_sectors + geo.num_fats * geo.fat_sectors);
        return Ok((start..start + u64::from(geo.root_dir_sectors)).collect());
    }
    let mut out = Vec::new();
    let mut c = dir;
    // A chain longer than the volume has clusters loops.
    for _ in 0..=geo.cluster_count {
        if c < 2 || c > geo.cluster_count + 1 {
            return Err(Error::Io);
        }
        let first = cluster_sector(geo, c);
        out.extend(first..first + u64::from(geo.sectors_per_cluster));
        c = match next_cluster(dev, geo, c)? {
            Some(n) => n,
            None => return Ok(out),
        };
    }
    Err(Error::Io)
}

/// The cluster after `c` in its chain (`None` at the end).
fn next_cluster<D: SectorDriver>(dev: &mut D, geo: &Geometry, c: u32) -> Result<Option<u32>> {
    let (width, end) = match geo.kind {
        FatKind::Fat16 => (2u64, 0xFFF8),
        FatKind::Fat32 => (4, 0x0FFF_FFF8),
        _ => return Err(Error::Unsupported),
    };
    let bps = u64::from(geo.bytes_per_sector);
    let fat = u64::from(geo.reserved_sectors + geo.active_fat * geo.fat_sectors);
    let at = u64::from(c) * width;
    let mut sec = vec![0u8; bps as usize];
    read(dev, geo, fat + at / bps, &mut sec)?;
    let o = (at % bps) as usize;
    let v = if width == 2 {
        u32::from(u16::from_le_bytes([sec[o], sec[o + 1]]))
    } else {
        u32::from_le_bytes([sec[o], sec[o + 1], sec[o + 2], sec[o + 3]]) & 0x0FFF_FFFF
    };
    Ok((v < end).then_some(v))
}

fn read<D: SectorDriver>(dev: &mut D, geo: &Geometry, sector: u64, buf: &mut [u8]) -> Result<()> {
    dev.read_sectors(geo.part_start + sector, buf).map_err(|_| Error::Io)
}

/// Read the sector holding `loc`, change its entry with `f`, write it back.
fn patch<D: SectorDriver>(dev: &mut D, geo: &Geometry, loc: Loc, f: impl FnOnce(&mut [u8])) -> Result<()> {
    let mut sec = vec![0u8; geo.bytes_per_sector as usize];
    read(dev, geo, loc.sector, &mut sec)?;
    f(&mut sec[loc.offset..loc.offset + ENTRY]);
    dev.write_sectors(geo.part_start + loc.sector, &sec).map_err(|_| Error::Io)
}
