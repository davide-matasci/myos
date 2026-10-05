//! Tiny VFS facade: syscalls and modules talk to [`vfs`]; rootfs is one mount.

pub mod cpio;
mod devfs;
pub mod ptsfs;
mod fstype;
mod procfs;
pub mod rootfs;
mod tmpfs;
pub mod vfs;

pub use vfs::{SetTime, StatInfo, Vnode};

fn path_is_dev_tty(path: &str) -> bool {
    let path = path.trim_start_matches('/');
    path == "dev/tty"
}

/// Resolve `path` to a vnode for open/read/write.
///
/// `/dev/console/data` is always the hardware console. `/dev/tty` resolves
/// to it only when the caller has a controlling terminal (ENXIO otherwise);
/// a pty session's `/dev/tty` is resolved before this, in `open_path`.
pub fn open(path: &str, flags: u32) -> Option<Vnode> {
    if path_is_dev_tty(path) {
        if !crate::task::has_ctty() {
            return None;
        }
        return vfs::open("/dev/console/data", flags);
    }
    vfs::open(path, flags)
}

/// Look up `path` on the best matching mount.
pub fn lookup(path: &str) -> Option<&'static [u8]> {
    vfs::lookup(path)
}

/// Read from an open vnode.
pub fn read(node: &Vnode, pos: usize, out: &mut [u8]) -> usize {
    vfs::read(node, pos, out)
}

/// Read a whole file by path (static lookup first, then VFS read).
///
/// Needed so `exec` can load a `tcc -o` ELF off tmpfs/ext2, where `lookup`
/// is not `'static`.
pub fn read_all(path: &str, max: usize) -> Option<alloc::vec::Vec<u8>> {
    vfs::read_all(path, max)
}

/// Write to an open vnode.
pub fn write(node: &Vnode, pos: usize, buf: &[u8]) -> Option<usize> {
    vfs::write(node, pos, buf)
}

/// The device page at `offset` of `node`, for a shared `mmap` (see
/// [`vfs::device_frame`]).
pub fn device_frame(node: &Vnode, offset: usize) -> Option<u64> {
    vfs::device_frame(node, offset)
}

/// `poll` readiness of a device or module file (see [`vfs::poll`]).
pub fn poll(node: &Vnode) -> Option<u32> {
    vfs::poll(node)
}

/// The console's control file (`/dev/console/ctl`, docs/tty.md), for an fd
/// on the console: its text, and a write to it.
pub fn console_ctl_text() -> alloc::vec::Vec<u8> {
    devfs::console_ctl_text()
}

pub fn console_ctl_write(text: &[u8]) -> Option<usize> {
    devfs::console_ctl_write(text)
}

/// Size of an open vnode (for `O_APPEND`).
pub fn size_of(node: &Vnode) -> Option<usize> {
    vfs::size_of(node)
}

/// Whether open `flags` request write access.
pub fn open_writable(flags: u32) -> bool {
    vfs::open_writable(flags)
}

/// Whether open `flags` include `O_APPEND`.
pub fn open_append(flags: u32) -> bool {
    vfs::open_append(flags)
}

/// Stat `path` on the best matching mount.
pub fn stat(path: &str) -> Option<StatInfo> {
    vfs::stat(path)
}

/// Set the access and modification times of `path`.
pub fn set_times(path: &str, atime: SetTime, mtime: SetTime) -> bool {
    vfs::set_times(path, atime, mtime)
}

/// Set the access and modification times of an open vnode.
pub fn set_times_node(node: &Vnode, atime: SetTime, mtime: SetTime) -> bool {
    vfs::set_times_node(node, atime, mtime)
}

/// List entries at `path` into `buf` (newline-separated basenames).
pub fn listdir(path: &str, buf: &mut [u8]) -> usize {
    vfs::listdir(path, buf)
}

/// Create a directory.
pub fn mkdir(path: &str) -> bool {
    vfs::mkdir(path)
}

/// Remove an empty directory.
pub fn rmdir(path: &str) -> bool {
    vfs::rmdir(path)
}

/// Unlink a file or symlink.
pub fn unlink(path: &str) -> bool {
    vfs::unlink(path)
}

/// Rename within one mount.
pub fn rename(old: &str, new: &str) -> bool {
    vfs::rename(old, new)
}

/// Create a symbolic link.
pub fn symlink(target: &str, linkpath: &str) -> bool {
    vfs::symlink(target, linkpath)
}

