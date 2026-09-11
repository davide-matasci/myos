//! tmpfs: small in-memory writable tree mounted at `/tmp/…`.
//!
//! Supports regular files, directories, and symlinks. Paths are relative to the
//! mount (no leading slash), e.g. `ci`, `d/f`.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::fs::StatInfo;

const MAX_ENTRIES: usize = 256;
const COMP_CAP: usize = 64;
const PATH_CAP: usize = 128;
const FILE_CAP: usize = 262144;
const LINK_CAP: usize = 64;

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;

// CI diagnostic: last-write content snapshot per tmpfs file (name → clone).
// Every read verifies its chunk against the snapshot so chunked stdio reads
// (git's 2 KB index.lock reads) also prove whether tmpfs bytes changed
// underneath us — distinguishes kernel-heap corruption from a userspace
// parser bug.
static TFS_HASH: Mutex<Vec<(String, Vec<u8>)>> = Mutex::new(Vec::new());

fn fnv(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in b {
        h = (h ^ x as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

#[derive(Clone)]
enum Kind {
    Dir,
    File(Vec<u8>),
    Symlink(String),
}

struct Entry {
    path: String,
    kind: Kind,
}

static ENTRIES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

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
    entries.push(Entry {
        path: String::from(name),
        kind: Kind::File(Vec::new()),
    });
    true
}

pub fn truncate(name: &str) -> bool {
    if !valid_rel_path(name) {
        return false;
    }
    let mut entries = ENTRIES.lock();
    let Some(i) = find_index(&entries, name) else {
        return false;
    };
    match &mut entries[i].kind {
        Kind::File(data) => {
            data.clear();
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
        // Chunk-level verify: every read range must match the last-write
        // snapshot; a mismatch proves kernel-side corruption of the Vec.
        if let Some(snap) = TFS_HASH.lock().iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()) {
            let end = pos + n;
            if snap.len() >= end && snap[pos..end] != data[pos..end] {
                crate::console::status_fail(&alloc::format!(
                    "tmpfs: chunk mismatch for {name} at {pos}..{end}\n"
                ));
                crate::console::flush();
                panic!("tmpfs: chunk mismatch");
            }
        }
    }
    n
}

pub fn write(name: &str, pos: usize, buf: &[u8]) -> Option<usize> {
    if !valid_rel_path(name) {
        return None;
    }
    let mut entries = ENTRIES.lock();
    let Some(i) = find_index(&entries, name) else {
        return None;
    };
    let Kind::File(data) = &mut entries[i].kind else {
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
        data.resize(end, 0);
    }
    data[pos..end].copy_from_slice(buf);
    // CI diagnostic: remember the content hash so the next full read can
    // prove whether tmpfs bytes changed underneath us (kernel-heap
    // corruption of the backing Vec) — distinguishes kernel-side corruption
    // from a userspace parser bug.
    {
        let mut hs = TFS_HASH.lock();
        let key = String::from(name);
        let snap = data[..end].to_vec();
        if let Some(slot) = hs.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = snap;
        } else {
            hs.push((key, snap));
        }
    }
    Some(buf.len())
}

pub fn mkdir(name: &str) -> bool {
    if !valid_rel_path(name) {
        return false;
    }
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
    entries.push(Entry {
        path: String::from(name),
        kind: Kind::Dir,
    });
    true
}

pub fn rmdir(name: &str) -> bool {
    if !valid_rel_path(name) {
        return false;
    }
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
    true
}

pub fn unlink(name: &str) -> bool {
    if !valid_rel_path(name) {
        return false;
    }
    let mut entries = ENTRIES.lock();
    let Some(i) = find_index(&entries, name) else {
        return false;
    };
    match entries[i].kind {
        Kind::File(_) | Kind::Symlink(_) => {
            entries.remove(i);
            true
        }
        Kind::Dir => false,
    }
}

pub fn rename(old: &str, new: &str) -> bool {
    if !valid_rel_path(old) || !valid_rel_path(new) {
        return false;
    }
    if old == new {
        return true;
    }
    // Refuse renaming a directory into itself.
    if new.starts_with(old) && new.as_bytes().get(old.len()) == Some(&b'/') {
        return false;
    }
    let mut entries = ENTRIES.lock();
    let Some(old_i) = find_index(&entries, old) else {
        return false;
    };
    let is_dir = matches!(entries[old_i].kind, Kind::Dir);

    // POSIX: file/symlink rename may replace an existing non-directory dest.
    // Without this, git init dies on the second config write (config.lock →
    // config) after core.repositoryformatversion already created config —
    // commit_lock_file rename failed with ENOENT on all boot arches.
    if let Some(new_i) = find_index(&entries, new) {
        if is_dir || matches!(entries[new_i].kind, Kind::Dir) {
            return false;
        }
        if old_i == new_i {
            return true;
        }
        entries.remove(new_i);
        let old_i = if new_i < old_i { old_i - 1 } else { old_i };
        entries[old_i].path = String::from(new);
        return true;
    }

    if !parent_ok(&entries, new) {
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
    entries.push(Entry {
        path: String::from(linkpath),
        kind: Kind::Symlink(String::from(target)),
    });
    true
}

pub fn readlink(path: &str, buf: &mut [u8]) -> Option<usize> {
    if !valid_rel_path(path) {
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
        });
    }
    if !valid_rel_path(name) {
        return None;
    }
    let entries = ENTRIES.lock();
    let i = find_index(&entries, name)?;
    let (mode, size, nlink) = match &entries[i].kind {
        Kind::Dir => (S_IFDIR | 0o755, 0u32, 2u32),
        Kind::File(data) => (S_IFREG | 0o644, data.len() as u32, 1u32),
        Kind::Symlink(t) => (S_IFLNK | 0o777, t.len() as u32, 1u32),
    };
    Some(StatInfo {
        mode,
        size,
        ino: (i as u32) + 2,
        nlink,
        dev: 0,
    })
}
