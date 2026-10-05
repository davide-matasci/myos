//! The path calls. Each names its file by a directory fd and a path
//! relative to it (`*at`): [`AT_FDCWD`] is the cwd, an absolute path leaves
//! the fd out, and [`AT_EMPTY_PATH`] with an empty path is the fd's own file
//! (`fstat`, `futimens`, `fchdir`, `fdopendir`, `fexecve`). A path is
//! `(ptr, len)`. Each call holds the tree (`vfs::hold_read`,
//! `vfs::hold_write`) from resolving its paths to acting on them, so a
//! directory fd stands for its directory whatever is renamed meanwhile.

use alloc::string::String;
use alloc::vec::Vec;

use super::*;
use crate::sec::Rights;

/// `openat(dirfd, path, len, flags)`: an fd on the file (`O_CREAT`
/// creates it).
pub(super) const SYS_OPENAT: usize = 70;
/// `statat(dirfd, path, len, flags, out)`: [`MyosStat`] of the file
/// ([`AT_SYMLINK_NOFOLLOW`]: of a symlink itself).
pub(super) const SYS_STATAT: usize = 71;
/// `mknodat(dirfd, path, len, kind)`: a new directory ([`MKNOD_DIR`]) or
/// named pipe ([`MKNOD_FIFO`], tmpfs only).
pub(super) const SYS_MKNODAT: usize = 72;
/// `symlinkat(target, target_len, dirfd, path, len)`: a symlink holding
/// `target` (any text).
pub(super) const SYS_SYMLINKAT: usize = 73;
/// `unlinkat(dirfd, path, len, flags)`: remove a file, or with
/// [`AT_REMOVEDIR`] an empty directory.
pub(super) const SYS_UNLINKAT: usize = 74;
/// `renameat(old_dirfd, old, old_len, new_dirfd, new, new_len)`.
pub(super) const SYS_RENAMEAT: usize = 75;
/// `readlinkat(dirfd, path, len, buf, size)`: a symlink's target; its
/// length.
pub(super) const SYS_READLINKAT: usize = 76;
/// `utimensat(dirfd, path, len, times, flags)`: set the access and
/// modification times from `times`, two `i64`s: seconds since the epoch,
/// [`UTIME_NOW`] or [`UTIME_OMIT`]; a null `times` sets both to now.
pub(super) const SYS_UTIMENSAT: usize = 77;
/// `chdirat(dirfd, path, len, flags)`: make the directory the cwd.
pub(super) const SYS_CHDIRAT: usize = 78;
/// `listdirat(dirfd, path, len, buf, cap, flags)`: the directory's entries
/// into `buf`, one name per line; the bytes written.
pub(super) const SYS_LISTDIRAT: usize = 79;
/// `execat(dirfd, path, len, args, flags)`: replace the image with the
/// program (an ELF, or a `#!` script); `args` is the native exec block
/// (`copy_user_exec_pack`). Returns only on failure.
pub(super) const SYS_EXECAT: usize = 80;

/// The cwd, as a directory fd.
pub const AT_FDCWD: usize = -100isize as usize;
/// `statat`, `utimensat`: a symlink in the last component is not followed.
const AT_SYMLINK_NOFOLLOW: usize = 0x100;
/// `unlinkat`: remove a directory.
const AT_REMOVEDIR: usize = 0x200;
/// An empty path: the fd's own file.
const AT_EMPTY_PATH: usize = 0x1000;
/// `mknodat` kinds.
const MKNOD_DIR: usize = 0;
const MKNOD_FIFO: usize = 1;
/// `utimensat` time values: now, or leave the time as it is.
const UTIME_NOW: i64 = -1;
const UTIME_OMIT: i64 = -2;
/// Longest `listdirat` result.
const LISTDIR_MAX: usize = 256 * 1024;

const S_IFCHR: u32 = 0o020000;
const S_IFIFO: u32 = 0o010000;