/// Read a symbolic link into `buf`.
pub fn readlink(path: &str, buf: &mut [u8]) -> Option<usize> {
    vfs::readlink(path, buf)
}

/// Register `name` on mount `mount_name` (rootfs keeps the bytes).
pub fn register(mount_name: &str, name: &str, bytes: &'static [u8]) -> bool {
    vfs::register(mount_name, name, bytes)
}

/// Register without copying; `bytes` must outlive the kernel.
pub fn register_static(mount_name: &str, name: &str, bytes: &'static [u8]) -> bool {
    vfs::register_static(mount_name, name, bytes)
}

/// Mount a module-provided backend at `/prefix/…`.
pub fn mount_module(name: &str, prefix: &str, ops: myos_abi::ModuleVfsOps) -> bool {
    vfs::mount_module(name, prefix, ops, "none")
}

/// Register a filesystem type (`fat`, …). `bind` is invoked from `mount(2)`.
pub fn register_fstype(name: &str, bind: myos_abi::FsBind) -> bool {
    fstype::register(name, bind)
}

/// Register a module character device under `/dev/<name>` (`S_IFCHR | 0666`).
pub fn register_chrdev(name: &str, ops: myos_abi::ModuleChrOps) -> bool {
    devfs::register_chrdev(name, ops)
}

/// Bind `dev` to `fstype` and mount it at `prefix`: an existing directory
/// that is not a mount point yet, at the top level or below it. A disk is
/// mounted once.
pub fn mount_fstype(source_dev: u32, prefix: &str, fstype_name: &str, source: &str) -> bool {
    if !vfs::mount_point_free(prefix) || vfs::source_mounted(source) {
        return false;
    }
    let Some(ops) = fstype::bind(fstype_name, source_dev) else {
        return false;
    };
    vfs::mount_instance(fstype_name, prefix, ops, source)
}

/// The block device `/dev/<name>` is the source of a mount or is open
/// (`blk::unregister` refuses it then).
pub fn blk_in_use(name: &str) -> bool {
    let path = alloc::format!("/dev/{name}");
    vfs::source_mounted(&path) || vfs::open_refs("dev", name) > 0
}

/// Block-device id for a `/dev/<name>` path (`vdX`, `nvmeXn1`, …), if registered.
pub fn blk_id_from_path(path: &str) -> Option<u32> {
    let path = path.trim_start_matches('/');
    let name = path.strip_prefix("dev/")?;
    if name.contains('/') {
        return None;
    }
    devfs::blk_id(name)
}

pub const S_IFBLK: u32 = 0o060000;
pub const S_IFMT: u32 = 0o170000;

/// Resolve `path` against the current task cwd into `out`, in the task's
/// own view of the tree (canonical absolute; after chroot `/` is the jail).
/// `..` is resolved here, so it can never climb above that `/`.
pub fn resolve_user_path_virtual(path: &str, out: &mut [u8]) -> Option<usize> {
    let mut cwd = [0u8; 256];
    let n = crate::task::cwd(&mut cwd);
    let cwd = core::str::from_utf8(&cwd[..n]).unwrap_or("/");
    vfs::resolve_against_cwd(cwd, path, out)
}

/// Resolve `path` into the real absolute path (`out`) the VFS understands:
/// the virtual path from [`resolve_user_path_virtual`], with symlinks
/// followed (the last component too), under the task's chroot prefix.
pub fn resolve_user_path(path: &str, out: &mut [u8]) -> Option<usize> {
    resolve_user_path_with(path, out, true)
}

/// Like [`resolve_user_path`], but a symlink in the last component is not
/// followed (`lstat`, `readlink`, `unlink`, `rename`, `symlink`, ...).
pub fn resolve_user_path_nofollow(path: &str, out: &mut [u8]) -> Option<usize> {
    resolve_user_path_with(path, out, false)
}

fn resolve_user_path_with(path: &str, out: &mut [u8], follow_last: bool) -> Option<usize> {
    // Unjailed and no symlinks anywhere (the common case): resolve straight
    // into `out` — no extra buffers on the kernel stack of every path syscall.
    let follow = vfs::symlinks_possible();
    if !crate::task::has_root() && !follow {
        let n = resolve_user_path_virtual(path, out)?;
        if !out[..n].starts_with(PROC_SELF) {
            return Some(n);
        }
        // `/proc/self` holds symlinks (`fd/N`, `tty`) whatever the tmpfs does.
        let mut virt = [0u8; vfs::PATH_MAX];
        virt[..n].copy_from_slice(&out[..n]);
        let vn = follow_symlinks(&mut virt, n, follow_last)?;
        out[..vn].copy_from_slice(&virt[..vn]);
        return Some(vn);
    }
    let mut virt = [0u8; vfs::PATH_MAX];
    let mut vn = resolve_user_path_virtual(path, &mut virt)?;
    if follow || virt[..vn].starts_with(PROC_SELF) {
        vn = follow_symlinks(&mut virt, vn, follow_last)?;
    }
    virtual_to_real(&virt[..vn], out)
}

