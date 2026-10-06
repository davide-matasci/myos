//! Virtual filesystem: path resolution, vnodes, and mount dispatch.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use myos_abi::ModuleVfsOps;

use super::node;
use super::pagecache;
pub use super::node::Vnode;

/// Metadata returned by [`stat`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatInfo {
    pub mode: u32,
    pub size: u32,
    pub ino: u32,
    pub nlink: u32,
    /// Filesystem device id for this mount (`st_dev`). Distinct per mount so
    /// `(st_dev, st_ino)` identities do not collide across VFS roots that all
    /// use `ino == 1`. Assigned by [`backend_stat`] as `mount_index + 1`.
    pub dev: u32,
    /// Last modification, in seconds since the epoch: 0 where the
    /// filesystem keeps none (tmpfs and module filesystems such as ext2 do).
    pub mtime: u64,
    /// Last access as set by `utimens` (reads do not change it), in seconds
    /// since the epoch: 0 where the filesystem keeps none.
    pub atime: u64,
}

/// A time for [`set_times`]: a value, now, or left as it is
/// (`UTIME_NOW` / `UTIME_OMIT`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetTime {
    At(u64),
    Now,
    Omit,
}

impl SetTime {
    /// The seconds to store, `None` to keep the current value.
    pub fn resolve(self) -> Option<u64> {
        match self {
            SetTime::At(t) => Some(t),
            SetTime::Now => Some(crate::time::unix_seconds().unwrap_or(0).max(0) as u64),
            SetTime::Omit => None,
        }
    }
}

/// Stable inode for a directory path relative to a mount root.
///
/// A read-only tree (rootfs) that returned `ino == 1` for every directory made
/// find(1) treat `/bin/std` as `/bin` and `/lib/newlib` as `/lib`. Hash the
/// relative path and force the high bit so values never collide with the mount
/// root (`ino == 1`) or file inodes from [`data_ino`].
pub(crate) fn dir_ino(path: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5; // FNV-1a offset basis
    for &b in path.as_bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    (h & 0x7fff_ffff) | 0x8000_0000
}

/// Stable inode for a file's backing bytes.
///
/// Cpio multicall aliases reuse the same `'static` slice (same pointer), so
/// they share an inode the way hardlinks should. Distinct files stay distinct.
/// Values stay in the low half so they never collide with [`dir_ino`].
pub(crate) fn data_ino(data: &[u8]) -> u32 {
    let p = data.as_ptr() as u64;
    let mut h: u32 = 0x811c_9dc5;
    for &b in &p.to_le_bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    for &b in &(data.len() as u32).to_le_bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    match h & 0x7fff_ffff {
        0 | 1 => 2,
        x => x,
    }
}

