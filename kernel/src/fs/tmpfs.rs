//! tmpfs: small in-memory writable tree mounted at `/tmp/…`.
//!
//! Supports regular files, directories, symlinks and named FIFOs. Paths are relative to the
//! mount (no leading slash), e.g. `ci`, `d/f`.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use crate::fs::StatInfo;

// os-test alone is ~6.5k files; cp -R of a port tree must not hit an
// arbitrary cap (creat fails with ENOENT once mkdir stops succeeding).
const MAX_ENTRIES: usize = 16384;
const COMP_CAP: usize = 255;
const PATH_CAP: usize = 255;
/// File data lives in the kernel heap (sized from memory, see `heap`), which
/// bounds the whole tmpfs; this caps one file.
const FILE_CAP: usize = 16 * 1024 * 1024;
const LINK_CAP: usize = 255;

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;
const S_IFIFO: u32 = 0o010000;

#[derive(Clone)]
enum Kind {
    Dir,
    File(Vec<u8>),
    Symlink(String),
    /// Named pipe (`mkfifo`): the kernel pipe slot it owns (see `pipe`).
    Fifo(usize),
}

struct Entry {
    path: String,
    kind: Kind,
    /// Seconds since the epoch: set at creation and by `set_times`; reads
    /// do not change `atime`.
    atime: u64,
    /// Seconds since the epoch: creation, a write or truncate, a directory's
    /// entries changing, `set_times`.
    mtime: u64,
}

impl Entry {
    fn new(path: &str, kind: Kind, now: u64) -> Entry {
        Entry { path: String::from(path), kind, atime: now, mtime: now }
    }
}

static ENTRIES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
/// The mount root's times (it has no entry).
static ROOT_ATIME: AtomicU64 = AtomicU64::new(0);
static ROOT_MTIME: AtomicU64 = AtomicU64::new(0);

/// The current time for a change, read before taking `ENTRIES` (reading the
/// clock may go to the RTC).
fn now() -> u64 {
    crate::time::unix_seconds().unwrap_or(0).max(0) as u64
}

/// A directory's entries changed: its modification time is now.
fn touch_dir(entries: &mut [Entry], dir: &str, now: u64) {
    if dir.is_empty() {
        ROOT_MTIME.store(now, Ordering::Relaxed);
    } else if let Some(i) = find_index(entries, dir) {
        entries[i].mtime = now;
    }
}

fn valid_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && name.len() <= COMP_CAP
}

fn valid_rel_path(path: &str) -> bool {
    if path.is_empty() || path.len() > PATH_CAP || path.starts_with('/') || path.ends_with('/') {
        return false;
    }
    if path.contains("//") {
        return false;
    }
    path.split('/').all(valid_component)
}

fn parent_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}

fn find_index(entries: &[Entry], path: &str) -> Option<usize> {
    entries.iter().position(|e| e.path == path)
}

fn is_dir_path(entries: &[Entry], path: &str) -> bool {
    if path.is_empty() {
        return true;
    }
    match find_index(entries, path).map(|i| &entries[i].kind) {
        Some(Kind::Dir) => true,
        _ => false,
    }
}

fn has_children(entries: &[Entry], dir: &str) -> bool {
    for e in entries.iter() {
        if dir.is_empty() {
            return true;
        }
        if e.path.starts_with(dir) && e.path.as_bytes().get(dir.len()) == Some(&b'/') {
            return true;
        }
    }
    false
}

fn parent_ok(entries: &[Entry], path: &str) -> bool {
    is_dir_path(entries, parent_of(path))
}

/// RO mounts expect lookup; tmpfs stores mutable bytes so this always returns None.
pub fn lookup(_name: &str) -> Option<&'static [u8]> {
    None
}

pub fn register(_name: &str, _bytes: &'static [u8]) -> bool {
    false
}