/// The path at user `(ptr, len)` (empty when `len` is 0).
fn user_path(ptr: usize, len: usize) -> Option<String> {
    if len == 0 {
        return Some(String::new());
    }
    let buf = copy_user_path(ptr, len)?;
    core::str::from_utf8(&buf[..len]).ok().map(String::from)
}

/// The directory `dirfd` is open on, in the caller's view of the tree
/// (`None`: [`AT_FDCWD`], the cwd). `Err` when it is no directory, or one
/// the caller's namespace does not name.
fn base(dirfd: usize) -> Result<Option<String>, ()> {
    if dirfd == AT_FDCWD {
        return Ok(None);
    }
    let node = task::fd_file_node(dirfd).ok_or(())?;
    if fs::vfs::stat_node(&node).is_none_or(|st| st.mode & fs::S_IFMT != S_IFDIR) {
        return Err(());
    }
    let real = fs::vfs::node_path(&node).ok_or(())?;
    task::with_ns(|ns| match ns {
        None => Some(real),
        Some(ns) => ns.to_virtual(&real),
    })
    .map(Some)
    .ok_or(())
}

/// `path` relative to `dirfd`, as an absolute path in the caller's view.
fn virtual_at(dirfd: usize, path: &str) -> Option<String> {
    let dir = if path.starts_with('/') { None } else { base(dirfd).ok()? };
    let mut out = [0u8; MAX_PATH];
    let n = fs::resolve_user_path_virtual(dir.as_deref(), path, &mut out)?;
    core::str::from_utf8(&out[..n]).ok().map(String::from)
}

/// The real path `path` names relative to `dirfd`, symlinks followed (in
/// the last component only if `follow`).
fn resolve(dirfd: usize, path: &str, follow: bool) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let dir = if path.starts_with('/') { None } else { base(dirfd).ok()? };
    let mut out = [0u8; MAX_PATH];
    let n = fs::resolve_user_path_at(dir.as_deref(), path, &mut out, follow)?;
    core::str::from_utf8(&out[..n]).ok().map(String::from)
}

/// The file an empty path with [`AT_EMPTY_PATH`] names: `fd`'s.
fn empty_path(path: &str, flags: usize) -> bool {
    path.is_empty() && flags & AT_EMPTY_PATH != 0
}

pub(super) fn sys_openat(dirfd: usize, ptr: usize, len: usize, flags: usize) -> usize {
    let Some(path) = user_path(ptr, len) else {
        return SYSERR;
    };
    let tree = fs::vfs::hold_read();
    match resolve(dirfd, &path, true) {
        Some(real) => open_real(real, flags, tree),
        None => SYSERR,
    }
}

/// [`SYS_STATAT`]'s result.
#[repr(C)]
struct MyosStat {
    /// The type, and as permission bits what the caller may do
    /// (`crate::sec::mode_bits`).
    st_mode: u32,
    st_nlink: u32,
    st_ino: u32,
    st_dev: u32,
    st_size: u64,
    /// Seconds since the epoch, 0 where the filesystem keeps none.
    st_atime: i64,
    st_mtime: i64,
    /// The user the file's label names (`home(alice)`), else 0.
    st_uid: u32,
    st_gid: u32,
}

impl MyosStat {
    fn new(info: &fs::StatInfo, uid: u32) -> MyosStat {
        MyosStat {
            st_mode: info.mode,
            st_nlink: info.nlink,
            st_ino: info.ino,
            st_dev: info.dev,
            st_size: u64::from(info.size),
            st_atime: info.atime as i64,
            st_mtime: info.mtime as i64,
            st_uid: uid,
            st_gid: 0,
        }
    }
}

/// The type and the caller's rights on `real` as `stat` mode bits.
fn checked_mode(real: &str, mode: u32) -> u32 {
    let is_dir = mode & fs::S_IFMT == S_IFDIR;
    (mode & !0o777) | crate::sec::mode_bits(real, is_dir)
}

