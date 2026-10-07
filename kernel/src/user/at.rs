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
/// creates it; with `O_EXCL` only a new one, `EEXIST` when the name is
/// taken, by a symlink too; `O_CLOEXEC`: the fd closes at exec;
/// `O_NOFOLLOW`: `ELOOP` for a symlink; `O_DIRECTORY`: `ENOTDIR` for
/// anything but a directory).
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

/// Where a path relative to a directory fd starts.
enum Base {
    /// [`AT_FDCWD`]: the cwd.
    Cwd,
    /// A directory the caller's namespace names: its path in that view.
    Named(String),
    /// One it does not name (the fd came from a process that could, or
    /// from before a `ns`): a capability. Its real path, and the rights the
    /// fd grants beneath it; paths resolve beneath it only.
    Cap(String, Rights),
}

/// Where `dirfd` starts a relative path; `Err` when it is no directory (or
/// one unlinked since).
fn base(dirfd: usize) -> Result<Base, ()> {
    if dirfd == AT_FDCWD {
        return Ok(Base::Cwd);
    }
    let node = task::fd_file_node(dirfd).ok_or(())?;
    if fs::vfs::stat_node(&node).is_none_or(|st| st.mode & fs::S_IFMT != S_IFDIR) {
        return Err(());
    }
    let real = fs::vfs::node_path(&node).ok_or(())?;
    let virt = task::with_ns(|ns| match ns {
        None => Some(real.clone()),
        Some(ns) => ns.to_virtual(&real),
    });
    match virt {
        Some(virt) => Ok(Base::Named(virt)),
        None => Ok(Base::Cap(real, task::fd_rights(dirfd).ok_or(())?)),
    }
}

/// `path` relative to `dirfd`, as an absolute path in the caller's view
/// (`None` beneath a capability: it has no name there).
fn virtual_at(dirfd: usize, path: &str) -> Option<String> {
    let dir = match path.starts_with('/') {
        true => None,
        false => match base(dirfd).ok()? {
            Base::Cwd => None,
            Base::Named(dir) => Some(dir),
            Base::Cap(..) => return None,
        },
    };
    let mut out = [0u8; MAX_PATH];
    let n = fs::resolve_user_path_virtual(dir.as_deref(), path, &mut out)?;
    core::str::from_utf8(&out[..n]).ok().map(String::from)
}

/// A file a path names: its real path, and the rights the caller has on it
/// before the policy's.
struct Found {
    real: String,
    /// The capability's rights when it was found beneath one ([`Base::Cap`]),
    /// else `None`: the namespace's.
    cap: Option<Rights>,
}

impl Found {
    fn ns_rights(&self) -> Rights {
        self.cap.unwrap_or_else(|| task::ns_rights(&self.real))
    }

    /// May the caller do `need` to the file (`may`, with a capability's
    /// rights in place of the namespace's)?
    fn may(&self, need: Rights) -> bool {
        crate::sec::allowed_in(&self.real, need, self.ns_rights())
    }
}

/// The file `path` names relative to `dirfd`, symlinks followed (in the
/// last component only if `follow`).
fn resolve(dirfd: usize, path: &str, follow: bool) -> Option<Found> {
    if path.is_empty() {
        return None;
    }
    let dir = match path.starts_with('/') {
        true => None,
        false => match base(dirfd).ok()? {
            Base::Cwd => None,
            Base::Named(dir) => Some(dir),
            Base::Cap(real, rights) => {
                let real = fs::resolve_beneath(&real, path, follow)?;
                return Some(Found { real, cap: Some(rights) });
            }
        },
    };
    let mut out = [0u8; MAX_PATH];
    let n = fs::resolve_user_path_at(dir.as_deref(), path, &mut out, follow)?;
    let real = core::str::from_utf8(&out[..n]).ok().map(String::from)?;
    Some(Found { real, cap: None })
}

/// The file open `fd` is on, with the rights it grants (its own, see
/// `OpenFile::rights`); `None` for one unlinked.
fn fd_found(fd: usize) -> Option<(fs::Vnode, Found)> {
    let node = task::fd_file_node(fd)?;
    let real = fs::vfs::node_path(&node)?;
    let cap = Some(task::fd_rights(fd)?);
    Some((node, Found { real, cap }))
}

/// The file an empty path with [`AT_EMPTY_PATH`] names: `fd`'s.
fn empty_path(path: &str, flags: usize) -> bool {
    path.is_empty() && flags & AT_EMPTY_PATH != 0
}

