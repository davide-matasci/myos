//! The ext2 on-disk format over a block device: [`mkfs`], and a mounted
//! [`Fs`] that reads and writes files, directories and symlinks.
//!
//! Revision 1 ext2 as Linux and e2fsprogs know it: 1, 2 or 4 KiB blocks in
//! block groups, 128- or 256-byte inodes, direct and single/double/triple
//! indirect blocks, typed directory entries (`filetype`), sparse superblock
//! backups and files over 2 GiB (`large_file`). Other incompatible features
//! (journals, extents, ...) are refused at mount. Hard links and extended
//! attributes are not created; `dir_index` directories are read linearly
//! (their index is a valid linear directory) and lose the index when changed.
//!
//! Paths are relative to the filesystem root, `/`-separated, without a
//! leading `/`, and are not resolved through symlinks (the caller does that,
//! with [`Fs::readlink`]). Metadata and data go through a block cache that
//! every public call flushes before it returns.
//!
//! `no_std` + `alloc`, used by the ext2 kernel module and by `mkfs.ext2`, and
//! tested on the host against e2fsprogs (`cargo test -p ext2fs`).

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod cache;
mod dir;
mod fs;
mod inode;
mod layout;
mod mkfs;

#[cfg(test)]
mod tests;

pub use fs::Fs;
pub use mkfs::mkfs;

/// A block device addressed in bytes.
pub trait Device {
    /// Read `buf.len()` bytes at byte `offset`.
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> bool;
    /// Write `buf` at byte `offset`.
    fn write(&mut self, offset: u64, buf: &[u8]) -> bool;
    /// Seconds since the epoch, for inode times (0 when unknown).
    fn now(&mut self) -> u32 {
        0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    NotFound,
    Exists,
    NotDir,
    IsDir,
    NotEmpty,
    NoSpace,
    /// Beyond what the format (or this filesystem's features) can address.
    TooBig,
    Invalid,
    /// Not an ext2 filesystem this code can mount.
    Unsupported,
    Io,
}

pub type Result<T> = core::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Debug, Clone, Copy)]
pub struct Stat {
    pub kind: Kind,
    /// `i_mode`: the file type bits and the permissions.
    pub mode: u16,
    pub size: u64,
    pub ino: u32,
    pub links: u16,
}