/// `stat` of open `fd`: a terminal or pipe has no file of its own; a file
/// unlinked since it was opened is still there for whoever holds it.
fn stat_fd(fd: usize) -> Option<MyosStat> {
    let blank = |mode| fs::StatInfo { mode, size: 0, ino: 0, nlink: 1, dev: 0, mtime: 0, atime: 0 };
    match task::fd_kind(fd)? {
        task::FdKind::Tty => Some(MyosStat::new(&blank(S_IFCHR | 0o600), 0)),
        task::FdKind::Pipe => Some(MyosStat::new(&blank(S_IFIFO | 0o600), 0)),
        task::FdKind::File { .. } => {
            let node = task::fd_file_node(fd)?;
            let mut info = fs::vfs::stat_node(&node)?;
            let uid = match fs::vfs::node_path(&node) {
                Some(real) => {
                    info.mode = checked_mode(&real, info.mode);
                    crate::sec::owner_uid(&real)
                }
                // Unlinked: what the fd was opened for.
                None => {
                    info.mode = (info.mode & !0o777) | 0o600;
                    0
                }
            };
            Some(MyosStat::new(&info, uid))
        }
    }
}

pub(super) fn sys_statat(dirfd: usize, ptr: usize, len: usize, flags: usize, out: usize) -> usize {
    if out == 0 || !user_range_ok(out, core::mem::size_of::<MyosStat>()) {
        return SYSERR;
    }
    let Some(path) = user_path(ptr, len) else {
        return SYSERR;
    };
    let st = if empty_path(&path, flags) {
        stat_fd(dirfd)
    } else {
        let _tree = fs::vfs::hold_read();
        resolve(dirfd, &path, flags & AT_SYMLINK_NOFOLLOW == 0).and_then(|real| {
            // A file the caller has no right at all on is not there for it.
            if crate::sec::rights_on(&real).is_empty() {
                return None;
            }
            let mut info = fs::stat(&real)?;
            info.mode = checked_mode(&real, info.mode);
            Some(MyosStat::new(&info, crate::sec::owner_uid(&real)))
        })
    };
    let Some(st) = st else {
        return SYSERR;
    };
    let bytes = unsafe {
        core::slice::from_raw_parts(&st as *const MyosStat as *const u8, core::mem::size_of::<MyosStat>())
    };
    if write_user_bytes(task::current_aspace(), out, bytes) { 0 } else { SYSERR }
}

pub(super) fn sys_mknodat(dirfd: usize, ptr: usize, len: usize, kind: usize) -> usize {
    let Some(path) = user_path(ptr, len) else {
        return SYSERR;
    };
    let _tree = fs::vfs::hold_read();
    let Some(real) = resolve(dirfd, &path, false) else {
        return SYSERR;
    };
    if fs::stat(&real).is_some() || !may(&real, Rights::CREATE) {
        return SYSERR;
    }
    let made = match kind {
        MKNOD_DIR => fs::mkdir(&real),
        MKNOD_FIFO => fs::vfs::mkfifo(&real),
        _ => false,
    };
    if made { 0 } else { SYSERR }
}

pub(super) fn sys_symlinkat(target_ptr: usize, target_len: usize, dirfd: usize, ptr: usize, len: usize) -> usize {
    // The target is text (it may be relative): stored, not resolved.
    let (Some(target), Some(path)) = (user_path(target_ptr, target_len), user_path(ptr, len)) else {
        return SYSERR;
    };
    if target.is_empty() {
        return SYSERR;
    }
    let _tree = fs::vfs::hold_read();
    let Some(real) = resolve(dirfd, &path, false) else {
        return SYSERR;
    };
    if may(&real, Rights::CREATE) && fs::symlink(&target, &real) { 0 } else { SYSERR }
}