pub fn create(name: &str) -> bool {
    if !valid_rel_path(name) {
        return false;
    }
    let now = now();
    let mut entries = ENTRIES.lock();
    if let Some(i) = find_index(&entries, name) {
        return matches!(entries[i].kind, Kind::File(_));
    }
    if !parent_ok(&entries, name) {
        return false;
    }
    if entries.len() >= MAX_ENTRIES {
        return false;
    }
    entries.push(Entry::new(name, Kind::File(Vec::new()), now));
    touch_dir(&mut entries, parent_of(name), now);
    true
}

pub fn truncate(name: &str) -> bool {
    if !valid_rel_path(name) {
        return false;
    }
    let now = now();
    let mut entries = ENTRIES.lock();
    let Some(i) = find_index(&entries, name) else {
        return false;
    };
    match &mut entries[i].kind {
        Kind::File(data) => {
            data.clear();
            entries[i].mtime = now;
            true
        }
        _ => false,
    }
}

pub fn read(name: &str, pos: usize, out: &mut [u8]) -> usize {
    if !valid_rel_path(name) {
        return 0;
    }
    let entries = ENTRIES.lock();
    let Some(i) = find_index(&entries, name) else {
        return 0;
    };
    let Kind::File(data) = &entries[i].kind else {
        return 0;
    };
    let n = out.len().min(data.len().saturating_sub(pos));
    if n != 0 {
        out[..n].copy_from_slice(&data[pos..pos + n]);
    }
    n
}

pub fn write(name: &str, pos: usize, buf: &[u8]) -> Option<usize> {
    if !valid_rel_path(name) {
        return None;
    }
    let now = now();
    let mut entries = ENTRIES.lock();
    let Some(i) = find_index(&entries, name) else {
        return None;
    };
    let entry = &mut entries[i];
    let Kind::File(data) = &mut entry.kind else {
        return None;
    };
    if pos > data.len() {
        return None;
    }
    let end = pos.checked_add(buf.len())?;
    if end > FILE_CAP {
        return None;
    }
    if end > data.len() {
        // Sequential writes grow the file a block at a time: reserve ahead,
        // but by at most 1 MiB so a large file does not double the heap use.
        let extra = end - data.len();
        let ahead = data.len().min(1 << 20).max(extra);
        if data.try_reserve(ahead.min(FILE_CAP - data.len())).is_err() && data.try_reserve_exact(extra).is_err() {
            return None;
        }
        data.resize(end, 0);
    }
    data[pos..end].copy_from_slice(buf);
    entry.mtime = now;
    Some(buf.len())
}

/// mkfifo(2): create a named pipe at `name` (fails if anything exists there).
pub fn mkfifo(name: &str) -> bool {
    if !valid_rel_path(name) {
        return false;
    }
    let now = now();
    let mut entries = ENTRIES.lock();
    if find_index(&entries, name).is_some()
        || !parent_ok(&entries, name)
        || entries.len() >= MAX_ENTRIES
    {
        return false;
    }
    let Some(id) = crate::pipe::alloc_named() else {
        return false;
    };
    entries.push(Entry::new(name, Kind::Fifo(id), now));
    touch_dir(&mut entries, parent_of(name), now);
    true
}

/// Pipe slot behind the FIFO at `name`, if `name` is one.
pub fn fifo_id(name: &str) -> Option<usize> {
    if !valid_rel_path(name) {
        return None;
    }
    let entries = ENTRIES.lock();
    match entries[find_index(&entries, name)?].kind {
        Kind::Fifo(id) => Some(id),
        _ => None,
    }
}

pub fn mkdir(name: &str) -> bool {
    if !valid_rel_path(name) {
        return false;
    }
    let now = now();
    let mut entries = ENTRIES.lock();
    if find_index(&entries, name).is_some() {
        return false;
    }
    if !parent_ok(&entries, name) {
        return false;
    }
    if entries.len() >= MAX_ENTRIES {
        return false;
    }
    entries.push(Entry::new(name, Kind::Dir, now));
    touch_dir(&mut entries, parent_of(name), now);
    true
}

pub fn rmdir(name: &str) -> bool {
    if !valid_rel_path(name) {
        return false;
    }
    let now = now();
    let mut entries = ENTRIES.lock();
    let Some(i) = find_index(&entries, name) else {
        return false;
    };
    if !matches!(entries[i].kind, Kind::Dir) {
        return false;
    }
    if has_children(&entries, name) {
        return false;
    }
    entries.remove(i);
    touch_dir(&mut entries, parent_of(name), now);
    true
}

