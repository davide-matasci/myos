//! A FAT16 or FAT32 filesystem, by path, over a sector device: what the
//! `fat` kernel module serves the VFS with (`modules/fat/src/main.rs`).
//!
//! The on-disk work is fstool's FAT driver (crates.io, MIT; pinned in
//! `Cargo.toml`): directories with long names, files, the allocation table
//! in both copies, the FAT32 FSInfo sector. This crate adds what the VFS
//! needs and fstool lacks or gets wrong:
//!
//! - growing a file writes zeros itself: fstool's `set_len`, and the gap of
//!   a write past the end, leave the old contents of the clusters they
//!   allocate in the file;
//! - writes go a cluster at a time, so that one that fills the volume
//!   records the length it reached (fstool's leaves the entry shorter than
//!   its chain), and the file last read or written stays open between
//!   calls (a fresh handle walks the cluster chain from the start);
//! - `rename` and `set_times` edit directory entries on the disk directly
//!   (`raw.rs`), fstool having no call for either;
//! - names a short (8.3) entry alone records are listed in lower case, as
//!   Linux's `shortname=lower` does; lookups ignore case either way.
//!
//! Paths are relative to the volume's root (`""` is the root), as the VFS
//! hands them over. Host tests against dosfstools and mtools: `tests.rs`.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod format;
mod raw;
#[cfg(test)]
mod tests;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use fstool::fs::fat::{self, Timestamp, Volume};

pub use format::format;
pub use fstool::device::SectorDriver;
pub use fstool::fs::fat::FatKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    NotFound,
    Exists,
    NotDir,
    IsDir,
    NotEmpty,
    NoSpace,
    /// Past FAT's 4 GiB - 1 file size.
    TooBig,
    Invalid,
    /// Not a FAT volume, or one this code does not change this way (a
    /// FAT12 rename).
    Unsupported,
    Io,
}

pub type Result<T> = core::result::Result<T, Error>;