pub(super) fn sys_unlinkat(dirfd: usize, ptr: usize, len: usize, flags: usize) -> usize {
    let Some(path) = user_path(ptr, len) else {
        return SYSERR;
    };
    let _tree = fs::vfs::hold_write();
    let Some(real) = resolve(dirfd, &path, false) else {
        return SYSERR;
    };
    if !may(&real, Rights::REMOVE) {
        return SYSERR;
    }
    let removed = if flags & AT_REMOVEDIR != 0 { fs::rmdir(&real) } else { fs::unlink(&real) };
    if removed { 0 } else { SYSERR }
}

pub(super) fn sys_renameat(
    old_dirfd: usize,
    old_ptr: usize,
    old_len: usize,
    new_dirfd: usize,
    new_ptr: usize,
    new_len: usize,
) -> usize {
    let (Some(old), Some(new)) = (user_path(old_ptr, old_len), user_path(new_ptr, new_len)) else {
        return SYSERR;
    };
    let _tree = fs::vfs::hold_write();
    let (Some(old), Some(new)) = (resolve(old_dirfd, &old, false), resolve(new_dirfd, &new, false)) else {
        return SYSERR;
    };
    // A move removes the old name and creates the new one (replacing a file
    // there removes it too).
    let replaced = fs::stat(&new).is_some();
    if !may(&old, Rights::REMOVE) || !may(&new, if replaced { Rights::CREATE | Rights::REMOVE } else { Rights::CREATE }) {
        return SYSERR;
    }
    if fs::rename(&old, &new) { 0 } else { SYSERR }
}

pub(super) fn sys_readlinkat(dirfd: usize, ptr: usize, len: usize, buf: usize, size: usize) -> usize {
    if buf == 0 || size == 0 || !user_range_ok(buf, size) {
        return SYSERR;
    }
    let Some(path) = user_path(ptr, len) else {
        return SYSERR;
    };
    let mut target = [0u8; MAX_PATH];
    let n = {
        let _tree = fs::vfs::hold_read();
        let Some(real) = resolve(dirfd, &path, false) else {
            return SYSERR;
        };
        if !may(&real, Rights::READ) {
            return SYSERR;
        }
        let cap = size.min(target.len());
        match fs::readlink(&real, &mut target[..cap]) {
            Some(n) => n,
            None => return SYSERR,
        }
    };
    if write_user_bytes(task::current_aspace(), buf, &target[..n]) { n } else { SYSERR }
}

/// The two times at user `times` (null: both now).
fn read_set_times(times: usize) -> Option<(fs::SetTime, fs::SetTime)> {
    if times == 0 {
        return Some((fs::SetTime::Now, fs::SetTime::Now));
    }
    let mut raw = [0u8; 16];
    if !user_range_ok(times, raw.len()) || !read_user_bytes(task::current_aspace(), times, &mut raw) {
        return None;
    }
    let one = |b: &[u8]| match i64::from_ne_bytes(b.try_into().unwrap()) {
        UTIME_NOW => Some(fs::SetTime::Now),
        UTIME_OMIT => Some(fs::SetTime::Omit),
        t if t >= 0 => Some(fs::SetTime::At(t as u64)),
        _ => None,
    };
    Some((one(&raw[..8])?, one(&raw[8..])?))
}

pub(super) fn sys_utimensat(dirfd: usize, ptr: usize, len: usize, times: usize, flags: usize) -> usize {
    let (Some(path), Some((atime, mtime))) = (user_path(ptr, len), read_set_times(times)) else {
        return SYSERR;
    };
    let _tree = fs::vfs::hold_read();
    if empty_path(&path, flags) {
        let Some(node) = task::fd_file_node(dirfd) else {
            return SYSERR;
        };
        let real = fs::vfs::vnode_path(&node);
        return if may(&real, Rights::SETATTR) && fs::set_times_node(&node, atime, mtime) { 0 } else { SYSERR };
    }
    let Some(real) = resolve(dirfd, &path, flags & AT_SYMLINK_NOFOLLOW == 0) else {
        return SYSERR;
    };
    if may(&real, Rights::SETATTR) && fs::set_times(&real, atime, mtime) { 0 } else { SYSERR }
}