/// Operations provided by an in-kernel filesystem backend.
#[derive(Clone, Copy)]
pub struct MountOps {
    pub lookup: fn(&str) -> Option<&'static [u8]>,
    pub stat: fn(&str) -> Option<StatInfo>,
    pub listdir: fn(&str, &mut [u8]) -> usize,
    pub register: fn(&str, &'static [u8]) -> bool,
    /// Create an empty file (no-op success if it already exists).
    pub create: fn(&str) -> bool,
    /// Truncate an existing file to zero length.
    pub truncate: fn(&str) -> bool,
    /// Read file/device bytes at `pos` into `out`.
    pub read: fn(&str, usize, &mut [u8]) -> usize,
    /// Write bytes at `pos`. `None` if the mount rejects the write.
    pub write: fn(&str, usize, &[u8]) -> Option<usize>,
    /// Create a directory.
    pub mkdir: fn(&str) -> bool,
    /// Remove an empty directory.
    pub rmdir: fn(&str) -> bool,
    /// Unlink a file or symlink.
    pub unlink: fn(&str) -> bool,
    /// Rename within the same mount.
    pub rename: fn(&str, &str) -> bool,
    /// Create a symlink at `linkpath` pointing at `target`.
    pub symlink: fn(&str, &str) -> bool,
    /// Read symlink target into `buf`; returns bytes written.
    pub readlink: fn(&str, &mut [u8]) -> Option<usize>,
    /// Optional `poll` readiness of a path relative to this mount: the bits
    /// that hold now, or `None` for a file that is always ready.
    pub poll: Option<fn(&str) -> Option<u32>>,
    /// Optional: set a path's access and modification times (seconds since
    /// the epoch; `None` keeps one). `None` for a mount that keeps no times.
    pub set_times: Option<fn(&str, Option<u64>, Option<u64>) -> bool>,
    /// Optional: make a file that many bytes long, cut or grown with zeros
    /// (`ftruncate`). `None` for a mount whose files cannot be resized.
    pub set_size: Option<fn(&str, usize) -> bool>,
    /// Optional: the mount's files by id (see [`FileOps`]).
    pub files: Option<FileOps>,
    /// Mount accepts write opens / creates.
    pub writable: bool,
}

/// A filesystem's files by the id it gives each (an inode number): the
/// path is looked up once, at open, and an open file is used by its id
/// from then on, whatever is renamed meanwhile ([`node`]). The module form
/// is `ModuleVfsOps::file_id` and the `*_ino` hooks.
#[derive(Clone, Copy)]
pub struct FileOps {
    /// The id of the file (or directory) at a path.
    pub id: fn(&str) -> Option<u64>,
    pub read: fn(u64, usize, &mut [u8]) -> usize,
    pub write: fn(u64, usize, &[u8]) -> Option<usize>,
    pub stat: fn(u64) -> Option<StatInfo>,
    /// Make the file that many bytes long, cut or grown with zeros.
    pub set_size: fn(u64, usize) -> bool,
    /// Access and modification times, seconds since the epoch (`None`
    /// keeps one).
    pub set_times: fn(u64, Option<u64>, Option<u64>) -> bool,
    /// Remove the name of a regular file but keep the file, which something
    /// holds: false if it cannot.
    pub unlink_keep: fn(&str) -> bool,
    /// Nothing holds the file `unlink_keep` kept any more: free it.
    pub forget: fn(u64),
}

/// Helper for RO backends: copy from a `lookup` result.
pub fn read_from_static(data: Option<&'static [u8]>, pos: usize, out: &mut [u8]) -> usize {
    let Some(data) = data else {
        return 0;
    };
    let n = out.len().min(data.len().saturating_sub(pos));
    if n != 0 {
        out[..n].copy_from_slice(&data[pos..pos + n]);
    }
    n
}

enum MountBackend {
    Kernel(MountOps),
    Module(ModuleVfsOps),
}

impl Copy for MountBackend {}
impl Clone for MountBackend {
    fn clone(&self) -> Self {
        *self
    }
}

struct Mount {
    name: String,
    prefix: String,
    source: String,
    backend: MountBackend,
}

/// The prefix of an unmounted entry: no path matches it. The entry stays in
/// the table (open vnodes name their mount by index) until a mount reuses it.
const GONE: &str = "\0";

impl Mount {
    fn gone(&self) -> bool {
        self.prefix == GONE
    }
}

static MOUNTS: Mutex<Vec<Mount>> = Mutex::new(Vec::new());

/// Attach an in-kernel backend at `prefix` (empty string = root).
///
/// Source is `none` (no block device). `name` is the fstype (`rootfs`, `tmpfs`, ...).
pub fn mount(name: &str, prefix: &str, ops: MountOps) {
    MOUNTS.lock().push(Mount {
        name: String::from(name),
        prefix: String::from(prefix),
        source: String::from("none"),
        backend: MountBackend::Kernel(ops),
    });
}

/// Attach a module backend at `prefix`. `ops` must live for the kernel lifetime.
///
/// A second module mount with the same prefix replaces the existing one (a
/// module loaded again). `mount(2)` refuses a mount point instead
/// ([`mount_point_free`]). `source` is the userspace path (`/dev/vda`) or
/// `none` when there is no block device.
pub fn mount_module(name: &str, prefix: &str, ops: ModuleVfsOps, source: &str) -> bool {
    attach_module(name, prefix, ops, source, true)
}

/// Attach one more filesystem of type `name` (a block device bound through
/// its fstype, `mount(2)`): unlike [`mount_module`], several mounts may
/// share the name (two ext2 disks).
pub fn mount_instance(name: &str, prefix: &str, ops: ModuleVfsOps, source: &str) -> bool {
    attach_module(name, prefix, ops, source, false)
}

fn attach_module(name: &str, prefix: &str, ops: ModuleVfsOps, source: &str, unique: bool) -> bool {
    // A nested prefix (`dev/fb`) wins over its parent mount for the paths
    // below it (`resolve_index` takes the longest match).
    if prefix.starts_with('/') || prefix.ends_with('/') || prefix.contains("//") {
        return false;
    }
    if ops.readlink.is_some() {
        MODULE_SYMLINKS.store(true, core::sync::atomic::Ordering::Relaxed);
    }
    let source = if source.is_empty() { "none" } else { source };
    let mut mounts = MOUNTS.lock();
    if let Some(m) = mounts.iter_mut().find(|m| m.prefix == prefix) {
        m.name = String::from(name);
        m.source = String::from(source);
        m.backend = MountBackend::Module(ops);
        return true;
    }
    if unique && mounts.iter().any(|m| m.name == name) {
        return false;
    }
    let mount = Mount {
        name: String::from(name),
        prefix: String::from(prefix),
        source: String::from(source),
        backend: MountBackend::Module(ops),
    };
    match mounts.iter_mut().find(|m| m.gone()) {
        Some(slot) => *slot = mount,
        None => mounts.push(mount),
    }
    true
}

/// `prefix` (an absolute path without its leading `/`) can take a mount: an
/// existing directory that is not a mount point already. The root, `/`, is
/// one.
pub fn mount_point_free(prefix: &str) -> bool {
    if prefix.is_empty() || MOUNTS.lock().iter().any(|m| m.prefix == prefix) {
        return false;
    }
    stat(prefix).is_some_and(|st| st.mode & super::S_IFMT == 0o040000)
}

/// A mount point lies at `prefix` or below it: such a directory can be
/// neither removed nor renamed, its mount would hang from nothing.
pub fn holds_mount(prefix: &str) -> bool {
    let prefix = normalize_path(prefix);
    MOUNTS.lock().iter().any(|m| {
        !m.gone()
            && !m.prefix.is_empty()
            && (prefix.is_empty()
                || m.prefix == prefix
                || (m.prefix.starts_with(prefix) && m.prefix.as_bytes().get(prefix.len()) == Some(&b'/')))
    })
}

/// `umount(2)`: detach the block-device mount at `prefix`. Refused for the
/// kernel's own trees, and while the mount is busy: a mount below it, a bind
/// into or out of it, an open file on it. The filesystem's `unmount` hook
/// then writes back what it caches.
pub fn unmount(prefix: &str) -> bool {
    let prefix = normalize_path(prefix);
    // The page cache's hold on its files would keep it busy.
    let idx = MOUNTS.lock().iter().position(|m| m.prefix == prefix);
    if let Some(idx) = idx {
        pagecache::forget_mount(idx);
    }
    reap();
    let ops = {
        let mut mounts = MOUNTS.lock();
        let Some(idx) = mounts.iter().position(|m| m.prefix == prefix) else {
            return false;
        };
        let MountBackend::Module(ops) = mounts[idx].backend else {
            return false;
        };
        if !mounts[idx].source.starts_with("/dev/") {
            return false;
        }
        let below = alloc::format!("{prefix}/");
        if mounts.iter().any(|m| m.prefix.starts_with(below.as_str())) {
            return false;
        }
        let under = |p: &str| p == prefix || p.starts_with(below.as_str());
        if BINDS.lock().iter().any(|(target, source)| under(target) || under(source)) {
            return false;
        }
        if node::on_mount(idx) {
            return false;
        }
        let m = &mut mounts[idx];
        m.prefix = String::from(GONE);
        m.name.clear();
        m.source.clear();
        ops
    };
    if let Some(unmount) = ops.unmount {
        unsafe { unmount() };
    }
    true
}

/// A mount has `source` (`/dev/sda`) as its block device.
pub fn source_mounted(source: &str) -> bool {
    MOUNTS.lock().iter().any(|m| !m.gone() && m.source == source)
}

/// Open fds on `rel` of the mount at `prefix` (`"dev"`, `"sda"`: the block
/// device `/dev/sda`), counting a fork's and a dup's copies.
pub fn open_refs(prefix: &str, rel: &str) -> u32 {
    let Some(mount) = MOUNTS.lock().iter().position(|m| m.prefix == prefix) else {
        return 0;
    };
    node::opens_at(mount, rel)
}

/// Linux-shaped `/proc/mounts` snapshot (`source target fstype opts 0 0\n`).
///
/// Must not be called while `MOUNTS` is already held (procfs `read`/`stat`
/// re-lock after `backend_read` / `backend_stat` drop it).
pub fn mounts_text() -> Vec<u8> {
    let mounts = MOUNTS.lock();
    let mut out = Vec::new();
    for m in mounts.iter().filter(|m| !m.gone()) {
        let source = if m.source.is_empty() {
            "none"
        } else {
            m.source.as_str()
        };
        out.extend_from_slice(source.as_bytes());
        out.push(b' ');
        out.push(b'/');
        out.extend_from_slice(m.prefix.as_bytes());
        out.push(b' ');
        out.extend_from_slice(m.name.as_bytes());
        out.push(b' ');
        let opts: &[u8] = match m.backend {
            MountBackend::Kernel(ops) if ops.writable => b"rw",
            MountBackend::Module(ops) if ops.write.is_some() || ops.create.is_some() => b"rw",
            _ => b"ro",
        };
        out.extend_from_slice(opts);
        out.extend_from_slice(b" 0 0\n");
    }
    drop(mounts);
    for (target, source) in BINDS.lock().iter() {
        out.extend_from_slice(alloc::format!("/{source} /{target} bind rw 0 0\n").as_bytes());
    }
    out
}

const O_ACCMODE: u32 = 3;
const O_WRONLY: u32 = 1;
const O_RDWR: u32 = 2;
const O_CREAT: u32 = 0o100;
const O_TRUNC: u32 = 0o1000;
const O_APPEND: u32 = 0o2000;

const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const S_IFLNK: u32 = 0o120000;
const S_IFREG: u32 = 0o100000;

/// True if `flags` request write access.
pub fn open_writable(flags: u32) -> bool {
    matches!(flags & O_ACCMODE, O_WRONLY | O_RDWR)
}

/// True if `flags` include `O_APPEND`.
pub fn open_append(flags: u32) -> bool {
    flags & O_APPEND != 0
}

fn is_dir_mode(mode: u32) -> bool {
    (mode & S_IFMT) == S_IFDIR
}

fn is_lnk_mode(mode: u32) -> bool {
    (mode & S_IFMT) == S_IFLNK
}

fn backend_openable(idx: usize, rel: &str) -> bool {
    if backend_lookup(idx, rel).is_some() {
        return true;
    }
    match backend_stat(idx, rel) {
        Some(st) if !is_dir_mode(st.mode) && !is_lnk_mode(st.mode) => true,
        _ => false,
    }
}

/// Resolve `path` to a vnode suitable for open/read/write.
///
/// Directories (including mount roots like `/bin/sbase`, and `/`) may be
/// opened read-only: a directory fd, the cwd.
pub fn open(path: &str, flags: u32) -> Option<Vnode> {
    reap();
    let _tree = tree_read();
    let (idx, ref rel) = resolve_index(path)?;
    if rel.len() > PATH_MAX {
        return None;
    }

    let acc = flags & O_ACCMODE;
    if acc > O_RDWR {
        return None;
    }
    let wants_write = matches!(acc, O_WRONLY | O_RDWR);
    let creat = flags & O_CREAT != 0;
    let trunc = flags & O_TRUNC != 0;

    // Directory open: read-only. Mount roots have empty `rel` but still stat as dirs.
    if let Some(st) = backend_stat(idx, rel) {
        if is_dir_mode(st.mode) {
            if wants_write || creat || trunc {
                return None;
            }
            return Some(node::get(idx, rel, backend_file_id(idx, rel)));
        }
    }

    if backend_openable(idx, rel) {
        if wants_write {
            if !backend_has_write(idx) {
                return None;
            }
            if trunc && !backend_truncate(idx, rel) {
                return None;
            }
        }
        let node = node::get(idx, rel, backend_file_id(idx, rel));
        if wants_write && trunc {
            pagecache::invalidate(&node);
        }
        return Some(node);
    }

    if creat {
        if rel.is_empty() {
            return None;
        }
        if !backend_create(idx, rel) {
            return None;
        }
        if trunc {
            let _ = backend_truncate(idx, rel);
        }
        return Some(node::get(idx, rel, backend_file_id(idx, rel)));
    }
    None
}

/// Look up file bytes for `path` on the best matching mount.
pub fn lookup(path: &str) -> Option<&'static [u8]> {
    let rel = normalize_path(path);
    if rel.is_empty() {
        return None;
    }
    let _tree = tree_read();
    let (idx, ref rel) = resolve_index(path)?;
    backend_lookup(idx, rel)
}

