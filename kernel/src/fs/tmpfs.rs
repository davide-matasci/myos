//! tmpfs: small in-memory writable tree mounted at `/tmp/…`.
//!
//! Supports regular files, directories, symlinks and named FIFOs. Paths are relative to the
//! mount (no leading slash), e.g. `ci`, `d/f`.
//!
//! The entries sit in slots, found by two ordered maps: their whole path to their slot, and
//! their id (what an open file names it by) to their slot. A lookup by either is
//! logarithmic and allocates nothing (the kernel heap's allocator walks its free list on
//! every allocation), and a directory's descendants are one range of the path map (the
//! paths with its prefix and a `/`), which its listing, its emptiness check and its rename
//! walk. A plain list scanned on every lookup made unpacking os-test's 7500 files
//! quadratic: five minutes of its install under TCG.

use alloc::collections::BTreeMap;
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
/// bounds the whole tmpfs; this caps one file: twice the largest the system
/// keeps there (the rust app's libcore rlib, 67 MB, in `/tmp/apps`).
const FILE_CAP: usize = 128 * 1024 * 1024;
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

/// An entry; its path is its key in [`Tree::by_path`]: for a file unlinked
/// while held (`unlink_keep`) `\0` and its id, a name no path reaches (the
/// VFS refuses a NUL) and listings leave out.
struct Entry {
    /// The file's id (`vfs::FileOps`) and inode number: unique until reboot.
    id: u64,
    kind: Kind,
    /// Seconds since the epoch: set at creation and by `set_times`; reads
    /// do not change `atime`.
    atime: u64,
    /// Seconds since the epoch: creation, a write or truncate, a directory's
    /// entries changing, `set_times`.
    mtime: u64,
}

impl Entry {
    fn new(kind: Kind, now: u64) -> Entry {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        Entry { id, kind, atime: now, mtime: now }
    }
}

/// The tmpfs's entries (see the module's comment): a path is stored once, as
/// its key in `by_path`.
struct Tree {
    slots: Vec<Option<Entry>>,
    free: Vec<usize>,
    by_path: BTreeMap<String, usize>,
    by_id: BTreeMap<u64, usize>,
}

impl Tree {
    const fn new() -> Tree {
        Tree { slots: Vec::new(), free: Vec::new(), by_path: BTreeMap::new(), by_id: BTreeMap::new() }
    }

    fn len(&self) -> usize {
        self.by_path.len()
    }

    fn slot(&self, i: usize) -> Option<&Entry> {
        self.slots.get(i)?.as_ref()
    }

    fn slot_mut(&mut self, i: usize) -> Option<&mut Entry> {
        self.slots.get_mut(i)?.as_mut()
    }

    fn get(&self, path: &str) -> Option<&Entry> {
        self.slot(*self.by_path.get(path)?)
    }

    fn get_mut(&mut self, path: &str) -> Option<&mut Entry> {
        let i = *self.by_path.get(path)?;
        self.slot_mut(i)
    }

    fn insert(&mut self, path: String, entry: Entry) {
        let id = entry.id;
        let i = match self.free.pop() {
            Some(i) => {
                self.slots[i] = Some(entry);
                i
            }
            None => {
                self.slots.push(Some(entry));
                self.slots.len() - 1
            }
        };
        self.by_id.insert(id, i);
        self.by_path.insert(path, i);
    }

    fn remove(&mut self, path: &str) -> Option<Entry> {
        let i = self.by_path.remove(path)?;
        let entry = self.slots[i].take()?;
        self.by_id.remove(&entry.id);
        self.free.push(i);
        Some(entry)
    }

    /// Give the entry at `old` the path `new` (nothing there).
    fn move_to(&mut self, old: &str, new: String) {
        if let Some(i) = self.by_path.remove(old) {
            self.by_path.insert(new, i);
        }
    }

    /// The paths below the directory `dir` (not `dir` itself), in order: the
    /// keys after `dir` that start with it and a `/`. Those that go on with a
    /// byte below `/` (`dir-x`, `dir.y`) sort between `dir` and them.
    fn below<'a>(&'a self, dir: &'a str) -> impl Iterator<Item = (&'a String, usize)> + 'a {
        use core::ops::Bound;
        let next = move |path: &String| path.starts_with(dir) && path.len() > dir.len();
        self.by_path
            .range::<str, _>((Bound::Excluded(dir), Bound::Unbounded))
            .skip_while(move |(path, _)| next(path) && path.as_bytes()[dir.len()] < b'/')
            .take_while(move |(path, _)| next(path) && path.as_bytes()[dir.len()] == b'/')
            .map(|(path, i)| (path, *i))
    }
}