pub(super) fn sys_chdirat(dirfd: usize, ptr: usize, len: usize, flags: usize) -> usize {
    match user_path(ptr, len) {
        Some(path) => chdir_at(dirfd, &path, flags),
        None => SYSERR,
    }
}

/// `chdirat` of a path already in kernel memory: the cwd becomes the
/// directory's node, which it follows wherever it moves, or a directory
/// the caller's namespace makes up.
pub(super) fn chdir_at(dirfd: usize, path: &str, flags: usize) -> usize {
    let _tree = fs::vfs::hold_read();
    let (real, node) = if empty_path(path, flags) {
        let Some(node) = task::fd_file_node(dirfd) else {
            return SYSERR;
        };
        let Some(real) = fs::vfs::node_path(&node) else {
            return SYSERR;
        };
        (real, Some(node))
    } else {
        let Some(real) = resolve(dirfd, path, true) else {
            return SYSERR;
        };
        let node = if real.starts_with('@') { None } else { fs::open(&real, 0) };
        (real, node)
    };
    if !may(&real, Rights::READ) || fs::stat(&real).is_none_or(|st| st.mode & fs::S_IFMT != S_IFDIR) {
        return SYSERR;
    }
    if node.is_none() && !real.starts_with('@') {
        return SYSERR;
    }
    // Its name in the caller's view: the cwd without a node.
    let virt = match &node {
        Some(_) => task::with_ns(|ns| match ns {
            None => Some(real.clone()),
            Some(ns) => ns.to_virtual(&real),
        }),
        None => Some(String::from(&real[1..])),
    };
    match virt {
        Some(virt) if task::set_cwd(virt.as_bytes(), node) => 0,
        _ => SYSERR,
    }
}

pub(super) fn sys_listdirat(dirfd: usize, ptr: usize, len: usize, buf: usize, cap: usize, flags: usize) -> usize {
    let cap = cap.min(LISTDIR_MAX);
    if buf == 0 || cap == 0 || !user_range_ok(buf, cap) {
        return SYSERR;
    }
    let Some(path) = user_path(ptr, len) else {
        return SYSERR;
    };
    let mut names = alloc::vec![0u8; cap];
    let n = {
        let _tree = fs::vfs::hold_read();
        let real = if empty_path(&path, flags) {
            task::fd_file_node(dirfd).and_then(|node| fs::vfs::node_path(&node))
        } else {
            resolve(dirfd, &path, true)
        };
        let Some(real) = real else {
            return SYSERR;
        };
        if !may(&real, Rights::READ) {
            return SYSERR;
        }
        fs::listdir(&real, &mut names).min(cap)
    };
    if write_user_bytes(task::current_aspace(), buf, &names[..n]) { n } else { SYSERR }
}

pub(super) fn sys_execat(dirfd: usize, ptr: usize, len: usize, args: usize, flags: usize) -> usize {
    let Some(path) = user_path(ptr, len) else {
        return SYSERR;
    };
    // The program as an absolute path in the caller's view: exec reads it
    // and hands it to a script's interpreter.
    let virt = {
        let _tree = fs::vfs::hold_read();
        if empty_path(&path, flags) {
            task::fd_file_node(dirfd).and_then(|node| fs::vfs::node_path(&node)).and_then(|real| {
                task::with_ns(|ns| match ns {
                    None => Some(real),
                    Some(ns) => ns.to_virtual(&real),
                })
            })
        } else if path.is_empty() {
            None
        } else {
            virtual_at(dirfd, &path)
        }
    };
    let Some(virt) = virt else {
        return SYSERR;
    };
    let (arg_bufs, env_bufs) = match copy_user_exec_pack(args) {
        Ok(v) => v,
        Err(()) => return SYSERR,
    };
    let arg_refs: Vec<&[u8]> = arg_bufs.iter().map(|s| s.as_slice()).collect();
    let env_refs: Vec<&[u8]> = env_bufs.iter().map(|s| s.as_slice()).collect();
    exec_path(&virt, &arg_refs, &env_refs)
}