/// The directory of procfs symlinks (`procfs::readlink`).
const PROC_SELF: &[u8] = b"/proc/self/";

/// The real path behind `virt` (a canonical path in the task's view): the
/// task's chroot prefix + `virt`.
fn virtual_to_real(virt: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut root = [0u8; crate::task::ROOT_CAP];
    let rn = crate::task::root(&mut root);
    let tail: &[u8] = if rn != 0 && virt == b"/" { &[] } else { virt };
    if rn + tail.len() > out.len() {
        return None;
    }
    out[..rn].copy_from_slice(&root[..rn]);
    out[rn..rn + tail.len()].copy_from_slice(tail);
    Some(rn + tail.len())
}

/// Symlinks followed while resolving one path (Linux's `MAXSYMLINKS`).
const MAX_SYMLINKS: usize = 40;

/// Replace each symlink along `virt[..vn]` (a canonical path in the task's
/// view) by its target, the last component only if `follow_last`. Targets
/// are resolved in the same view, so an absolute one stays inside a chroot
/// and `..` cannot climb out of it.
fn follow_symlinks(virt: &mut [u8; vfs::PATH_MAX], mut vn: usize, follow_last: bool) -> Option<usize> {
    let mut hops = 0;
    // Start of the next component to check; everything before is link-free.
    let mut start = 1;
    while start < vn {
        let end = virt[start..vn].iter().position(|&b| b == b'/').map_or(vn, |i| start + i);
        if end < vn || follow_last {
            let mut real = [0u8; vfs::PATH_MAX];
            let rn = virtual_to_real(&virt[..end], &mut real)?;
            let mut target = [0u8; vfs::PATH_MAX];
            if let Some(tn) = vfs::readlink(core::str::from_utf8(&real[..rn]).ok()?, &mut target) {
                hops += 1;
                if hops > MAX_SYMLINKS || tn == 0 {
                    return None;
                }
                // The target, then the rest of the path, relative to the
                // directory holding the link.
                let rest = &virt[end..vn];
                let mut joined = [0u8; vfs::PATH_MAX];
                if tn + rest.len() > joined.len() {
                    return None;
                }
                joined[..tn].copy_from_slice(&target[..tn]);
                joined[tn..tn + rest.len()].copy_from_slice(rest);
                let dir: &[u8] = if start > 1 { &virt[..start - 1] } else { b"/" };
                let mut next = [0u8; vfs::PATH_MAX];
                vn = vfs::resolve_against_cwd(
                    core::str::from_utf8(dir).ok()?,
                    core::str::from_utf8(&joined[..tn + rest.len()]).ok()?,
                    &mut next,
                )?;
                virt[..vn].copy_from_slice(&next[..vn]);
                // The target may itself go through links: check from the top.
                start = 1;
                continue;
            }
        }
        start = end + 1;
    }
    Some(vn)
}

fn reject_mkdir(_path: &str) -> bool {
    false
}
fn reject_rmdir(_path: &str) -> bool {
    false
}
fn reject_unlink(_path: &str) -> bool {
    false
}
fn reject_rename(_old: &str, _new: &str) -> bool {
    false
}
fn reject_symlink(_target: &str, _linkpath: &str) -> bool {
    false
}
fn reject_readlink(_path: &str, _buf: &mut [u8]) -> Option<usize> {
    None
}

