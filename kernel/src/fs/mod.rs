//! Tiny VFS facade: syscalls and modules talk to [`vfs`]; bootfs is one mount.

pub mod bootfs;
mod sbasefs;
mod vfs;

pub use vfs::{StatInfo, Vnode};

/// Resolve `path` to a vnode for open/read.
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

/// Stat `path` on the best matching mount.
pub fn stat(path: &str) -> Option<StatInfo> {
    vfs::stat(path)
}

/// List entries at `path` into `buf` (newline-separated basenames).
pub fn listdir(path: &str, buf: &mut [u8]) -> usize {
    vfs::listdir(path, buf)
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

/// Mount bootfs at `/`, sbasefs at `/s/`, and register embedded user ELFs.
pub fn init() {
    vfs::mount(
        "bootfs",
        "",
        vfs::MountOps {
            lookup: bootfs::lookup,
            stat: bootfs::stat,
            listdir: bootfs::listdir_at,
            register: bootfs::register,
        },
    );
    bootfs::init_embedded();
    vfs::mount(
        "sbasefs",
        "s",
        vfs::MountOps {
            lookup: sbasefs::lookup,
            stat: sbasefs::stat,
            listdir: sbasefs::listdir_at,
            register: sbasefs::register,
        },
    );
    sbasefs::init_embedded();
}

/// Ingest Limine ESP modules into bootfs (overrides embedded names).
pub fn init_limine() {
    bootfs::init_limine();
}