pub fn unlink(name: &str) -> bool {
    if !valid_rel_path(name) {
        return false;
    }
    let now = now();
    let mut entries = ENTRIES.lock();
    let Some(i) = find_index(&entries, name) else {
        return false;
    };
    match entries[i].kind {
        Kind::File(_) => {}
        Kind::Symlink(_) => {
            SYMLINKS.fetch_sub(1, Ordering::Relaxed);
        }
        Kind::Fifo(id) => crate::pipe::fifo_unlink(id),
        Kind::Dir => return false,
    }
    entries.remove(i);
    touch_dir(&mut entries, parent_of(name), now);
    true
}

pub fn rename(old: &str, new: &str) -> bool {
    if !valid_rel_path(old) || !valid_rel_path(new) {
        return false;
    }
    if old == new {
        return true;
    }
    let now = now();
    let mut entries = ENTRIES.lock();
    if !rename_locked(&mut entries, old, new) {
        return false;
    }
    touch_dir(&mut entries, parent_of(old), now);
    touch_dir(&mut entries, parent_of(new), now);
    true
}

fn rename_locked(entries: &mut Vec<Entry>, old: &str, new: &str) -> bool {
    // Refuse renaming a directory into itself.
    if new.starts_with(old) && new.as_bytes().get(old.len()) == Some(&b'/') {
        return false;
    }
    let Some(old_i) = find_index(entries, old) else {
        return false;
    };
    let is_dir = matches!(entries[old_i].kind, Kind::Dir);

    // POSIX: file/symlink rename may replace an existing non-directory dest.
    // Without this, git init dies on the second config write (config.lock →
    // config) after core.repositoryformatversion already created config —
    // commit_lock_file rename failed with ENOENT on all boot arches.
    if let Some(new_i) = find_index(entries, new) {
        if is_dir || matches!(entries[new_i].kind, Kind::Dir) {
            return false;
        }
        if old_i == new_i {
            return true;
        }
        if let Kind::Fifo(id) = entries[new_i].kind {
            crate::pipe::fifo_unlink(id);
        }
        entries.remove(new_i);
        let old_i = if new_i < old_i { old_i - 1 } else { old_i };
        entries[old_i].path = String::from(new);
        return true;
    }

    if !parent_ok(entries, new) {
        return false;
    }

    if is_dir {
        let mut idxs: Vec<usize> = Vec::new();
        for (i, e) in entries.iter().enumerate() {
            if e.path == old
                || (e.path.starts_with(old) && e.path.as_bytes().get(old.len()) == Some(&b'/'))
            {
                idxs.push(i);
            }
        }
        // Validate all new paths fit before mutating.
        for &i in idxs.iter() {
            let suffix = if entries[i].path.len() == old.len() {
                ""
            } else {
                &entries[i].path[old.len()..]
            };
            if new.len() + suffix.len() > PATH_CAP {
                return false;
            }
        }
        for i in idxs {
            let suffix = if entries[i].path.len() == old.len() {
                String::new()
            } else {
                String::from(&entries[i].path[old.len()..])
            };
            let mut np = String::from(new);
            np.push_str(&suffix);
            entries[i].path = np;
        }
        true
    } else {
        entries[old_i].path = String::from(new);
        true
    }
}

pub fn symlink(target: &str, linkpath: &str) -> bool {
    if !valid_rel_path(linkpath) {
        return false;
    }
    if target.is_empty() || target.len() > LINK_CAP {
        return false;
    }
    let now = now();
    let mut entries = ENTRIES.lock();
    if find_index(&entries, linkpath).is_some() {
        return false;
    }
    if !parent_ok(&entries, linkpath) {
        return false;
    }
    if entries.len() >= MAX_ENTRIES {
        return false;
    }
    entries.push(Entry::new(linkpath, Kind::Symlink(String::from(target)), now));
    touch_dir(&mut entries, parent_of(linkpath), now);
    SYMLINKS.fetch_add(1, Ordering::Relaxed);
    true
}