fn ro_ops(
    lookup: fn(&str) -> Option<&'static [u8]>,
    stat: fn(&str) -> Option<StatInfo>,
    listdir: fn(&str, &mut [u8]) -> usize,
    register: fn(&str, &'static [u8]) -> bool,
    create: fn(&str) -> bool,
    truncate: fn(&str) -> bool,
    read: fn(&str, usize, &mut [u8]) -> usize,
    write: fn(&str, usize, &[u8]) -> Option<usize>,
) -> vfs::MountOps {
    vfs::MountOps {
        lookup,
        stat,
        listdir,
        register,
        create,
        truncate,
        read,
        write,
        mkdir: reject_mkdir,
        rmdir: reject_rmdir,
        unlink: reject_unlink,
        rename: reject_rename,
        symlink: reject_symlink,
        readlink: reject_readlink,
        poll: None,
        set_times: None,
        writable: false,
    }
}

fn rw_ops(
    lookup: fn(&str) -> Option<&'static [u8]>,
    stat: fn(&str) -> Option<StatInfo>,
    listdir: fn(&str, &mut [u8]) -> usize,
    register: fn(&str, &'static [u8]) -> bool,
    create: fn(&str) -> bool,
    truncate: fn(&str) -> bool,
    read: fn(&str, usize, &mut [u8]) -> usize,
    write: fn(&str, usize, &[u8]) -> Option<usize>,
    mkdir: fn(&str) -> bool,
    rmdir: fn(&str) -> bool,
    unlink: fn(&str) -> bool,
    rename: fn(&str, &str) -> bool,
    symlink: fn(&str, &str) -> bool,
    readlink: fn(&str, &mut [u8]) -> Option<usize>,
) -> vfs::MountOps {
    vfs::MountOps {
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
        poll: None,
        set_times: None,
        writable: true,
    }
}

/// Mount rootfs at `/` (the image's files, read-only), tmpfs at `/tmp/`,
/// devfs at `/dev/` with ptsfs at `/dev/pts/`, procfs at `/proc/`.
pub fn init() {
    vfs::mount(
        "rootfs",
        "",
        ro_ops(
            rootfs::lookup,
            rootfs::stat,
            rootfs::listdir_at,
            rootfs::register,
            rootfs::create,
            rootfs::truncate,
            rootfs::read,
            rootfs::write,
        ),
    );
    vfs::mount(
        "tmpfs",
        "tmp",
        vfs::MountOps {
            set_times: Some(tmpfs::set_times),
            ..rw_ops(
                tmpfs::lookup,
                tmpfs::stat,
                tmpfs::listdir_at,
                tmpfs::register,
                tmpfs::create,
                tmpfs::truncate,
                tmpfs::read,
                tmpfs::write,
                tmpfs::mkdir,
                tmpfs::rmdir,
                tmpfs::unlink,
                tmpfs::rename,
                tmpfs::symlink,
                tmpfs::readlink,
            )
        },
    );
    // Device nodes are fixed; mutation ops stay rejected.
    {
        let mut ops = rw_ops(
            devfs::lookup,
            devfs::stat,
            devfs::listdir_at,
            devfs::register,
            devfs::create,
            devfs::truncate,
            devfs::read,
            devfs::write,
            reject_mkdir,
            reject_rmdir,
            reject_unlink,
            reject_rename,
            reject_symlink,
            reject_readlink,
        );
        ops.poll = Some(devfs::poll);
        vfs::mount("devfs", "dev", ops);
    }
    // The ptys: /dev/pts/clone and /dev/pts/N/{master,data,ctl}. Only ctl
    // is read and written here (fds on the ends route via crate::task).
    vfs::mount(
        "ptsfs",
        "dev/pts",
        rw_ops(
            ptsfs::lookup,
            ptsfs::stat,
            ptsfs::listdir_at,
            ptsfs::register,
            ptsfs::create,
            ptsfs::truncate,
            ptsfs::read,
            ptsfs::write,
            reject_mkdir,
            reject_rmdir,
            reject_unlink,
            reject_rename,
            reject_symlink,
            reject_readlink,
        ),
    );
    vfs::mount(
        "procfs",
        "proc",
        rw_ops(
            procfs::lookup,
            procfs::stat,
            procfs::listdir_at,
            procfs::register,
            procfs::create,
            procfs::truncate,
            procfs::read,
            procfs::write,
            reject_mkdir,
            reject_rmdir,
            reject_unlink,
            reject_rename,
            reject_symlink,
            procfs::readlink,
        ),
    );
}

/// Unpack the initramfs into rootfs.
pub fn init_limine() {
    rootfs::init_limine();
}


/// Register a generated `/proc/<name>` node (used by loadable modules).
pub fn procfs_register(name: &str, data: &'static [u8]) -> bool {
    procfs::register_dynamic(name, data)
}

pub fn procfs_set_writer(
    name: &str,
    writer: Option<unsafe extern "C" fn(*const u8, usize) -> i32>,
) -> bool {
    procfs::set_writer(name, writer)
}
