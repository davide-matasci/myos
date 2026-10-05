//! `MetadataExt` for myos: what the kernel's stat reports (no owners, no
//! sub-second times, the change time is the modification time).
#![stable(feature = "rust1", since = "1.0.0")]

use crate::fs::Metadata;
use crate::sys::AsInner;

#[stable(feature = "rust1", since = "1.0.0")]
pub trait MetadataExt {
    #[stable(feature = "rust1", since = "1.0.0")]
    fn dev(&self) -> u64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn ino(&self) -> u64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn mode(&self) -> u32;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn nlink(&self) -> u64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn uid(&self) -> u32;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn gid(&self) -> u32;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn rdev(&self) -> u64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn size(&self) -> u64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn blksize(&self) -> u64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn blocks(&self) -> u64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn atime(&self) -> i64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn atime_nsec(&self) -> i64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn mtime(&self) -> i64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn mtime_nsec(&self) -> i64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn ctime(&self) -> i64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn ctime_nsec(&self) -> i64;
}

#[stable(feature = "rust1", since = "1.0.0")]
impl MetadataExt for Metadata {
    fn dev(&self) -> u64 {
        self.as_inner().dev()
    }
    fn ino(&self) -> u64 {
        self.as_inner().ino()
    }
    fn mode(&self) -> u32 {
        self.as_inner().mode()
    }
    fn nlink(&self) -> u64 {
        self.as_inner().nlink()
    }
    fn uid(&self) -> u32 {
        0
    }
    fn gid(&self) -> u32 {
        0
    }
    fn rdev(&self) -> u64 {
        0
    }
    fn size(&self) -> u64 {
        self.len()
    }
    fn blksize(&self) -> u64 {
        4096
    }
    fn blocks(&self) -> u64 {
        self.len().div_ceil(512)
    }
    fn atime(&self) -> i64 {
        self.as_inner().atime()
    }
    fn atime_nsec(&self) -> i64 {
        0
    }
    fn mtime(&self) -> i64 {
        self.as_inner().mtime()
    }
    fn mtime_nsec(&self) -> i64 {
        0
    }
    fn ctime(&self) -> i64 {
        self.as_inner().mtime()
    }
    fn ctime_nsec(&self) -> i64 {
        0
    }
}

use crate::io;
use crate::path::Path;

/// Create a new symbolic link on the filesystem.
#[stable(feature = "symlink", since = "1.1.0")]
pub fn symlink<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> io::Result<()> {
    crate::sys::fs::symlink(original.as_ref(), link.as_ref())
}
