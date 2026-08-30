//! Tiny VFS facade: syscalls and modules talk to [`vfs`]; bootfs is one mount.

pub mod bootfs;
mod vfs;

pub use vfs::StatInfo;

/// Look up `path` on the best matching mount.
pub fn lookup(path: &str) -> Option<&'static [u8]> {
    vfs::lookup(path)
}

/// Stat `path` on the best matching mount.
pub fn stat(path: &str) -> Option<StatInfo> {
    vfs::stat(path)
}

/// List entries on the root mount into `buf` (newline-separated basenames).
pub fn listdir(buf: &mut [u8]) -> usize {
    vfs::listdir(buf)
}

/// Register `name` on mount `mount_name`.
pub fn register(mount_name: &str, name: &str, bytes: &'static [u8]) -> bool {
    vfs::register(mount_name, name, bytes)
}

/// Mount bootfs at `/` and register embedded user ELFs.
pub fn init() {
    vfs::mount(
        "bootfs",
        "",
        vfs::MountOps {
            lookup: bootfs::lookup,
            stat: bootfs::stat,
            listdir: bootfs::listdir,
            register: bootfs::register,
        },
    );
    bootfs::init_embedded();
}

/// Ingest Limine ESP modules into bootfs (overrides embedded names).
pub fn init_limine() {
    bootfs::init_limine();
}