fn err<E>(e: fat::Error<E>) -> Error {
    match e {
        fat::Error::NotFound => Error::NotFound,
        fat::Error::AlreadyExists => Error::Exists,
        fat::Error::NotADirectory => Error::NotDir,
        fat::Error::IsADirectory => Error::IsDir,
        fat::Error::DirectoryNotEmpty => Error::NotEmpty,
        fat::Error::NoSpace | fat::Error::DirectoryFull => Error::NoSpace,
        fat::Error::FileTooLarge | fat::Error::InvalidOffset => Error::TooBig,
        fat::Error::InvalidName | fat::Error::InvalidPath => Error::Invalid,
        fat::Error::NotFat | fat::Error::Unsupported(_) => Error::Unsupported,
        _ => Error::Io,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    pub kind: Kind,
    pub size: u32,
    /// FAT keeps no permissions, only a read-only bit.
    pub read_only: bool,
    /// Seconds since the epoch; FAT has a 2-second resolution and no zone
    /// (its times are taken as UTC).
    pub mtime: u32,
    /// The modification time again: FAT records only the day of the last
    /// access.
    pub atime: u32,
    /// Stands in for an inode number: FAT has none. Stable while the file
    /// keeps its path.
    pub id: u32,
}

/// What [`Fat::write`] and [`Fat::set_size`] write zeros with.
const ZEROS: [u8; 4096] = [0; 4096];

pub struct Fat<D: SectorDriver> {
    /// `None` only while a rename or a time change has the volume
    /// unmounted to edit entries under it (`raw.rs`), or after the device
    /// failed it on the way: the calls then fail with `Io`.
    vol: Option<Volume<D>>,
    /// The file the last read or write was on, kept open: a handle walks
    /// the cluster chain from where it last was, a fresh one from the
    /// start, which made reading a file through the VFS (a call per
    /// buffer) quadratic in its length. Dropped by whatever changes names.
    open: Option<(String, fat::File)>,
    now: u32,
}

impl<D: SectorDriver> Fat<D> {
    /// Mount the FAT volume that starts at sector 0 of `dev`.
    pub fn mount(dev: D) -> Result<Self> {
        let vol = Volume::mount(dev).map_err(err)?;
        Ok(Self { vol: Some(vol), open: None, now: 0 })
    }

    fn vol(&mut self) -> Result<&mut Volume<D>> {
        self.vol.as_mut().ok_or(Error::Io)
    }

    /// The volume and the open file `path` (absolute).
    fn file(&mut self, path: &str) -> Result<(&mut Volume<D>, &mut fat::File)> {
        let vol = self.vol.as_mut().ok_or(Error::Io)?;
        if self.open.as_ref().is_none_or(|(p, _)| p != path) {
            self.open = None;
            let f = vol.open_file(path).map_err(err)?;
            self.open = Some((String::from(path), f));
        }
        Ok((vol, &mut self.open.as_mut().unwrap().1))
    }

    pub fn kind(&self) -> Option<FatKind> {
        self.vol.as_ref().map(|v| v.kind())
    }

    pub fn device(&self) -> Option<&D> {
        self.vol.as_ref().map(|v| v.driver())
    }

    /// The time (seconds since the epoch) what changes from now on is
    /// stamped with: fstool has no clock.
    pub fn set_now(&mut self, now: u32) {
        self.now = now;
        if let Some(v) = self.vol.as_mut() {
            v.set_time(timestamp(now));
        }
    }

    /// Write back everything cached: the allocation table, FSInfo.
    pub fn sync(&mut self) -> Result<()> {
        self.vol()?.flush().map_err(err)
    }

    pub fn unmount(mut self) -> Result<D> {
        self.vol.take().ok_or(Error::Io)?.unmount().map_err(err)
    }

    pub fn stat(&mut self, path: &str) -> Result<Stat> {
        let path = abs(path)?;
        if path == "/" {
            return Ok(Stat { kind: Kind::Dir, size: 0, read_only: false, mtime: 0, atime: 0, id: 1 });
        }
        let m = self.vol()?.metadata(&path).map_err(err)?;
        let mtime = unix(m.modified);
        Ok(Stat {
            kind: if m.is_dir() { Kind::Dir } else { Kind::File },
            size: m.len(),
            read_only: m.attrs.is_read_only(),
            mtime,
            atime: mtime,
            id: path_id(&path),
        })
    }

    /// Call `f` with each name in the directory `path` (not `.` and `..`)
    /// until it returns false.
    pub fn list(&mut self, path: &str, mut f: impl FnMut(&[u8]) -> bool) -> Result<()> {
        let path = abs(path)?;
        let vol = self.vol()?;
        let dir = vol.open_dir(&path).map_err(err)?;
        let mut names = Vec::new();
        let mut it = vol.iter_dir(dir);
        while let Some(e) = it.next().map_err(err)? {
            if !e.is_dot() {
                names.push(shown_name(e.name()));
            }
        }
        for name in names {
            if !f(name.as_bytes()) {
                break;
            }
        }
        Ok(())
    }

    /// Read the file `path` at `pos` into `out`: the bytes read (0 at the end).
    pub fn read(&mut self, path: &str, pos: u64, out: &mut [u8]) -> Result<usize> {
        let path = abs(path)?;
        let (vol, f) = self.file(&path)?;
        if pos >= u64::from(f.len()) {
            return Ok(0);
        }
        f.seek(vol, pos as u32).map_err(err)?;
        f.read(vol, out).map_err(err)
    }

    /// Write `buf` at `pos` of the file `path` (a gap before `pos` reads as
    /// zeros): the bytes written, fewer than `buf` when the volume filled
    /// up on the way.
    pub fn write(&mut self, path: &str, pos: u64, buf: &[u8]) -> Result<usize> {
        let path = abs(path)?;
        if pos + buf.len() as u64 > u64::from(u32::MAX) {
            return Err(Error::TooBig);
        }
        let (vol, f) = self.file(&path)?;
        let r = grow_to(vol, f, pos as u32).and_then(|()| put(vol, f, pos as u32, buf));
        // The size, and the clusters written so far, go to the entry even
        // when the volume filled up on the way.
        f.flush(vol).map_err(err)?;
        // Only growing allocates, so what failed is past what was written.
        let written = (u64::from(f.len()).saturating_sub(pos) as usize).min(buf.len());
        match r {
            Ok(()) => Ok(buf.len()),
            Err(_) if written > 0 => Ok(written),
            Err(e) => Err(e),
        }
    }

    /// Create the empty regular file `path` (fine if one is there already).
    pub fn create(&mut self, path: &str) -> Result<()> {
        let path = abs(path)?;
        let vol = self.vol()?;
        match vol.metadata(&path) {
            Ok(m) if m.is_dir() => Err(Error::IsDir),
            Ok(_) => Ok(()),
            Err(fat::Error::NotFound) => vol.create_file(&path).map(|_| ()).map_err(err),
            Err(e) => Err(err(e)),
        }
    }

    /// Cut the file `path` to length 0.
    pub fn truncate(&mut self, path: &str) -> Result<()> {
        self.set_size(path, 0)
    }

    /// Make the file `path` `size` bytes long: cut, or grown with zeros.
    pub fn set_size(&mut self, path: &str, size: u64) -> Result<()> {
        let path = abs(path)?;
        let size = u32::try_from(size).map_err(|_| Error::TooBig)?;
        let (vol, f) = self.file(&path)?;
        let r = if size > f.len() { grow_to(vol, f, size) } else { f.set_len(vol, size).map_err(err) };
        f.flush(vol).map_err(err)?;
        r
    }

    pub fn mkdir(&mut self, path: &str) -> Result<()> {
        let path = abs(path)?;
        self.vol()?.create_dir(&path).map(|_| ()).map_err(err)
    }

    pub fn rmdir(&mut self, path: &str) -> Result<()> {
        let path = abs(path)?;
        if path == "/" {
            return Err(Error::Invalid);
        }
        self.open = None;
        let vol = self.vol()?;
        if !vol.metadata(&path).map_err(err)?.is_dir() {
            return Err(Error::NotDir);
        }
        vol.remove_dir(&path).map_err(err)
    }

    pub fn unlink(&mut self, path: &str) -> Result<()> {
        let path = abs(path)?;
        self.open = None;
        self.vol()?.remove_file(&path).map_err(err)
    }

    /// Move `old` to `new`, replacing what `new` names (an empty directory
    /// by a directory, a file by a file). The data stays where it is: the
    /// new entry takes over the old one's clusters (`raw.rs`).
    pub fn rename(&mut self, old: &str, new: &str) -> Result<()> {
        let (old, new) = (abs(old)?, abs(new)?);
        if old == "/" || new == "/" {
            return Err(Error::Invalid);
        }
        self.open = None;
        let is_dir = self.vol()?.metadata(&old).map_err(err)?.is_dir();
        if old.eq_ignore_ascii_case(&new) {
            if old == new {
                return Ok(());
            }
            // The same entry under another case: FAT would find `new`
            // taken by `old` itself, so go through a name of its own.
            let tmp = format!("{}.rename-{}", old, self.now);
            self.rename(&old, &tmp)?;
            return self.rename(&tmp, &new);
        }
        if is_dir && within(&new, &old) {
            return Err(Error::Invalid);
        }
        if self.kind() == Some(FatKind::Fat12) {
            return Err(Error::Unsupported);
        }
        let vol = self.vol()?;
        match vol.metadata(&new) {
            Ok(t) => match (is_dir, t.is_dir()) {
                (false, true) => return Err(Error::IsDir),
                (true, false) => return Err(Error::NotDir),
                // A file replaced keeps its entry, emptied, for `old`'s:
                // there is no moment without a `new`, and no entry to find
                // room for. An empty directory makes room for one.
                (false, false) => {
                    let mut f = vol.open_file(&new).map_err(err)?;
                    f.set_len(vol, 0).map_err(err)?;
                    f.flush(vol).map_err(err)?;
                }
                (true, true) => {
                    vol.remove_dir(&new).map_err(err)?;
                    vol.create_file(&new).map_err(err)?;
                }
            },
            Err(fat::Error::NotFound) => {
                vol.create_file(&new).map_err(err)?;
            }
            Err(e) => return Err(err(e)),
        }
        self.edit_entries(|dev, geo| raw::move_entry(dev, geo, &old, &new, is_dir))?;
        // What `old` names is an empty file now: removing it frees nothing.
        self.vol()?.remove_file(&old).map_err(err)
    }

    /// Set the access and modification times of `path` (seconds since the
    /// epoch; `None` keeps one). The root has no entry to keep them in.
    pub fn set_times(&mut self, path: &str, atime: Option<u32>, mtime: Option<u32>) -> Result<()> {
        let path = abs(path)?;
        if path == "/" {
            return Ok(());
        }
        self.vol()?.metadata(&path).map_err(err)?;
        let (a, m) = (atime.map(timestamp), mtime.map(timestamp));
        self.edit_entries(|dev, geo| raw::set_times(dev, geo, &path, a, m))
    }

    /// Run `f` on the device with the volume unmounted, so that nothing
    /// fstool caches (a directory sector, the allocation table, an open
    /// file's entry) is stale after `f` writes entries under it, then mount
    /// it again. The volume is flushed first, so that an error writing
    /// back leaves it mounted.
    fn edit_entries(&mut self, f: impl FnOnce(&mut D, &fat::Geometry) -> Result<()>) -> Result<()> {
        self.open = None;
        self.vol()?.flush().map_err(err)?;
        let vol = self.vol.take().ok_or(Error::Io)?;
        let geo = *vol.geometry();
        let mut dev = vol.unmount().map_err(err)?;
        let r = f(&mut dev, &geo);
        let mut vol = Volume::mount(dev).map_err(err)?;
        vol.set_time(timestamp(self.now));
        self.vol = Some(vol);
        r
    }
}

/// Grow `f` to `size` with zeros, if it is shorter.
fn grow_to<D: SectorDriver>(vol: &mut Volume<D>, f: &mut fat::File, size: u32) -> Result<()> {
    while f.len() < size {
        let n = ((size - f.len()) as usize).min(ZEROS.len());
        put(vol, f, f.len(), &ZEROS[..n])?;
    }
    Ok(())
}

/// Write `buf` at `pos` of `f`, a cluster at most at a time: fstool's
/// `write` returns at a cluster it cannot allocate without recording the
/// length the clusters before it reached, so a write that fills the volume
/// would leave the entry shorter than its chain. In pieces, a piece that
/// fails allocates nothing, and the ones before it are recorded.
fn put<D: SectorDriver>(vol: &mut Volume<D>, f: &mut fat::File, pos: u32, buf: &[u8]) -> Result<()> {
    let cb = vol.cluster_bytes();
    let mut done = 0usize;
    while done < buf.len() {
        let at = pos + done as u32;
        let n = (buf.len() - done).min((cb - at % cb) as usize);
        f.seek(vol, at).map_err(err)?;
        f.write_all(vol, &buf[done..done + n]).map_err(err)?;
        done += n;
    }
    Ok(())
}

/// `path` as fstool takes it: absolute, without a trailing slash.
fn abs(path: &str) -> Result<String> {
    let p = path.trim_matches('/');
    if p.split('/').any(|c| c.is_empty() || c == "." || c == "..") && !p.is_empty() {
        return Err(Error::Invalid);
    }
    Ok(format!("/{p}"))
}

/// Whether `path` is `dir` or below it (FAT names ignore case).
fn within(path: &str, dir: &str) -> bool {
    let (p, d) = (path.as_bytes(), dir.as_bytes());
    p.len() >= d.len() && p[..d.len()].eq_ignore_ascii_case(d) && (p.len() == d.len() || p[d.len()] == b'/')
}

/// A name as `list` shows it: one that could be a short name alone, in
/// upper case, is shown in lower case (`MSG` as `msg`).
fn shown_name(name: &str) -> String {
    if raw::is_short_upper(name) { name.to_ascii_lowercase() } else { String::from(name) }
}

/// FNV-1a of the path, ignoring case: an id for `stat`, never 0 or 1 (the root).
fn path_id(path: &str) -> u32 {
    let h = path.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b.to_ascii_lowercase())).wrapping_mul(0x0100_0193));
    h.max(2)
}

/// Seconds since the epoch of a FAT date and time (taken as UTC).
fn unix(t: Timestamp) -> u32 {
    let (year, month, day) = (1980 + i64::from(t.date >> 9), i64::from((t.date >> 5) & 0xF), i64::from(t.date & 0x1F));
    if month == 0 || day == 0 {
        return 0;
    }
    let secs = i64::from(t.time >> 11) * 3600 + i64::from((t.time >> 5) & 0x3F) * 60 + i64::from(t.time & 0x1F) * 2;
    (days_from_civil(year, month, day) * 86_400 + secs).clamp(0, i64::from(u32::MAX)) as u32
}

/// The FAT timestamp of seconds since the epoch (clamped to 1980..=2107).
pub fn timestamp(secs: u32) -> Timestamp {
    let days = i64::from(secs / 86_400);
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    Timestamp::from_ymd_hms(
        y.clamp(0, i64::from(u16::MAX)) as u16,
        m as u8,
        d as u8,
        (rem / 3600) as u8,
        (rem / 60 % 60) as u8,
        (rem % 60) as u8,
    )
}

// Howard Hinnant's civil-calendar algorithms (public domain).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}
