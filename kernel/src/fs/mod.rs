//! Tiny VFS facade: syscalls and modules talk to [`vfs`]; bootfs is one mount.

pub mod bootfs;
mod coreutilsfs;
mod devfs;
mod sbasefs;
mod tmpfs;
mod vfs;

pub use vfs::{StatInfo, Vnode};

/// Resolve `path` to a vnode for open/read/write.
pub fn open(path: &str, flags: u32) -> Option<Vnode> {
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

/// Write to an open vnode.
pub fn write(node: &Vnode, pos: usize, buf: &[u8]) -> Option<usize> {
    vfs::write(node, pos, buf)
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

/// Register `name` on mount `mount_name` (bootfs copies into its table).
pub fn register(mount_name: &str, name: &str, bytes: &'static [u8]) -> bool {
    vfs::register(mount_name, name, bytes)
}

/// Register without copying; `bytes` must outlive the kernel.
pub fn register_static(mount_name: &str, name: &str, bytes: &'static [u8]) -> bool {
    vfs::register_static(mount_name, name, bytes)
}

/// Mount a module-provided backend at `/prefix/…`.
pub fn mount_module(name: &str, prefix: &str, ops: myos_abi::ModuleVfsOps) -> bool {
    vfs::mount_module(name, prefix, ops)
}


/// Resolve `path` against the current task cwd into `out` (absolute).
pub fn resolve_user_path(path: &str, out: &mut [u8]) -> Option<usize> {
    let mut cwd = [0u8; 64];
    let n = crate::task::cwd(&mut cwd);
    let cwd = core::str::from_utf8(&cwd[..n]).unwrap_or("/");
    vfs::resolve_against_cwd(cwd, path, out)
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
        writable: true,
    }
}

/// Mount bootfs at `/`, sbasefs at `/s/`, coreutilsfs at `/c/`, tmpfs at `/tmp/`,
/// devfs at `/dev/`, and register embedded user ELFs.
pub fn init() {
    vfs::mount(
        "bootfs",
        "",
        ro_ops(
            bootfs::lookup,
            bootfs::stat,
            bootfs::listdir_at,
            bootfs::register,
            bootfs::create,
            bootfs::truncate,
            bootfs::read,
            bootfs::write,
        ),
    );
    bootfs::init_embedded();
    vfs::mount(
        "sbasefs",
        "s",
        ro_ops(
            sbasefs::lookup,
            sbasefs::stat,
            sbasefs::listdir_at,
            sbasefs::register,
            sbasefs::create,
            sbasefs::truncate,
            sbasefs::read,
            sbasefs::write,
        ),
    );
    sbasefs::init_embedded();
    vfs::mount(
        "coreutilsfs",
        "c",
        ro_ops(
            coreutilsfs::lookup,
            coreutilsfs::stat,
            coreutilsfs::listdir_at,
            coreutilsfs::register,
            coreutilsfs::create,
            coreutilsfs::truncate,
            coreutilsfs::read,
            coreutilsfs::write,
        ),
    );
    coreutilsfs::init_embedded();
    vfs::mount(
        "tmpfs",
        "tmp",
        rw_ops(
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
        ),
    );
    // Device nodes are fixed; mutation ops stay rejected.
    vfs::mount(
        "devfs",
        "dev",
        rw_ops(
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
        ),
    );
}

/// Ingest Limine ESP modules into bootfs (overrides embedded names).
pub fn init_limine() {
    bootfs::init_limine();
}