/// Whole-file read for exec of non-static mounts (tmpfs, ext2).
pub fn read_all(path: &str, max: usize) -> Option<alloc::vec::Vec<u8>> {
    if let Some(b) = lookup(path) {
        if b.len() > max {
            return None;
        }
        return Some(b.to_vec());
    }
    let _tree = tree_read();
    let (idx, ref rel) = resolve_index(path)?;
    if rel.is_empty() {
        return None;
    }
    let st = backend_stat(idx, rel)?;
    if is_dir_mode(st.mode) {
        return None;
    }
    let size = st.size as usize;
    if size == 0 || size > max {
        return None;
    }
    let mut buf = alloc::vec![0u8; size];
    let n = backend_read(idx, rel, 0, &mut buf);
    if n == 0 {
        return None;
    }
    buf.truncate(n);
    Some(buf)
}

/// Stat `path` on the best matching mount.
pub fn stat(path: &str) -> Option<StatInfo> {
    let _tree = tree_read();
    let (idx, ref rel) = resolve_index(path)?;
    backend_stat(idx, rel)
}

/// Stat an open vnode.
pub fn stat_node(node: &Vnode) -> Option<StatInfo> {
    let _tree = tree_read();
    match node::locate(node)? {
        (idx, node::Loc::File(id)) => file_stat(idx, id),
        (idx, node::Loc::Path(rel)) => backend_stat(idx, rel.as_str()),
    }
}

/// Set the access and modification times of `path` on the best matching
/// mount: false when there is no such file or the mount keeps no times.
pub fn set_times(path: &str, atime: SetTime, mtime: SetTime) -> bool {
    let _tree = tree_read();
    let Some((idx, ref rel)) = resolve_index(path) else {
        return false;
    };
    backend_set_times(idx, rel, atime, mtime)
}

/// [`set_times`] for an open vnode (`futimens`).
pub fn set_times_node(node: &Vnode, atime: SetTime, mtime: SetTime) -> bool {
    let _tree = tree_read();
    match node::locate(node) {
        Some((idx, node::Loc::File(id))) => file_set_times(idx, id, atime, mtime),
        Some((idx, node::Loc::Path(rel))) => backend_set_times(idx, rel.as_str(), atime, mtime),
        None => false,
    }
}

/// Read from an open vnode at `pos` into `out`. Returns bytes read.
pub fn read(node: &Vnode, pos: usize, out: &mut [u8]) -> usize {
    let _tree = tree_read();
    match node::locate(node) {
        Some((idx, node::Loc::File(id))) => file_read(idx, id, pos, out),
        Some((idx, node::Loc::Path(rel))) => backend_read(idx, rel.as_str(), pos, out),
        None => 0,
    }
}

/// Write to an open vnode at `pos`. Returns bytes written, or `None` on error.
pub fn write(node: &Vnode, pos: usize, buf: &[u8]) -> Option<usize> {
    let _tree = tree_read();
    let written = match node::locate(node)? {
        (idx, node::Loc::File(id)) => file_write(idx, id, pos, buf),
        (idx, node::Loc::Path(rel)) => backend_write(idx, rel.as_str(), pos, buf),
    };
    // After the write: a page read before it is then dropped, or not kept.
    pagecache::invalidate(node);
    written
}

/// Make an open vnode `size` bytes long, cut or grown with zeros
/// (`ftruncate`): false when its filesystem cannot.
pub fn set_size(node: &Vnode, size: usize) -> bool {
    let _tree = tree_read();
    let done = match node::locate(node) {
        Some((idx, node::Loc::File(id))) => file_set_size(idx, id, size),
        Some((idx, node::Loc::Path(rel))) => backend_set_size(idx, rel.as_str(), size),
        None => false,
    };
    // As after a write: the pages past the new end are gone or zero now.
    pagecache::invalidate(node);
    done
}

/// One more open file description on `node` (an `open` that became an
/// fd). A fork's and a dup's fds share theirs: see [`close_ref`].
pub fn open_ref(node: &Vnode) {
    node::opened(node);
}

/// An open file description on `node` went away. The last one runs its
/// module's `release` hook (netfs tears its connection down): only then,
/// as the parent of a fork may close an accepted socket the child still
/// writes to (dropbear).
pub fn close_ref(node: &Vnode) {
    if node::closed(node) {
        let backend = {
            let _tree = tree_read();
            node::location(node).and_then(|(idx, rel)| {
                let ops = match MOUNTS.lock().get(idx)?.backend {
                    MountBackend::Module(ops) => ops,
                    MountBackend::Kernel(_) => return None,
                };
                if let Some(release) = ops.release {
                    let rel = rel.as_str();
                    let _ = unsafe { (release)(rel.as_ptr(), rel.len()) };
                }
                Some(())
            })
        };
        // A peer that just went away (a socket's hangup) is news for
        // pollers.
        if backend.is_some() {
            crate::task::wake_any();
        }
    }
    reap();
}

/// The module ops behind `node` and its path there, if a module serves it.
fn module_location(node: &Vnode) -> Option<(ModuleVfsOps, node::Rel)> {
    let (idx, rel) = node::location(node)?;
    match MOUNTS.lock().get(idx)?.backend {
        MountBackend::Module(ops) => Some((ops, rel)),
        MountBackend::Kernel(_) => None,
    }
}

/// An `open(2)` of `node` is about to become an fd: its module's `open` hook
/// may refuse it (a file one program holds at a time).
pub fn open_hook(node: &Vnode) -> bool {
    let _tree = tree_read();
    let Some((ops, rel)) = module_location(node) else {
        return true;
    };
    let Some(open) = ops.open else {
        return true;
    };
    let rel = rel.as_str();
    unsafe { (open)(rel.as_ptr(), rel.len()) >= 0 }
}

/// [`open_hook`] let `node` through but no fd came of it: undo, as the last
/// close would.
pub fn open_hook_undo(node: &Vnode) {
    let _tree = tree_read();
    if let Some((ops, rel)) = module_location(node) {
        if let Some(release) = ops.release {
            let rel = rel.as_str();
            let _ = unsafe { (release)(rel.as_ptr(), rel.len()) };
        }
    }
}