/// Symlinks currently in the tmpfs (a rename over one may leave this too
/// high, which only costs lookups).
static SYMLINKS: AtomicUsize = AtomicUsize::new(0);

/// Whether the tmpfs holds any symlink (the only filesystem that can): path
/// resolution skips its per-component symlink checks otherwise.
pub fn has_symlinks() -> bool {
    SYMLINKS.load(Ordering::Relaxed) != 0
}

pub fn readlink(path: &str, buf: &mut [u8]) -> Option<usize> {
    if !has_symlinks() || !valid_rel_path(path) {
        return None;
    }
    let entries = ENTRIES.lock();
    let i = find_index(&entries, path)?;
    let Kind::Symlink(target) = &entries[i].kind else {
        return None;
    };
    let n = target.len().min(buf.len());
    buf[..n].copy_from_slice(&target.as_bytes()[..n]);
    Some(n)
}

pub fn listdir_at(rel: &str, buf: &mut [u8]) -> usize {
    let dir = if rel.is_empty() || rel == "." {
        ""
    } else if valid_rel_path(rel) {
        rel
    } else {
        return 0;
    };
    let entries = ENTRIES.lock();
    if !dir.is_empty() && !is_dir_path(&entries, dir) {
        return 0;
    }
    let mut n = 0;
    for e in entries.iter() {
        let child = if dir.is_empty() {
            if e.path.contains('/') {
                continue;
            }
            e.path.as_str()
        } else if e.path.starts_with(dir)
            && e.path.as_bytes().get(dir.len()) == Some(&b'/')
        {
            let rest = &e.path[dir.len() + 1..];
            if rest.contains('/') {
                continue;
            }
            rest
        } else {
            continue;
        };
        let name = child.as_bytes();
        let need = name.len() + 1;
        if n + need > buf.len() {
            break;
        }
        buf[n..n + name.len()].copy_from_slice(name);
        n += name.len();
        buf[n] = b'\n';
        n += 1;
    }
    n
}

pub fn stat(name: &str) -> Option<StatInfo> {
    if name.is_empty() || name == "." || name == ".." {
        return Some(StatInfo {
            mode: S_IFDIR | 0o755,
            size: 0,
            ino: 1,
            nlink: 2,
            dev: 0,
            mtime: ROOT_MTIME.load(Ordering::Relaxed),
            atime: ROOT_ATIME.load(Ordering::Relaxed),
        });
    }
    if !valid_rel_path(name) {
        return None;
    }
    let entries = ENTRIES.lock();
    let i = find_index(&entries, name)?;
    let (mode, size, nlink) = match &entries[i].kind {
        Kind::Dir => (S_IFDIR | 0o755, 0u32, 2u32),
        Kind::File(data) => (S_IFREG | 0o755, data.len() as u32, 1u32),
        Kind::Symlink(t) => (S_IFLNK | 0o777, t.len() as u32, 1u32),
        Kind::Fifo(_) => (S_IFIFO | 0o644, 0u32, 1u32),
    };
    Some(StatInfo {
        mode,
        size,
        ino: (i as u32) + 2,
        nlink,
        dev: 0,
        mtime: entries[i].mtime,
        atime: entries[i].atime,
    })
}

/// Set a path's access and modification times (`None` keeps one); the mount
/// root (`""`) included.
pub fn set_times(name: &str, atime: Option<u64>, mtime: Option<u64>) -> bool {
    if name.is_empty() || name == "." {
        if let Some(t) = atime {
            ROOT_ATIME.store(t, Ordering::Relaxed);
        }
        if let Some(t) = mtime {
            ROOT_MTIME.store(t, Ordering::Relaxed);
        }
        return true;
    }
    if !valid_rel_path(name) {
        return false;
    }
    let mut entries = ENTRIES.lock();
    let Some(i) = find_index(&entries, name) else {
        return false;
    };
    if let Some(t) = atime {
        entries[i].atime = t;
    }
    if let Some(t) = mtime {
        entries[i].mtime = t;
    }
    true
}
