//! Read-only flat namespaces for cross-built ELFs: one directory level, names
//! without `/`, contents borrowed from `'static` image bytes.
//!
//! Instances (mounted in [`crate::fs::init`]):
//! - [`SBASE`] at `/bin/sbase` — suckless sbase,
//! - [`UBASE`] at `/bin/ubase` — suckless ubase (getty/login are embedded in
//!   the kernel by `kernel/build.rs`; the rest arrive via the initramfs),
//! - [`TCC`] at `/bin/tcc` — TinyCC,
//! - [`COREUTILS`] at `/bin/coreutils` — uutils multicall + ripgrep; all
//!   utilities share one ELF and the exec basename selects the utility.
//!
//! Everything except the embedded ubase ELFs is registered by
//! [`crate::fs::cpio`] when the initramfs module is unpacked at boot.

use spin::Mutex;

use crate::fs::StatInfo;

const NAME_CAP: usize = 32;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;

pub static SBASE: FlatFs<128> = FlatFs::new();
pub static UBASE: FlatFs<16> = FlatFs::new();
pub static TCC: FlatFs<16> = FlatFs::new();
pub static COREUTILS: FlatFs<128> = FlatFs::new();

#[derive(Clone, Copy)]
struct Slot {
    name: [u8; NAME_CAP],
    len: usize,
    data: &'static [u8],
}

impl Slot {
    fn is(&self, name: &str) -> bool {
        &self.name[..self.len] == name.as_bytes()
    }
}

pub struct FlatFs<const N: usize> {
    files: Mutex<[Option<Slot>; N]>,
}

impl<const N: usize> FlatFs<N> {
    pub const fn new() -> Self {
        Self {
            files: Mutex::new([None; N]),
        }
    }

    /// Add or replace `name`. Names longer than `NAME_CAP` are truncated.
    pub fn register(&self, name: &str, bytes: &'static [u8]) -> bool {
        if name.is_empty() || name.contains('/') {
            return false;
        }
        let len = name.len().min(NAME_CAP);
        let mut n = [0u8; NAME_CAP];
        n[..len].copy_from_slice(&name.as_bytes()[..len]);

        let mut files = self.files.lock();
        if let Some(s) = files
            .iter_mut()
            .flatten()
            .find(|s| s.len == len && s.name[..len] == n[..len])
        {
            s.data = bytes;
            return true;
        }
        match files.iter_mut().find(|s| s.is_none()) {
            Some(free) => {
                *free = Some(Slot {
                    name: n,
                    len,
                    data: bytes,
                });
                true
            }
            None => false,
        }
    }

    pub fn lookup(&self, name: &str) -> Option<&'static [u8]> {
        if name.is_empty() || name == "." || name == ".." {
            return None;
        }
        let files = self.files.lock();
        files.iter().flatten().find(|s| s.is(name)).map(|s| s.data)
    }

    pub fn read(&self, name: &str, pos: usize, out: &mut [u8]) -> usize {
        let Some(data) = self.lookup(name) else {
            return 0;
        };
        let n = out.len().min(data.len().saturating_sub(pos));
        if n != 0 {
            out[..n].copy_from_slice(&data[pos..pos + n]);
        }
        n
    }

    /// Newline-separated names; only the mount root is listable.
    pub fn listdir_at(&self, rel: &str, buf: &mut [u8]) -> usize {
        if !rel.is_empty() && rel != "." {
            return 0;
        }
        let files = self.files.lock();
        let mut n = 0;
        for slot in files.iter().flatten() {
            let name = &slot.name[..slot.len];
            let need = name.len() + 1;
            if n + need > buf.len() {
                break;
            }
            buf[n..n + name.len()].copy_from_slice(name);
            n += name.len();
            buf[n] = b'\n';
            n += 1;
        }
        n
    }

    pub fn stat(&self, name: &str) -> Option<StatInfo> {
        if name.is_empty() || name == "." || name == ".." {
            return Some(StatInfo {
                mode: S_IFDIR | 0o755,
                size: 0,
                ino: 1,
                nlink: 2,
                dev: 0,
            });
        }
        let files = self.files.lock();
        files.iter().flatten().find(|s| s.is(name)).map(|s| StatInfo {
            mode: S_IFREG | 0o555,
            size: s.data.len() as u32,
            ino: crate::fs::vfs::data_ino(s.data),
            nlink: 1,
            dev: 0,
        })
    }
}

/// Register the ubase ELFs that `kernel/build.rs` embeds in the kernel.
pub fn init_embedded() {
    ubase_embed::register_all();
}

// The generated `register_all` calls `super::register(name, bytes)`.
fn register(name: &str, bytes: &'static [u8]) -> bool {
    UBASE.register(name, bytes)
}

mod ubase_embed {
    include!(concat!(env!("OUT_DIR"), "/ubase_embed.rs"));
}