/// The next file id (1 is the mount root's inode number).
static NEXT_ID: AtomicU64 = AtomicU64::new(2);

/// The tmpfs's files by id: what an open file uses ([`crate::fs::vfs::FileOps`]).
pub const FILES: crate::fs::vfs::FileOps = crate::fs::vfs::FileOps {
    id: file_id,
    read: |id, pos, out| read_at(At::Id(id), pos, out),
    write: |id, pos, buf| write_at(At::Id(id), pos, buf),
    stat: |id| stat_at(At::Id(id)),
    set_size: |id, size| set_size_at(At::Id(id), size),
    set_times: |id, atime, mtime| set_times_at(At::Id(id), atime, mtime),
    unlink_keep,
    forget,
};

/// How a call names its entry: by path in the mount, or by the id an open
/// file keeps.
#[derive(Clone, Copy)]
enum At<'a> {
    Path(&'a str),
    Id(u64),
}

/// The slot of the entry `at` names, if it exists.
fn slot_at(entries: &Tree, at: At) -> Option<usize> {
    match at {
        At::Path(name) if valid_rel_path(name) => entries.by_path.get(name).copied(),
        At::Path(_) => None,
        At::Id(id) => entries.by_id.get(&id).copied(),
    }
}

fn entry_at<'a>(entries: &'a Tree, at: At) -> Option<&'a Entry> {
    entries.slot(slot_at(entries, at)?)
}

fn entry_at_mut<'a>(entries: &'a mut Tree, at: At) -> Option<&'a mut Entry> {
    let i = slot_at(entries, at)?;
    entries.slot_mut(i)
}

/// The id of the entry at `name`.
fn file_id(name: &str) -> Option<u64> {
    let entries = ENTRIES.lock();
    entry_at(&entries, At::Path(name)).map(|e| e.id)
}

/// Unlink the file at `name` but keep it, by its id, for whoever holds it.
fn unlink_keep(name: &str) -> bool {
    let now = now();
    let mut entries = ENTRIES.lock();
    let Some(entry) = entry_at(&entries, At::Path(name)) else {
        return false;
    };
    if !matches!(entry.kind, Kind::File(_)) {
        return false;
    }
    let kept = alloc::format!("\0{}", entry.id);
    entries.move_to(name, kept);
    touch_dir(&mut entries, parent_of(name), now);
    true
}

/// A new empty file that no name reaches, kept as `unlink_keep` keeps one
/// (`vfs::anon_file`): its id.
pub fn create_kept() -> Option<u64> {
    let now = now();
    let mut entries = ENTRIES.lock();
    if entries.len() >= MAX_ENTRIES {
        return None;
    }
    let entry = Entry::new(Kind::File(Vec::new()), now);
    let id = entry.id;
    entries.insert(alloc::format!("\0{id}"), entry);
    Some(id)
}

/// The file `unlink_keep` kept is let go.
fn forget(id: u64) {
    let mut entries = ENTRIES.lock();
    let kept = alloc::format!("\0{id}");
    if entries.by_path.contains_key(kept.as_str()) {
        entries.remove(&kept);
    }
}

static ENTRIES: Mutex<Tree> = Mutex::new(Tree::new());

/// How full tmpfs is (`statvfs`): its files' bytes are in the kernel heap,
/// so the heap is its space, and [`MAX_ENTRIES`] its file nodes.
pub fn statfs() -> myos_abi::VfsStatFs {
    const BLOCK: u64 = 4096;
    let (size, free) = crate::heap::usage();
    let used = ENTRIES.lock().len() as u64;
    myos_abi::VfsStatFs {
        bsize: BLOCK,
        blocks: size as u64 / BLOCK,
        bfree: free as u64 / BLOCK,
        bavail: free as u64 / BLOCK,
        files: MAX_ENTRIES as u64,
        ffree: (MAX_ENTRIES as u64).saturating_sub(used),
        namemax: 255,
    }
}
/// The mount root's times (it has no entry).
static ROOT_ATIME: AtomicU64 = AtomicU64::new(0);
static ROOT_MTIME: AtomicU64 = AtomicU64::new(0);

/// The current time for a change, read before taking `ENTRIES` (reading the
/// clock may go to the RTC).
fn now() -> u64 {
    crate::time::unix_seconds().unwrap_or(0).max(0) as u64
}

