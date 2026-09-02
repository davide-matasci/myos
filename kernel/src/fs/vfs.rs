//! Virtual filesystem: path resolution, vnodes, and mount dispatch.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use myos_abi::ModuleVfsOps;

/// Metadata returned by [`stat`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatInfo {
    pub mode: u32,
    pub size: u32,
    pub ino: u32,
    pub nlink: u32,
}

/// Open file identity: mount index + path relative to that mount.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vnode {
    pub mount: u16,
    pub path_len: u16,
    pub path: [u8; Vnode::PATH_CAP],
}

impl Vnode {
    pub const PATH_CAP: usize = 64;

    pub fn path_str(&self) -> &str {
        core::str::from_utf8(&self.path[..self.path_len as usize]).unwrap_or("")
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
    /// Mount accepts write opens / creates.
    pub writable: bool,
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

static MOUNTS: Mutex<Vec<Mount>> = Mutex::new(Vec::new());

/// Attach an in-kernel backend at `prefix` (empty string = root).
///
/// Source is `none` (no block device). `name` is the fstype (`bootfs`, `tmpfs`, ...).
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
/// A second mount with the same prefix replaces the existing one (no umount).
/// That lets userspace retry `vd*` disks at `/fat` until the right volume is bound.
/// `source` is the userspace path (`/dev/vda`) or `none` when there is no block device.
pub fn mount_module(name: &str, prefix: &str, ops: ModuleVfsOps, source: &str) -> bool {
    if prefix.contains('/') {
        return false;
    }
    let source = if source.is_empty() { "none" } else { source };
    let mut mounts = MOUNTS.lock();
    if let Some(m) = mounts.iter_mut().find(|m| m.prefix == prefix) {
        m.name = String::from(name);
        m.source = String::from(source);
        m.backend = MountBackend::Module(ops);
        return true;
    }
    if mounts.iter().any(|m| m.name == name) {
        return false;
    }
    mounts.push(Mount {
        name: String::from(name),
        prefix: String::from(prefix),
        source: String::from(source),
        backend: MountBackend::Module(ops),
    });
    true
}

/// Attach an in-kernel backend at `prefix`, recording `source` (e.g. `/dev/nvme0n1`).
///
/// Same prefix replaces the existing mount (module or kernel). `source` empty becomes `none`.
pub fn mount_kernel(name: &str, prefix: &str, ops: MountOps, source: &str) -> bool {
    if prefix.contains('/') {
        return false;
    }
    let source = if source.is_empty() { "none" } else { source };
    let mut mounts = MOUNTS.lock();
    if let Some(m) = mounts.iter_mut().find(|m| m.prefix == prefix) {
        m.name = String::from(name);
        m.source = String::from(source);
        m.backend = MountBackend::Kernel(ops);
        return true;
    }
    if mounts.iter().any(|m| m.name == name) {
        return false;
    }
    mounts.push(Mount {
        name: String::from(name),
        prefix: String::from(prefix),
        source: String::from(source),
        backend: MountBackend::Kernel(ops),
    });
    true
}

/// Linux-shaped `/proc/mounts` snapshot (`source target fstype opts 0 0\n`).
///
/// Must not be called while `MOUNTS` is already held (procfs `read`/`stat`
/// re-lock after `backend_read` / `backend_stat` drop it).
pub fn mounts_text() -> Vec<u8> {
    let mounts = MOUNTS.lock();
    let mut out = Vec::new();
    for m in mounts.iter() {
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
            _ => b"ro",
        };
        out.extend_from_slice(opts);
        out.extend_from_slice(b" 0 0\n");
    }
    out
}

const O_ACCMODE: u32 = 3;
const O_RDONLY: u32 = 0;
const O_WRONLY: u32 = 1;
const O_RDWR: u32 = 2;
const O_CREAT: u32 = 0o100;
const O_TRUNC: u32 = 0o1000;
const O_APPEND: u32 = 0o2000;

const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const S_IFLNK: u32 = 0o120000;

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
pub fn open(path: &str, flags: u32) -> Option<Vnode> {
    let rel_check = normalize_path(path);
    if rel_check.is_empty() {
        return None;
    }
    let (idx, rel) = resolve_index(path)?;
    if rel.is_empty() {
        return None;
    }

    let acc = flags & O_ACCMODE;
    if acc > O_RDWR {
        return None;
    }
    let wants_write = matches!(acc, O_WRONLY | O_RDWR);
    let creat = flags & O_CREAT != 0;
    let trunc = flags & O_TRUNC != 0;

    if backend_openable(idx, rel) {
        if wants_write {
            if !backend_has_write(idx) {
                return None;
            }
            if trunc && !backend_truncate(idx, rel) {
                return None;
            }
        }
        return Some(make_vnode(idx, rel));
    }

    if creat {
        if !backend_create(idx, rel) {
            return None;
        }
        if trunc {
            let _ = backend_truncate(idx, rel);
        }
        return Some(make_vnode(idx, rel));
    }
    None
}

fn make_vnode(idx: usize, rel: &str) -> Vnode {
    let mut node = Vnode {
        mount: idx as u16,
        path_len: 0,
        path: [0; Vnode::PATH_CAP],
    };
    let len = rel.len().min(Vnode::PATH_CAP);
    node.path[..len].copy_from_slice(&rel.as_bytes()[..len]);
    node.path_len = len as u16;
    node
}

/// Look up file bytes for `path` on the best matching mount.
pub fn lookup(path: &str) -> Option<&'static [u8]> {
    let rel = normalize_path(path);
    if rel.is_empty() {
        return None;
    }
    let (idx, rel) = resolve_index(path)?;
    backend_lookup(idx, rel)
}