pub(super) fn sys_openat(dirfd: usize, ptr: usize, len: usize, flags: usize) -> usize {
    let Some(path) = user_path(ptr, len) else {
        return SYSERR;
    };
    let tree = if open_excl(flags) { fs::vfs::hold_write() } else { fs::vfs::hold_read() };
    match resolve(dirfd, &path, open_follows(flags)) {
        Some(f) => open_real(f.real, f.cap, flags, tree),
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

/// The type and the caller's rights on `f` as `stat` mode bits.
fn checked_mode(f: &Found, mode: u32) -> u32 {
    let is_dir = mode & fs::S_IFMT == S_IFDIR;
    (mode & !0o777) | crate::sec::mode_bits_in(&f.real, is_dir, f.ns_rights())
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
            let uid = match fd_found(fd) {
                Some((_, f)) => {
                    info.mode = checked_mode(&f, info.mode);
                    crate::sec::owner_uid(&f.real)
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
        resolve(dirfd, &path, flags & AT_SYMLINK_NOFOLLOW == 0).and_then(|f| {
            // A file the caller has no right at all on is not there for it.
            if crate::sec::rights_in(&f.real, f.ns_rights()).is_empty() {
                return None;
            }
            let mut info = fs::stat(&f.real)?;
            info.mode = checked_mode(&f, info.mode);
            Some(MyosStat::new(&info, crate::sec::owner_uid(&f.real)))
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
    let Some(f) = resolve(dirfd, &path, false) else {
        return SYSERR;
    };
    if fs::stat(&f.real).is_some() || !f.may(Rights::CREATE) {
        return SYSERR;
    }
    let made = match kind {
        MKNOD_DIR => fs::mkdir(&f.real),
        MKNOD_FIFO => fs::vfs::mkfifo(&f.real),
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
    let Some(f) = resolve(dirfd, &path, false) else {
        return SYSERR;
    };
    if f.may(Rights::CREATE) && fs::symlink(&target, &f.real) { 0 } else { SYSERR }
}

pub(super) fn sys_unlinkat(dirfd: usize, ptr: usize, len: usize, flags: usize) -> usize {
    let Some(path) = user_path(ptr, len) else {
        return SYSERR;
    };
    let _tree = fs::vfs::hold_write();
    let Some(f) = resolve(dirfd, &path, false) else {
        return SYSERR;
    };
    if !f.may(Rights::REMOVE) {
        return SYSERR;
    }
    let removed = if flags & AT_REMOVEDIR != 0 { fs::rmdir(&f.real) } else { fs::unlink(&f.real) };
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
    // there removes it too), and takes what is beneath a directory along
    // (`sec::may_rename`).
    let replaced = fs::stat(&new.real).is_some();
    if !crate::sec::may_rename(&old.real, old.cap, &new.real, new.cap, replaced) {
        return SYSERR;
    }
    if fs::rename(&old.real, &new.real) { 0 } else { SYSERR }
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
        let Some(f) = resolve(dirfd, &path, false) else {
            return SYSERR;
        };
        if !f.may(Rights::READ) {
            return SYSERR;
        }
        let cap = size.min(target.len());
        match fs::readlink(&f.real, &mut target[..cap]) {
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
        let Some((node, f)) = fd_found(dirfd) else {
            return SYSERR;
        };
        return if f.may(Rights::SETATTR) && fs::set_times_node(&node, atime, mtime) { 0 } else { SYSERR };
    }
    let Some(f) = resolve(dirfd, &path, flags & AT_SYMLINK_NOFOLLOW == 0) else {
        return SYSERR;
    };
    if f.may(Rights::SETATTR) && fs::set_times(&f.real, atime, mtime) { 0 } else { SYSERR }
}

pub(super) fn sys_chdirat(dirfd: usize, ptr: usize, len: usize, flags: usize) -> usize {
    match user_path(ptr, len) {
        Some(path) => chdir_at(dirfd, &path, flags),
        None => SYSERR,
    }
}

/// `chdirat` of a path already in kernel memory: the cwd becomes the
/// directory's node, which it follows wherever it moves, or a directory
/// the caller's namespace makes up. The cwd has a name in the caller's
/// view: a directory beneath a capability cannot be it.
pub(super) fn chdir_at(dirfd: usize, path: &str, flags: usize) -> usize {
    let _tree = fs::vfs::hold_read();
    let (f, node) = if empty_path(path, flags) {
        let Some((node, f)) = fd_found(dirfd) else {
            return SYSERR;
        };
        (f, Some(node))
    } else {
        let Some(f) = resolve(dirfd, path, true) else {
            return SYSERR;
        };
        let node = if f.real.starts_with('@') { None } else { fs::open(&f.real, 0) };
        (f, node)
    };
    let real = &f.real;
    if !f.may(Rights::READ) || fs::stat(real).is_none_or(|st| st.mode & fs::S_IFMT != S_IFDIR) {
        return SYSERR;
    }
    if node.is_none() && !real.starts_with('@') {
        return SYSERR;
    }
    // Its name in the caller's view: the cwd without a node.
    let virt = match &node {
        Some(_) => task::with_ns(|ns| match ns {
            None => Some(real.clone()),
            Some(ns) => ns.to_virtual(real),
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
        let f = if empty_path(&path, flags) { fd_found(dirfd).map(|(_, f)| f) } else { resolve(dirfd, &path, true) };
        let Some(f) = f else {
            return SYSERR;
        };
        if !f.may(Rights::READ) {
            return SYSERR;
        }
        fs::listdir(&f.real, &mut names).min(cap)
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