/// A directory's entries changed: its modification time is now.
fn touch_dir(entries: &mut Tree, dir: &str, now: u64) {
    if dir.is_empty() {
        ROOT_MTIME.store(now, Ordering::Relaxed);
    } else if let Some(entry) = entries.get_mut(dir) {
        entry.mtime = now;
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

fn is_dir_path(entries: &Tree, path: &str) -> bool {
    path.is_empty() || matches!(entries.get(path).map(|e| &e.kind), Some(Kind::Dir))
}

fn has_children(entries: &Tree, dir: &str) -> bool {
    if dir.is_empty() {
        return entries.len() != 0;
    }
    entries.below(dir).next().is_some()
}

fn parent_ok(entries: &Tree, path: &str) -> bool {
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
    if let Some(entry) = entries.get(name) {
        return matches!(entry.kind, Kind::File(_));
    }
    if !parent_ok(&entries, name) {
        return false;
    }
    if entries.len() >= MAX_ENTRIES {
        return false;
    }
    entries.insert(String::from(name), Entry::new(Kind::File(Vec::new()), now));
    touch_dir(&mut entries, parent_of(name), now);
    true
}

pub fn truncate(name: &str) -> bool {
    if !valid_rel_path(name) {
        return false;
    }
    let now = now();
    let mut entries = ENTRIES.lock();
    let Some(entry) = entries.get_mut(name) else {
        return false;
    };
    match &mut entry.kind {
        Kind::File(data) => {
            data.clear();
            entry.mtime = now;
            true
        }
        _ => false,
    }
}

/// Make the file `name` `size` bytes long: cut, or grown with zeros.
pub fn set_size(name: &str, size: usize) -> bool {
    set_size_at(At::Path(name), size)
}

fn set_size_at(at: At, size: usize) -> bool {
    if size > FILE_CAP {
        return false;
    }
    let now = now();
    let mut entries = ENTRIES.lock();
    let Some(entry) = entry_at_mut(&mut entries, at) else {
        return false;
    };
    let Kind::File(data) = &mut entry.kind else {
        return false;
    };
    if size > data.len() && data.try_reserve_exact(size - data.len()).is_err() {
        return false;
    }
    data.resize(size, 0);
    if size < data.capacity() / 2 {
        data.shrink_to_fit();
    }
    entry.mtime = now;
    true
}

pub fn read(name: &str, pos: usize, out: &mut [u8]) -> usize {
    read_at(At::Path(name), pos, out)
}

fn read_at(at: At, pos: usize, out: &mut [u8]) -> usize {
    let entries = ENTRIES.lock();
    let Some(entry) = entry_at(&entries, at) else {
        return 0;
    };
    let Kind::File(data) = &entry.kind else {
        return 0;
    };
    let n = out.len().min(data.len().saturating_sub(pos));
    if n != 0 {
        out[..n].copy_from_slice(&data[pos..pos + n]);
    }
    n
}

pub fn write(name: &str, pos: usize, buf: &[u8]) -> Option<usize> {
    write_at(At::Path(name), pos, buf)
}

fn write_at(at: At, pos: usize, buf: &[u8]) -> Option<usize> {
    let now = now();
    let mut entries = ENTRIES.lock();
    let entry = entry_at_mut(&mut entries, at)?;
    let Kind::File(data) = &mut entry.kind else {
        return None;
    };
    // A write past the end leaves zeros in the gap.
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
    if entries.get(name).is_some() || !parent_ok(&entries, name) || entries.len() >= MAX_ENTRIES {
        return false;
    }
    let Some(id) = crate::pipe::alloc_named() else {
        return false;
    };
    entries.insert(String::from(name), Entry::new(Kind::Fifo(id), now));
    touch_dir(&mut entries, parent_of(name), now);
    true
}

/// Pipe slot behind the FIFO at `name`, if `name` is one.
pub fn fifo_id(name: &str) -> Option<usize> {
    if !valid_rel_path(name) {
        return None;
    }
    let entries = ENTRIES.lock();
    match entries.get(name)?.kind {
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
    if entries.get(name).is_some() {
        return false;
    }
    if !parent_ok(&entries, name) {
        return false;
    }
    if entries.len() >= MAX_ENTRIES {
        return false;
    }
    entries.insert(String::from(name), Entry::new(Kind::Dir, now));
    touch_dir(&mut entries, parent_of(name), now);
    true
}

pub fn rmdir(name: &str) -> bool {
    if !valid_rel_path(name) {
        return false;
    }
    let now = now();
    let mut entries = ENTRIES.lock();
    let Some(entry) = entries.get(name) else {
        return false;
    };
    if !matches!(entry.kind, Kind::Dir) {
        return false;
    }
    if has_children(&entries, name) {
        return false;
    }
    entries.remove(name);
    touch_dir(&mut entries, parent_of(name), now);
    true
}

pub fn unlink(name: &str) -> bool {
    if !valid_rel_path(name) {
        return false;
    }
    let now = now();
    let mut entries = ENTRIES.lock();
    let Some(entry) = entries.get(name) else {
        return false;
    };
    match entry.kind {
        Kind::File(_) => {}
        Kind::Symlink(_) => {
            SYMLINKS.fetch_sub(1, Ordering::Relaxed);
        }
        Kind::Fifo(id) => crate::pipe::fifo_unlink(id),
        Kind::Dir => return false,
    }
    entries.remove(name);
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

fn rename_locked(entries: &mut Tree, old: &str, new: &str) -> bool {
    // Refuse renaming a directory into itself.
    if new.starts_with(old) && new.as_bytes().get(old.len()) == Some(&b'/') {
        return false;
    }
    let Some(entry) = entries.get(old) else {
        return false;
    };
    let is_dir = matches!(entry.kind, Kind::Dir);

    // POSIX: file/symlink rename may replace an existing non-directory dest.
    // Without this, git init dies on the second config write (config.lock →
    // config) after core.repositoryformatversion already created config —
    // commit_lock_file rename failed with ENOENT on all boot arches.
    if let Some(dest) = entries.get(new) {
        if is_dir || matches!(dest.kind, Kind::Dir) {
            return false;
        }
        if let Kind::Fifo(id) = dest.kind {
            crate::pipe::fifo_unlink(id);
        }
        entries.remove(new);
        entries.move_to(old, String::from(new));
        return true;
    }

    if !parent_ok(entries, new) {
        return false;
    }

    if is_dir {
        let below: Vec<String> = entries.below(old).map(|(path, _)| path.clone()).collect();
        // Validate all new paths fit before mutating.
        if below.iter().any(|path| new.len() + path.len() - old.len() > PATH_CAP) {
            return false;
        }
        for path in below {
            let mut np = String::from(new);
            np.push_str(&path[old.len()..]);
            entries.move_to(&path, np);
        }
    }
    entries.move_to(old, String::from(new));
    true
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
    if entries.get(linkpath).is_some() {
        return false;
    }
    if !parent_ok(&entries, linkpath) {
        return false;
    }
    if entries.len() >= MAX_ENTRIES {
        return false;
    }
    entries.insert(String::from(linkpath), Entry::new(Kind::Symlink(String::from(target)), now));
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
    let Kind::Symlink(target) = &entries.get(path)?.kind else {
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
    // The mount root's children are the paths without a `/`; a directory's
    // are the ones of its range without one past its prefix.
    let children: alloc::boxed::Box<dyn Iterator<Item = &str>> = if dir.is_empty() {
        // A kept file (`\0` and its id) has no name.
        alloc::boxed::Box::new(
            entries.by_path.keys().map(String::as_str).filter(|p| !p.contains('/') && !p.starts_with('\0')),
        )
    } else {
        alloc::boxed::Box::new(
            entries.below(dir).map(move |(p, _)| &p[dir.len() + 1..]).filter(|rest| !rest.contains('/')),
        )
    };
    for child in children {
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
    stat_at(At::Path(name))
}

fn stat_at(at: At) -> Option<StatInfo> {
    let entries = ENTRIES.lock();
    let entry = entry_at(&entries, at)?;
    let (mode, size, nlink) = match &entry.kind {
        Kind::Dir => (S_IFDIR | 0o755, 0u64, 2u32),
        Kind::File(data) => (S_IFREG | 0o755, data.len() as u64, 1u32),
        Kind::Symlink(t) => (S_IFLNK | 0o777, t.len() as u64, 1u32),
        Kind::Fifo(_) => (S_IFIFO | 0o644, 0u64, 1u32),
    };
    Some(StatInfo {
        mode,
        size,
        ino: entry.id as u32,
        nlink,
        dev: 0,
        mtime: entry.mtime,
        atime: entry.atime,
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
    set_times_at(At::Path(name), atime, mtime)
}

fn set_times_at(at: At, atime: Option<u64>, mtime: Option<u64>) -> bool {
    let mut entries = ENTRIES.lock();
    let Some(entry) = entry_at_mut(&mut entries, at) else {
        return false;
    };
    if let Some(t) = atime {
        entry.atime = t;
    }
    if let Some(t) = mtime {
        entry.mtime = t;
    }
    true
}