/// Stat `path` on the best matching mount.
pub fn stat(path: &str) -> Option<StatInfo> {
    let (idx, rel) = resolve_index(path)?;
    backend_stat(idx, rel)
}

/// Read from an open vnode at `pos` into `out`. Returns bytes read.
pub fn read(node: &Vnode, pos: usize, out: &mut [u8]) -> usize {
    backend_read(node.mount as usize, node.path_str(), pos, out)
}

/// Write to an open vnode at `pos`. Returns bytes written, or `None` on error.
pub fn write(node: &Vnode, pos: usize, buf: &[u8]) -> Option<usize> {
    backend_write(node.mount as usize, node.path_str(), pos, buf)
}

/// Current size of the vnode path (for `O_APPEND`), if known.
pub fn size_of(node: &Vnode) -> Option<usize> {
    backend_stat(node.mount as usize, node.path_str()).map(|s| s.size as usize)
}

/// Create directory at `path` (must resolve to a writable mount).
pub fn mkdir(path: &str) -> bool {
    let Some((idx, rel)) = resolve_index(path) else {
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
    let Some((idx, rel)) = resolve_index(path) else {
        return false;
    };
    if rel.is_empty() {
        return false;
    }
    if !backend_has_write(idx) {
        return false;
    }
    backend_rmdir(idx, rel)
}

/// Unlink file or symlink at `path`.
pub fn unlink(path: &str) -> bool {
    let Some((idx, rel)) = resolve_index(path) else {
        return false;
    };
    if rel.is_empty() {
        return false;
    }
    if !backend_has_write(idx) {
        return false;
    }
    backend_unlink(idx, rel)
}

/// Rename within a single mount (`old` and `new` must resolve to the same mount).
pub fn rename(old: &str, new: &str) -> bool {
    let Some((idx_o, rel_o)) = resolve_index(old) else {
        return false;
    };
    let Some((idx_n, rel_n)) = resolve_index(new) else {
        return false;
    };
    if idx_o != idx_n || rel_o.is_empty() || rel_n.is_empty() {
        return false;
    }
    if !backend_has_write(idx_o) {
        return false;
    }
    backend_rename(idx_o, rel_o, rel_n)
}

/// Create symlink at `linkpath` with contents `target`.
pub fn symlink(target: &str, linkpath: &str) -> bool {
    let Some((idx, rel)) = resolve_index(linkpath) else {
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
    let (idx, rel) = resolve_index(path)?;
    if rel.is_empty() {
        return None;
    }
    backend_readlink(idx, rel, buf)
}

/// List directory entries at `path` into `buf` (newline-separated basenames).
///
/// When listing the root mount (`/` / `.`), also append other mount prefixes
/// (e.g. `s`, `c`) so tools like `/s/ls` show bootfs files and mount points.
pub fn listdir(path: &str, buf: &mut [u8]) -> usize {
    let Some((idx, rel)) = resolve_index(path) else {
        return 0;
    };
    let mounts = MOUNTS.lock();
    let Some(m) = mounts.get(idx) else {
        return 0;
    };
    let mut n = backend_listdir(m, rel, buf);
    if m.prefix.is_empty() && (rel.is_empty() || rel == ".") {
        for other in mounts.iter() {
            let prefix = other.prefix.as_str();
            if prefix.is_empty() || prefix.contains('/') {
                continue;
            }
            let name = prefix.as_bytes();
            let need = name.len() + 1;
            if n + need > buf.len() {
                break;
            }
            buf[n..n + name.len()].copy_from_slice(name);
            n += name.len();
            buf[n] = b'\n';
            n += 1;
        }
    }
    n
}

/// Register `name` on mount `mount_name` (copying `bytes` into bootfs storage).
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

fn resolve_index(path: &str) -> Option<(usize, &str)> {
    let path = normalize_path(path);
    let mounts = MOUNTS.lock();
    let mut best: Option<(usize, usize, &str)> = None;
    for (idx, m) in mounts.iter().enumerate() {
        let prefix = m.prefix.as_str();
        let (score, rel) = if prefix.is_empty() {
            (0, path)
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
    best.map(|(_, idx, rel)| (idx, rel))
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
    match backend {
        MountBackend::Kernel(ops) => (ops.stat)(rel),
        MountBackend::Module(ops) => module_stat(&ops, rel),
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
        MountBackend::Module(_) => false,
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
        MountBackend::Module(_) => false,
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
        MountBackend::Module(_) => false,
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
        MountBackend::Module(ops) => {
            read_from_static(module_lookup(&ops, rel), pos, out)
        }
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
        MountBackend::Module(_) => None,
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
        MountBackend::Module(_) => false,
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
        MountBackend::Module(_) => false,
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
        MountBackend::Module(_) => false,
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
        MountBackend::Module(_) => false,
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
        MountBackend::Module(_) => false,
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
        MountBackend::Module(_) => None,
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

fn normalize_path(path: &str) -> &str {
    path.trim_start_matches('/')
}

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
    let mut raw = [0u8; 128];
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
    let mut scratch = [0u8; 128];
    let mut sn = 0usize;
    // component starts in scratch
    let mut starts = [0usize; 32];
    let mut lens = [0usize; 32];
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