/// Read `node` at `pos` for an fd: `None` when its module has nothing yet
/// and the reader should wait ([`myos_abi::MYOS_READ_WAIT`]).
pub fn read_or_wait(node: &Vnode, pos: usize, out: &mut [u8]) -> Option<usize> {
    {
        let _tree = tree_read();
        let by_path = matches!(node::locate(node), Some((_, node::Loc::Path(_))));
        if let Some((ops, rel)) = module_location(node).filter(|_| by_path) {
            if let Some(read) = ops.read {
                let rel = rel.as_str();
                let rc = unsafe { (read)(rel.as_ptr(), rel.len(), pos, out.as_mut_ptr(), out.len()) };
                if rc == myos_abi::MYOS_READ_WAIT {
                    return None;
                }
                return Some(if rc < 0 { 0 } else { (rc as usize).min(out.len()) });
            }
        }
    }
    Some(read(node, pos, out))
}

/// The absolute path of an open vnode: its mount's prefix and the path
/// inside it (what `/proc/self/fd/N` points at), ` (deleted)` after it for
/// a file that has been unlinked.
pub fn vnode_path(node: &Vnode) -> String {
    let Some((idx, rel, gone)) = node::name(node) else {
        return String::new();
    };
    let mounts = MOUNTS.lock();
    let prefix = mounts.get(idx).map_or("", |m| m.prefix.as_str());
    let mut path = String::from("/");
    path.push_str(prefix);
    if !prefix.is_empty() && !rel.is_empty() {
        path.push('/');
    }
    path.push_str(&rel);
    if gone {
        path.push_str(" (deleted)");
    }
    path
}

/// The absolute path of `node`'s file, `None` once it is unlinked or gone.
pub fn node_path(node: &Vnode) -> Option<String> {
    let (idx, rel, gone) = node::name(node)?;
    if gone {
        return None;
    }
    let mounts = MOUNTS.lock();
    let prefix = mounts.get(idx)?.prefix.as_str();
    Some(match (prefix.is_empty(), rel.is_empty()) {
        (true, _) => alloc::format!("/{rel}"),
        (false, true) => alloc::format!("/{prefix}"),
        (false, false) => alloc::format!("/{prefix}/{rel}"),
    })
}

/// `node`'s file is the one at `path` (absolute, canonical).
pub fn node_at(node: &Vnode, path: &str) -> bool {
    let Some((idx, rel)) = node::location(node) else {
        return false;
    };
    let mounts = MOUNTS.lock();
    let Some(m) = mounts.get(idx) else {
        return false;
    };
    let rest = path.strip_prefix('/').and_then(|p| p.strip_prefix(m.prefix.as_str()));
    match rest {
        Some(rest) if m.prefix.is_empty() => rest == rel.as_str(),
        Some(rest) => rest.strip_prefix('/').unwrap_or(rest) == rel.as_str() && (rest.is_empty() || rest.starts_with('/')),
        None => false,
    }
}

/// The page holding byte `offset` (page aligned) of a device file, from its
/// module's `mmap` hook (`/dev/fb/data`); `None` for anything else.
pub fn device_frame(node: &Vnode, offset: usize) -> Option<u64> {
    let _tree = tree_read();
    let (ops, rel) = module_location(node)?;
    let mmap = ops.mmap?;
    let rel = rel.as_str();
    let phys = unsafe { mmap(rel.as_ptr(), rel.len(), offset) };
    (phys != 0 && phys % crate::user::PAGE as u64 == 0).then_some(phys)
}

/// The `poll` bits (`myos_abi::MYOS_POLL*`) that hold now for `node`, from
/// its backend's `poll` hook; `None` when there is none (a file that is
/// always ready).
pub fn poll(node: &Vnode) -> Option<u32> {
    let _tree = tree_read();
    let (idx, rel) = node::location(node)?;
    let backend = MOUNTS.lock().get(idx)?.backend;
    let rel = rel.as_str();
    match backend {
        MountBackend::Kernel(ops) => (ops.poll?)(rel),
        MountBackend::Module(ops) => {
            let poll = ops.poll?;
            Some(unsafe { poll(rel.as_ptr(), rel.len()) })
        }
    }
}

/// Current size of an open vnode (for `O_APPEND`), if known.
pub fn size_of(node: &Vnode) -> Option<usize> {
    stat_node(node).map(|s| s.size as usize)
}

/// Create directory at `path` (must resolve to a writable mount).
pub fn mkdir(path: &str) -> bool {
    let _tree = tree_read();
    let Some((idx, ref rel)) = resolve_index(path) else {
        return false;
    };
    if rel.is_empty() {
        return false;
    }
    if !backend_has_write(idx) {
        return false;
    }
    backend_mkdir(idx, rel)
}

/// Remove empty directory at `path`.
pub fn rmdir(path: &str) -> bool {
    if holds_mount(path) {
        return false;
    }
    reap();
    let _tree = tree_write();
    let Some((idx, ref rel)) = resolve_index(path) else {
        return false;
    };
    if rel.is_empty() || !backend_has_write(idx) || !backend_rmdir(idx, rel) {
        return false;
    }
    node::kill(idx, rel);
    true
}

/// Unlink file or symlink at `path`. A file something still holds (an
/// open fd, a mapping) stays readable and writable through it.
pub fn unlink(path: &str) -> bool {
    reap();
    let _tree = tree_write();
    let Some((idx, ref rel)) = resolve_index(path) else {
        return false;
    };
    if rel.is_empty() || !backend_has_write(idx) {
        return false;
    }
    // The page cache lets go of it first: it would be kept for the cache.
    pagecache::forget_at(idx, rel);
    if hide_held(idx, rel) {
        return true;
    }
    if !backend_unlink(idx, rel) {
        return false;
    }
    node::kill(idx, rel);
    true
}

/// Rename within a single mount (`old` and `new` must resolve to the same
/// mount). What is open below `old` follows it; a file `new` replaces
/// stays for whoever holds it.
pub fn rename(old: &str, new: &str) -> bool {
    if holds_mount(old) {
        return false;
    }
    reap();
    let _tree = tree_write();
    let Some((idx_o, ref rel_o)) = resolve_index(old) else {
        return false;
    };
    let Some((idx_n, ref rel_n)) = resolve_index(new) else {
        return false;
    };
    if idx_o != idx_n || rel_o.is_empty() || rel_n.is_empty() {
        return false;
    }
    if !backend_has_write(idx_o) {
        return false;
    }
    if rel_o == rel_n {
        return backend_rename(idx_o, rel_o, rel_n);
    }
    // Only a file that is not a directory replaces one (the rename could
    // not go ahead otherwise, and the file would be hidden for nothing).
    let replaces = backend_stat(idx_o, rel_o).is_some_and(|st| !is_dir_mode(st.mode));
    if replaces {
        // The rename goes ahead now (`old` is there and neither name is a
        // directory): the file it replaces is kept for whoever holds it,
        // the page cache aside.
        pagecache::forget_at(idx_n, rel_n);
        hide_held(idx_n, rel_n);
    }
    if !backend_rename(idx_o, rel_o, rel_n) {
        return false;
    }
    node::kill(idx_n, rel_n);
    node::moved(idx_o, rel_o, rel_n);
    true
}

/// The regular file at `rel` of mount `idx` is about to go: when something
/// holds it and its filesystem gives file ids, unlink it but have the
/// filesystem keep it by its id ([`FileOps::unlink_keep`]): true then. The
/// VFS lets it go once the last [`Vnode`] on it is gone ([`reap`]). On
/// any other filesystem its nodes die.
fn hide_held(idx: usize, rel: &str) -> bool {
    if !node::referenced(idx, rel) || !backend_stat(idx, rel).is_some_and(|st| st.mode & S_IFMT == S_IFREG) {
        return false;
    }
    if !backend_unlink_keep(idx, rel) {
        return false;
    }
    node::hide(idx, rel);
    true
}

/// Have the filesystems forget the kept files nothing references any more.
/// Called where no lock is held: on the way into the calls that change the
/// tree, and after a close.
fn reap() {
    for (idx, id) in node::take_reaped() {
        file_forget(idx as usize, id);
    }
}

