//! Minimal newc (cpio) parser for the initramfs Limine module.
//!
//! The archive layout mirrors the VFS tree: `bin/<category>/<name>` entries are
//! registered into the matching `/bin/…` mount (binfs, sbasefs, ubasefs,
//! coreutilsfs, tccfs) and `lib/…` entries into libfs. Longest-prefix routing
//! preserves each mount's capacity and the `/bin/<category>/<name>` layout the
//! shell's `_PATH_DEFPATH` expects.
//!
//! The module buffer is Limine-mapped for the kernel's lifetime, so each entry's
//! bytes are `'static` and can be handed to `register()` without copying.

use crate::fs::{binfs, coreutilsfs, libfs, sbasefs, tccfs, ubasefs};
use alloc::vec::Vec;

fn hex(s: &[u8]) -> usize {
    let mut v = 0usize;
    for &b in s {
        v = (v << 4)
            | match b {
                b'0'..=b'9' => (b - b'0') as usize,
                b'a'..=b'f' => (b - b'a' + 10) as usize,
                _ => return 0,
            };
    }
    v
}

fn pad4(n: usize) -> usize {
    (n + 3) & !3
}

/// Parse a newc archive and register every file into the matching mount.
/// Handles hardlinked multicall aliases (same ino, nlink > 1, zero-length
/// data) by reusing the first occurrence's bytes. Returns the number of
/// entries routed.
pub fn parse(data: &'static [u8]) -> usize {
    let mut off = 0usize;
    let mut count = 0usize;
    // ino -> bytes for hardlink aliases (the multicall coreutils ELF).
    let mut links: Vec<(usize, &'static [u8])> = Vec::new();
    while off + 110 <= data.len() {
        let hdr = &data[off..off + 110];
        if &hdr[0..6] != b"070701" {
            break;
        }
        let ino = hex(&hdr[6..14]);
        let nlink = hex(&hdr[38..46]);
        let filesize = hex(&hdr[54..62]);
        let namesize = hex(&hdr[94..102]);
        let name_off = off + 110;
        let name_end = name_off + namesize;
        if name_end > data.len() {
            break;
        }
        let raw = &data[name_off..name_end];
        let name_len = match raw.iter().position(|&b| b == 0) {
            Some(p) => p,
            None => raw.len(),
        };
        let name = core::str::from_utf8(&raw[..name_len]).unwrap_or("");
        let data_off = pad4(name_end);
        let data_end = data_off + filesize;
        if data_end > data.len() {
            break;
        }
        if name == "TRAILER!!!" || name.is_empty() {
            break;
        }
        if filesize > 0 {
            let bytes: &'static [u8] = &data[data_off..data_end];
            if nlink > 1 {
                links.push((ino, bytes));
            }
            route(name, bytes);
            count += 1;
        } else if nlink > 1 {
            // Hardlink alias: reuse the bytes registered under this ino.
            if let Some((_, bytes)) = links.iter().find(|(i, _)| *i == ino) {
                route(name, bytes);
                count += 1;
            }
        }
        off = pad4(data_end);
    }
    count
}

/// Route one archive entry to the mount that serves its path.
fn route(name: &str, bytes: &'static [u8]) {
    if let Some(rest) = name.strip_prefix("bin/sbase/") {
        let _ = sbasefs::register(rest, bytes);
    } else if let Some(rest) = name.strip_prefix("bin/ubase/") {
        let _ = ubasefs::register(rest, bytes);
    } else if let Some(rest) = name.strip_prefix("bin/coreutils/") {
        let _ = coreutilsfs::register(rest, bytes);
    } else if let Some(rest) = name.strip_prefix("bin/tcc/") {
        let _ = tccfs::register(rest, bytes);
    } else if let Some(rest) = name.strip_prefix("lib/") {
        let _ = libfs::register(rest, bytes);
    } else if let Some(rest) = name.strip_prefix("bin/") {
        let _ = binfs::register(rest, bytes);
    }
}