/// Held to read while a path is resolved and the file it names used, or a
/// node is used, and to write while names go or move (`unlink`, `rmdir`,
/// `rename`): where a node's file is and what its filesystem has there
/// change together, so an fd never reaches a file that took its file's
/// old name. A syscall holds it across resolving its paths and acting on
/// them ([`hold_read`], [`hold_write`]), so a path relative to a directory
/// fd is relative to that directory whatever moves meanwhile. A waiter
/// yields rather than spins: a holder may be waiting for a disk (USB
/// storage blocks) with the waiter's CPU its only one.
static TREE: spin::RwLock<()> = spin::RwLock::new(());

/// The task holding [`TREE`] to write (`usize::MAX`: none): the calls it
/// makes while it does go through without taking it again.
static TREE_WRITER: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(usize::MAX);

/// [`TREE`] held, or nothing for the task that holds it to write already.
/// The guards are held for their drop, never read.
#[allow(dead_code)]
pub enum TreeGuard {
    Read(spin::RwLockReadGuard<'static, ()>),
    Write(spin::RwLockWriteGuard<'static, ()>),
    Nested,
}

impl Drop for TreeGuard {
    fn drop(&mut self) {
        if let TreeGuard::Write(_) = self {
            TREE_WRITER.store(usize::MAX, core::sync::atomic::Ordering::Release);
        }
    }
}

fn writing() -> bool {
    TREE_WRITER.load(core::sync::atomic::Ordering::Acquire) == crate::task::current_id()
}

/// Hold the tree to read: a syscall that resolves a path and uses the file
/// (one that may wait for long, a FIFO's peer, must drop it first). A task
/// must not ask to write while it holds it to read.
pub fn hold_read() -> TreeGuard {
    if writing() {
        return TreeGuard::Nested;
    }
    loop {
        if let Some(guard) = TREE.try_read() {
            return TreeGuard::Read(guard);
        }
        crate::task::yield_now();
    }
}

/// Hold the tree to write: a syscall that removes or moves names.
pub fn hold_write() -> TreeGuard {
    if writing() {
        return TreeGuard::Nested;
    }
    loop {
        if let Some(guard) = TREE.try_write() {
            TREE_WRITER.store(crate::task::current_id(), core::sync::atomic::Ordering::Release);
            return TreeGuard::Write(guard);
        }
        crate::task::yield_now();
    }
}

fn tree_read() -> TreeGuard {
    hold_read()
}

fn tree_write() -> TreeGuard {
    hold_write()
}

/// A module filesystem that can hold symlinks (one with `readlink`, such
/// as ext2) has been mounted.
static MODULE_SYMLINKS: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Whether path resolution has to look for symlinks.
pub fn symlinks_possible() -> bool {
    super::tmpfs::has_symlinks() || MODULE_SYMLINKS.load(core::sync::atomic::Ordering::Relaxed)
}

/// Create symlink at `linkpath` with contents `target`.
pub fn symlink(target: &str, linkpath: &str) -> bool {
    let _tree = tree_read();
    let Some((idx, ref rel)) = resolve_index(linkpath) else {
        return false;
    };
    if rel.is_empty() {
        return false;
    }
    if !backend_has_write(idx) {
        return false;
    }
    backend_symlink(idx, target, rel)
}

/// Read symlink target at `path` into `buf`.
pub fn readlink(path: &str, buf: &mut [u8]) -> Option<usize> {
    let _tree = tree_read();
    let (idx, ref rel) = resolve_index(path)?;
    if rel.is_empty() {
        return None;
    }
    backend_readlink(idx, rel, buf)
}

/// List directory entries at `path` into `buf` (newline-separated basenames).
///
/// A listing also shows the mount points below the listed directory
/// (`/` lists `tmp`, `dev`, `proc` next to rootfs's own directories).
pub fn listdir(path: &str, buf: &mut [u8]) -> usize {
    let _tree = tree_read();
    let Some((idx, ref rel)) = resolve_index(path) else {
        return 0;
    };
    let mounts = MOUNTS.lock();
    let Some(m) = mounts.get(idx) else {
        return 0;
    };
    let n = backend_listdir(m, rel, buf);
    let mut n = without_hidden(buf, n);
    // Surface mount points that live below the listed directory, so the tree
    // is browsable even though mount prefixes are virtual (issue #79): `/` shows
    // top-level mounts (`bin`, …), `/bin` the port categories, `/dev/console`
    // the console module's `kbd`.
    let rel = if rel == "." { "" } else { rel.trim_end_matches('/') };
    let dir = match (m.prefix.is_empty(), rel.is_empty()) {
        (true, _) => String::from(rel),
        (false, true) => m.prefix.clone(),
        (false, false) => alloc::format!("{}/{}", m.prefix, rel),
    };
    for other in mounts.iter() {
        if other.prefix.is_empty() || other.gone() {
            continue;
        }
        // The next path segment of `other` below the listed directory.
        let rest = if dir.is_empty() {
            other.prefix.as_str()
        } else if other.prefix.starts_with(dir.as_str())
            && other.prefix.as_bytes().get(dir.len()) == Some(&b'/')
        {
            &other.prefix[dir.len() + 1..]
        } else {
            continue;
        };
        let child = match rest.split_once('/') {
            Some((head, _)) => head,
            None => rest,
        };
        if child.is_empty() || child.contains('\n') {
            continue;
        }
        // Skip if the backend has already listed this name.
        if buf_contains_entry(&buf[..n], child) {
            continue;
        }
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
    drop(mounts);
    // Binds whose target is in this directory (a package's program in
    // /bin/custom, its test script in /lib/myos-tests/ports): the target
    // need not exist in the backend, so it shows up only here.
    let dir = normalize_path(path).trim_end_matches('/');
    for (target, _) in BINDS.lock().iter() {
        let (parent, child) = match target.rsplit_once('/') {
            Some((p, c)) => (p, c),
            None => ("", target.as_str()),
        };
        if parent != dir || child.is_empty() || buf_contains_entry(&buf[..n], child) {
            continue;
        }
        let name = child.as_bytes();
        if n + name.len() + 1 > buf.len() {
            break;
        }
        buf[n..n + name.len()].copy_from_slice(name);
        n += name.len();
        buf[n] = b'\n';
        n += 1;
    }
    n
}

/// The listing in `buf[..n]` without the files unlinked while held (tmpfs
/// keeps them under a name holding a NUL, `tmpfs::unlink_keep`): its new
/// length.
fn without_hidden(buf: &mut [u8], n: usize) -> usize {
    if !buf[..n].contains(&0) {
        return n;
    }
    let (mut from, mut to) = (0, 0);
    while from < n {
        let end = buf[from..n].iter().position(|&b| b == b'\n').map_or(n, |i| from + i + 1);
        if !buf[from..end].contains(&0) {
            buf.copy_within(from..end, to);
            to += end - from;
        }
        from = end;
    }
    to
}

/// True if `dir_buf` (newline-separated basenames) already contains `child`.
fn buf_contains_entry(dir_buf: &[u8], child: &str) -> bool {
    for line in dir_buf.split(|b| *b == b'\n') {
        if line == child.as_bytes() {
            return true;
        }
    }
    false
}

/// Register `name` on mount `mount_name` (rootfs: a module's `vfs_register`).
pub fn register(mount_name: &str, name: &str, bytes: &'static [u8]) -> bool {
    let Some(idx) = mount_index(mount_name) else {
        return false;
    };
    backend_register(idx, name, bytes)
}

/// Register on `mount_name` without copying (`bytes` must outlive the kernel).
pub fn register_static(mount_name: &str, name: &str, bytes: &'static [u8]) -> bool {
    let Some(idx) = mount_index(mount_name) else {
        return false;
    };
    backend_register(idx, name, bytes)
}

fn mount_index(name: &str) -> Option<usize> {
    MOUNTS.lock().iter().position(|m| m.name == name)
}

/// The mount `path` lives on and the path relative to it (bind mounts
/// applied).
fn resolve_index(path: &str) -> Option<(usize, String)> {
    // A NUL is in no path: it marks the hidden names of unlinked files.
    if path.contains('\0') {
        return None;
    }
    let path = unbind(normalize_path(path))?;
    let mounts = MOUNTS.lock();
    let mut best: Option<(usize, usize, &str)> = None;
    for (idx, m) in mounts.iter().enumerate() {
        let prefix = m.prefix.as_str();
        let (score, rel) = if prefix.is_empty() {
            (0, path.as_str())
        } else if path == prefix {
            (prefix.len(), "")
        } else if path.starts_with(prefix)
            && path.as_bytes().get(prefix.len()) == Some(&b'/')
        {
            (prefix.len(), &path[prefix.len() + 1..])
        } else {
            continue;
        };
        let prev = best.as_ref().map(|(s, _, _)| *s);
        if prev.is_none() || score > prev.unwrap() {
            best = Some((score, idx, rel));
        }
    }
    best.map(|(_, idx, rel)| (idx, String::from(rel)))
}

/// Bind mounts as `(target, source)`: the tree at `source` is also seen at
/// `target` (both absolute paths without the leading `/`). A path is
/// rewritten before it reaches a mount, so a bind has no backend of its own.
static BINDS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Binds rewritten while resolving one path (binds of binds).
const MAX_BIND_HOPS: usize = 8;

/// The path a file is stored at: `path` (absolute) with every bind mount
/// it lies under replaced by its source. What labels are taken from
/// (`crate::sec`), so that a second name gives no second label.
pub fn canonical(path: &str) -> Option<String> {
    let mut out = String::from("/");
    out.push_str(&unbind(normalize_path(path))?);
    Some(out)
}

/// `path` with the longest bind it lies under replaced by its source, until
/// none applies. `None` on a bind loop.
fn unbind(path: &str) -> Option<String> {
    let mut path = String::from(path);
    for _ in 0..MAX_BIND_HOPS {
        let binds = BINDS.lock();
        let Some((target, source)) = binds
            .iter()
            .filter(|(t, _)| path == *t || (path.starts_with(t.as_str()) && path.as_bytes()[t.len()] == b'/'))
            .max_by_key(|(t, _)| t.len())
        else {
            return Some(path);
        };
        let rest = &path[target.len()..];
        let joined = if source.is_empty() { rest.trim_start_matches('/') } else { rest };
        path = alloc::format!("{source}{joined}");
    }
    None
}

/// Bind `source` (a directory or a file) at `target` (absolute paths),
/// replacing a bind already there. `target` need not exist: `get-myos`
/// binds a package's files where the image would have them (`/lib/vim`,
/// `/bin/custom/vim`), over a read-only tree or beside it. Binds last
/// until reboot.
pub fn bind(source: &str, target: &str) -> bool {
    let (source, target) = (normalize_path(source), normalize_path(target));
    if target.is_empty() || stat(source).is_none() {
        return false;
    }
    let mut binds = BINDS.lock();
    binds.retain(|(t, _)| t != target);
    binds.push((String::from(target), String::from(source)));
    true
}

/// True when `path` resolves into the tmpfs mount (the only fs with FIFOs).
fn tmpfs_rel(path: &str) -> Option<String> {
    let _tree = tree_read();
    let (idx, rel) = resolve_index(path)?;
    let mounts = MOUNTS.lock();
    (mounts.get(idx)?.name == "tmpfs").then_some(rel)
}

/// mkfifo(2) on an absolute path. Only tmpfs (`/tmp`) can hold FIFOs.
pub fn mkfifo(path: &str) -> bool {
    match tmpfs_rel(path) {
        Some(rel) => super::tmpfs::mkfifo(&rel),
        None => false,
    }
}

/// Pipe slot behind the named FIFO at `path`, if it is one.
pub fn fifo_id(path: &str) -> Option<usize> {
    super::tmpfs::fifo_id(&tmpfs_rel(path)?)
}

fn backend_lookup(idx: usize, rel: &str) -> Option<&'static [u8]> {
    let backend = {
        let mounts = MOUNTS.lock();
        let m = mounts.get(idx)?;
        m.backend
    };
    match backend {
        MountBackend::Kernel(ops) => (ops.lookup)(rel),
        MountBackend::Module(ops) => module_lookup(&ops, rel),
    }
}

fn backend_stat(idx: usize, rel: &str) -> Option<StatInfo> {
    let backend = {
        let mounts = MOUNTS.lock();
        let m = mounts.get(idx)?;
        m.backend
    };
    let mut info = match backend {
        MountBackend::Kernel(ops) => (ops.stat)(rel)?,
        MountBackend::Module(ops) => module_stat(&ops, rel)?,
    };
    // POSIX: each mount is a distinct device. Roots often share ino==1, so
    // find(1)/du(1)/rm(1) loop detection needs distinct st_dev per mount.
    info.dev = (idx as u32).wrapping_add(1);
    Some(info)
}

fn backend_of(idx: usize) -> Option<MountBackend> {
    MOUNTS.lock().get(idx).map(|m| m.backend)
}

/// The id mount `idx`'s filesystem gives the file at `rel` ([`FileOps`]),
/// if it gives ids.
fn backend_file_id(idx: usize, rel: &str) -> Option<u64> {
    match backend_of(idx)? {
        MountBackend::Kernel(ops) => (ops.files?.id)(rel),
        MountBackend::Module(ops) => {
            let id = unsafe { ops.file_id?(rel.as_ptr(), rel.len()) };
            (id > 0).then_some(id as u64)
        }
    }
}

fn file_read(idx: usize, id: u64, pos: usize, out: &mut [u8]) -> usize {
    match backend_of(idx) {
        Some(MountBackend::Kernel(ops)) => ops.files.map_or(0, |f| (f.read)(id, pos, out)),
        Some(MountBackend::Module(ops)) => {
            let Some(read) = ops.read_ino else {
                return 0;
            };
            let rc = unsafe { read(id, pos, out.as_mut_ptr(), out.len()) };
            if rc < 0 { 0 } else { (rc as usize).min(out.len()) }
        }
        None => 0,
    }
}

fn file_write(idx: usize, id: u64, pos: usize, buf: &[u8]) -> Option<usize> {
    match backend_of(idx)? {
        MountBackend::Kernel(ops) => (ops.files?.write)(id, pos, buf),
        MountBackend::Module(ops) => {
            let rc = unsafe { ops.write_ino?(id, pos, buf.as_ptr(), buf.len()) };
            if rc < 0 { None } else { Some(rc as usize) }
        }
    }
}

fn file_stat(idx: usize, id: u64) -> Option<StatInfo> {
    let info = match backend_of(idx)? {
        MountBackend::Kernel(ops) => (ops.files?.stat)(id)?,
        MountBackend::Module(ops) => {
            let mut out = myos_abi::VfsStatInfo::default();
            if unsafe { ops.stat_ino?(id, &mut out) } != 0 {
                return None;
            }
            StatInfo { mode: out.mode, size: out.size, ino: out.ino, nlink: out.nlink, dev: 0, mtime: out.mtime, atime: out.atime }
        }
    };
    Some(StatInfo { dev: (idx as u32).wrapping_add(1), ..info })
}

fn file_set_size(idx: usize, id: u64, size: usize) -> bool {
    match backend_of(idx) {
        Some(MountBackend::Kernel(ops)) => ops.files.is_some_and(|f| (f.set_size)(id, size)),
        Some(MountBackend::Module(ops)) => ops.set_size_ino.is_some_and(|f| unsafe { f(id, size as u64) == 0 }),
        None => false,
    }
}

fn file_set_times(idx: usize, id: u64, atime: SetTime, mtime: SetTime) -> bool {
    let Some(backend) = backend_of(idx) else {
        return false;
    };
    // "Now" is read here, outside the backend's locks.
    let (atime, mtime) = (atime.resolve(), mtime.resolve());
    match backend {
        MountBackend::Kernel(ops) => ops.files.is_some_and(|f| (f.set_times)(id, atime, mtime)),
        MountBackend::Module(ops) => ops.set_times_ino.is_some_and(|f| {
            let omit = myos_abi::MYOS_TIME_OMIT;
            unsafe { f(id, atime.unwrap_or(omit), mtime.unwrap_or(omit)) == 0 }
        }),
    }
}

/// Unlink the regular file at `rel` but keep it by its id for whoever holds
/// it: false when the filesystem cannot.
fn backend_unlink_keep(idx: usize, rel: &str) -> bool {
    match backend_of(idx) {
        Some(MountBackend::Kernel(ops)) => ops.files.is_some_and(|f| (f.unlink_keep)(rel)),
        Some(MountBackend::Module(ops)) => {
            ops.unlink_keep.is_some_and(|f| unsafe { f(rel.as_ptr(), rel.len()) } > 0)
        }
        None => false,
    }
}

/// The kept file `id` of mount `idx` is let go.
fn file_forget(idx: usize, id: u64) {
    match backend_of(idx) {
        Some(MountBackend::Kernel(ops)) => {
            if let Some(f) = ops.files {
                (f.forget)(id);
            }
        }
        Some(MountBackend::Module(ops)) => {
            if let Some(forget) = ops.forget_ino {
                let _ = unsafe { forget(id) };
            }
        }
        None => {}
    }
}

fn backend_listdir(m: &Mount, rel: &str, buf: &mut [u8]) -> usize {
    match m.backend {
        MountBackend::Kernel(ops) => (ops.listdir)(rel, buf),
        MountBackend::Module(ops) => module_listdir(&ops, rel, buf),
    }
}

fn backend_register(idx: usize, name: &str, bytes: &'static [u8]) -> bool {
    let backend = {
        let mounts = MOUNTS.lock();
        let Some(m) = mounts.get(idx) else {
            return false;
        };
        m.backend
    };
    match backend {
        MountBackend::Kernel(ops) => (ops.register)(name, bytes),
        MountBackend::Module(ops) => module_register(&ops, name, bytes),
    }
}

fn backend_has_write(idx: usize) -> bool {
    let backend = {
        let mounts = MOUNTS.lock();
        let Some(m) = mounts.get(idx) else {
            return false;
        };
        m.backend
    };
    match backend {
        MountBackend::Kernel(ops) => ops.writable,
        MountBackend::Module(ops) => ops.write.is_some() || ops.create.is_some(),
    }
}

fn backend_create(idx: usize, rel: &str) -> bool {
    let backend = {
        let mounts = MOUNTS.lock();
        let Some(m) = mounts.get(idx) else {
            return false;
        };
        m.backend
    };
    match backend {
        MountBackend::Kernel(ops) => (ops.create)(rel),
        MountBackend::Module(ops) => module_path_i32(ops.create, rel),
    }
}

fn backend_truncate(idx: usize, rel: &str) -> bool {
    let backend = {
        let mounts = MOUNTS.lock();
        let Some(m) = mounts.get(idx) else {
            return false;
        };
        m.backend
    };
    match backend {
        MountBackend::Kernel(ops) => (ops.truncate)(rel),
        MountBackend::Module(ops) => module_path_i32(ops.truncate, rel),
    }
}

fn backend_set_size(idx: usize, rel: &str, size: usize) -> bool {
    let backend = {
        let mounts = MOUNTS.lock();
        let Some(m) = mounts.get(idx) else {
            return false;
        };
        m.backend
    };
    match backend {
        MountBackend::Kernel(ops) => ops.set_size.is_some_and(|set_size| set_size(rel, size)),
        MountBackend::Module(ops) => match ops.set_size {
            Some(set_size) => unsafe { set_size(rel.as_ptr(), rel.len(), size as u64) == 0 },
            None => false,
        },
    }
}

fn backend_set_times(idx: usize, rel: &str, atime: SetTime, mtime: SetTime) -> bool {
    let backend = {
        let mounts = MOUNTS.lock();
        let Some(m) = mounts.get(idx) else {
            return false;
        };
        m.backend
    };
    // "Now" is read here, outside the backend's locks.
    let (atime, mtime) = (atime.resolve(), mtime.resolve());
    match backend {
        MountBackend::Kernel(ops) => ops.set_times.is_some_and(|f| f(rel, atime, mtime)),
        MountBackend::Module(ops) => ops.set_times.is_some_and(|f| {
            let omit = myos_abi::MYOS_TIME_OMIT;
            unsafe { f(rel.as_ptr(), rel.len(), atime.unwrap_or(omit), mtime.unwrap_or(omit)) == 0 }
        }),
    }
}

fn backend_read(idx: usize, rel: &str, pos: usize, out: &mut [u8]) -> usize {
    let backend = {
        let mounts = MOUNTS.lock();
        let Some(m) = mounts.get(idx) else {
            return 0;
        };
        m.backend
    };
    match backend {
        MountBackend::Kernel(ops) => (ops.read)(rel, pos, out),
        MountBackend::Module(ops) => module_read(&ops, rel, pos, out),
    }
}

fn backend_write(idx: usize, rel: &str, pos: usize, buf: &[u8]) -> Option<usize> {
    let backend = {
        let mounts = MOUNTS.lock();
        let m = mounts.get(idx)?;
        m.backend
    };
    match backend {
        MountBackend::Kernel(ops) => (ops.write)(rel, pos, buf),
        MountBackend::Module(ops) => module_write(&ops, rel, pos, buf),
    }
}

fn backend_mkdir(idx: usize, rel: &str) -> bool {
    let backend = {
        let mounts = MOUNTS.lock();
        let Some(m) = mounts.get(idx) else {
            return false;
        };
        m.backend
    };
    match backend {
        MountBackend::Kernel(ops) => (ops.mkdir)(rel),
        MountBackend::Module(ops) => module_path_i32(ops.mkdir, rel),
    }
}

fn backend_rmdir(idx: usize, rel: &str) -> bool {
    let backend = {
        let mounts = MOUNTS.lock();
        let Some(m) = mounts.get(idx) else {
            return false;
        };
        m.backend
    };
    match backend {
        MountBackend::Kernel(ops) => (ops.rmdir)(rel),
        MountBackend::Module(ops) => module_path_i32(ops.rmdir, rel),
    }
}

fn backend_unlink(idx: usize, rel: &str) -> bool {
    let backend = {
        let mounts = MOUNTS.lock();
        let Some(m) = mounts.get(idx) else {
            return false;
        };
        m.backend
    };
    match backend {
        MountBackend::Kernel(ops) => (ops.unlink)(rel),
        MountBackend::Module(ops) => module_path_i32(ops.unlink, rel),
    }
}

fn backend_rename(idx: usize, old: &str, new: &str) -> bool {
    let backend = {
        let mounts = MOUNTS.lock();
        let Some(m) = mounts.get(idx) else {
            return false;
        };
        m.backend
    };
    match backend {
        MountBackend::Kernel(ops) => (ops.rename)(old, new),
        MountBackend::Module(ops) => module_rename(&ops, old, new),
    }
}

fn backend_symlink(idx: usize, target: &str, linkpath: &str) -> bool {
    let backend = {
        let mounts = MOUNTS.lock();
        let Some(m) = mounts.get(idx) else {
            return false;
        };
        m.backend
    };
    match backend {
        MountBackend::Kernel(ops) => (ops.symlink)(target, linkpath),
        MountBackend::Module(ops) => module_symlink(&ops, target, linkpath),
    }
}

fn backend_readlink(idx: usize, rel: &str, buf: &mut [u8]) -> Option<usize> {
    let backend = {
        let mounts = MOUNTS.lock();
        let m = mounts.get(idx)?;
        m.backend
    };
    match backend {
        MountBackend::Kernel(ops) => (ops.readlink)(rel, buf),
        MountBackend::Module(ops) => module_readlink(&ops, rel, buf),
    }
}

fn module_lookup(ops: &ModuleVfsOps, rel: &str) -> Option<&'static [u8]> {
    if ops.lookup as usize == 0 {
        return None;
    }
    let mut ptr: *const u8 = core::ptr::null();
    let mut len: usize = 0;
    let rc = unsafe {
        (ops.lookup)(
            rel.as_ptr(),
            rel.len(),
            &mut ptr as *mut *const u8,
            &mut len,
        )
    };
    if rc != 0 || ptr.is_null() {
        return None;
    }
    Some(unsafe { core::slice::from_raw_parts(ptr, len) })
}

fn module_stat(ops: &ModuleVfsOps, rel: &str) -> Option<StatInfo> {
    if ops.stat as usize == 0 {
        return None;
    }
    let mut out = myos_abi::VfsStatInfo::default();
    let rc = unsafe {
        (ops.stat)(
            rel.as_ptr(),
            rel.len(),
            &mut out as *mut myos_abi::VfsStatInfo,
        )
    };
    if rc != 0 {
        return None;
    }
    Some(StatInfo {
        mode: out.mode,
        size: out.size,
        ino: out.ino,
        nlink: out.nlink,
        dev: 0,
        mtime: out.mtime,
        atime: out.atime,
    })
}

fn module_listdir(ops: &ModuleVfsOps, rel: &str, buf: &mut [u8]) -> usize {
    if ops.listdir as usize == 0 {
        return 0;
    }
    let mut n: usize = 0;
    let rc = unsafe {
        (ops.listdir)(
            rel.as_ptr(),
            rel.len(),
            buf.as_mut_ptr(),
            buf.len(),
            &mut n,
        )
    };
    if rc != 0 {
        0
    } else {
        n.min(buf.len())
    }
}

fn module_register(ops: &ModuleVfsOps, name: &str, bytes: &'static [u8]) -> bool {
    let Some(register) = ops.register else {
        return false;
    };
    unsafe { (register)(name.as_ptr(), name.len(), bytes.as_ptr(), bytes.len()) == 0 }
}

fn module_path_i32(f: Option<unsafe extern "C" fn(*const u8, usize) -> i32>, rel: &str) -> bool {
    let Some(f) = f else {
        return false;
    };
    unsafe { (f)(rel.as_ptr(), rel.len()) == 0 }
}

fn module_read(ops: &ModuleVfsOps, rel: &str, pos: usize, out: &mut [u8]) -> usize {
    if let Some(read) = ops.read {
        let rc = unsafe { (read)(rel.as_ptr(), rel.len(), pos, out.as_mut_ptr(), out.len()) };
        if rc < 0 {
            0
        } else {
            (rc as usize).min(out.len())
        }
    } else {
        read_from_static(module_lookup(ops, rel), pos, out)
    }
}

fn module_write(ops: &ModuleVfsOps, rel: &str, pos: usize, buf: &[u8]) -> Option<usize> {
    let write = ops.write?;
    let rc = unsafe { (write)(rel.as_ptr(), rel.len(), pos, buf.as_ptr(), buf.len()) };
    if rc < 0 { None } else { Some(rc as usize) }
}

fn module_rename(ops: &ModuleVfsOps, old: &str, new: &str) -> bool {
    let Some(rename) = ops.rename else {
        return false;
    };
    unsafe { (rename)(old.as_ptr(), old.len(), new.as_ptr(), new.len()) == 0 }
}

fn module_symlink(ops: &ModuleVfsOps, target: &str, linkpath: &str) -> bool {
    let Some(symlink) = ops.symlink else {
        return false;
    };
    unsafe {
        (symlink)(
            target.as_ptr(),
            target.len(),
            linkpath.as_ptr(),
            linkpath.len(),
        ) == 0
    }
}

fn module_readlink(ops: &ModuleVfsOps, rel: &str, buf: &mut [u8]) -> Option<usize> {
    let readlink = ops.readlink?;
    let rc = unsafe { (readlink)(rel.as_ptr(), rel.len(), buf.as_mut_ptr(), buf.len()) };
    if rc < 0 {
        None
    } else {
        Some((rc as usize).min(buf.len()))
    }
}

fn normalize_path(path: &str) -> &str {
    path.trim_start_matches('/')
}

/// Longest absolute path the VFS resolves (as the syscalls' `MAX_PATH`).
pub const PATH_MAX: usize = 256;

/// Join `cwd` (absolute) with `path` (absolute or relative) into `out`.
///
/// Returns the length of the canonical absolute path written to `out`, or
/// `None` if the result would not fit / is invalid. Handles `.` / `..` and
/// redundant slashes. Result is always absolute (`/` or `/…`).
pub fn resolve_against_cwd(cwd: &str, path: &str, out: &mut [u8]) -> Option<usize> {
    if out.is_empty() {
        return None;
    }
    let cwd = if cwd.is_empty() { "/" } else { cwd };
    if !cwd.starts_with('/') {
        return None;
    }

    // Build an absolute candidate before canonicalizing.
    let mut raw = [0u8; PATH_MAX];
    let mut n = 0usize;
    let push = |raw: &mut [u8], n: &mut usize, b: u8| -> bool {
        if *n >= raw.len() {
            return false;
        }
        raw[*n] = b;
        *n += 1;
        true
    };
    let push_str = |raw: &mut [u8], n: &mut usize, s: &str| -> bool {
        for &b in s.as_bytes() {
            if !push(raw, n, b) {
                return false;
            }
        }
        true
    };

    if path.is_empty() || path == "." {
        if !push_str(&mut raw, &mut n, cwd) {
            return None;
        }
    } else if path.starts_with('/') {
        if !push_str(&mut raw, &mut n, path) {
            return None;
        }
    } else if cwd == "/" {
        if !push(&mut raw, &mut n, b'/') {
            return None;
        }
        if !push_str(&mut raw, &mut n, path) {
            return None;
        }
    } else {
        if !push_str(&mut raw, &mut n, cwd) {
            return None;
        }
        if !push(&mut raw, &mut n, b'/') {
            return None;
        }
        if !push_str(&mut raw, &mut n, path) {
            return None;
        }
    }

    let Ok(raw_s) = core::str::from_utf8(&raw[..n]) else {
        return None;
    };
    canonicalize_absolute(raw_s, out)
}

fn canonicalize_absolute(path: &str, out: &mut [u8]) -> Option<usize> {
    // Stack of component byte ranges into a scratch buffer.
    let mut scratch = [0u8; PATH_MAX];
    let mut sn = 0usize;
    // component starts in scratch
    let mut starts = [0usize; 64];
    let mut lens = [0usize; 64];
    let mut depth = 0usize;

    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." {
            if depth > 0 {
                depth -= 1;
                sn = starts[depth];
            }
            continue;
        }
        if depth >= starts.len() {
            return None;
        }
        if sn + comp.len() > scratch.len() {
            return None;
        }
        starts[depth] = sn;
        lens[depth] = comp.len();
        scratch[sn..sn + comp.len()].copy_from_slice(comp.as_bytes());
        sn += comp.len();
        depth += 1;
    }

    if depth == 0 {
        if out.is_empty() {
            return None;
        }
        out[0] = b'/';
        return Some(1);
    }

    // '/' + components joined by '/'
    let mut need = 0usize;
    for i in 0..depth {
        need += 1 + lens[i];
    }
    if need > out.len() {
        return None;
    }
    let mut o = 0usize;
    for i in 0..depth {
        out[o] = b'/';
        o += 1;
        let s = starts[i];
        let l = lens[i];
        out[o..o + l].copy_from_slice(&scratch[s..s + l]);
        o += l;
    }
    Some(o)
}
