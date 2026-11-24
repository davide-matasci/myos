//! mkinitramfs SRC ARCH BASE OUT
//!
//! The initramfs of a system that rebuilt part of itself: the archive BASE
//! (the running system's, `/proc/boot/initramfs`) with what a build of the
//! tree SRC on myos made (`scripts/self-host.sh`: every module and every
//! `no_std` program, under `SRC/target`) in place of the base's files of
//! those names, written to OUT as the kernel takes a boot archive
//! (`kernel/src/fs/zstd.rs`: one zstd frame with its size and a checksum).
//! What myos does not build yet (the C programs, the `std` ones) is what
//! the system runs, as the base has it.
//!
//! A program of the system it packs: built by its own cargo for the arch
//! it runs on (it builds on a Linux host too, where the launcher packs the
//! images, `src/initramfs.rs`, from which this takes what it writes).

#![allow(dead_code)]

mod boot_modules {
    include!("../../../src/boot_modules.rs");
}
mod initramfs {
    include!("../../../src/initramfs.rs");
}
/// What the packer names of the launcher's disk code (`src/limine_image.rs`).
mod limine_image {
    pub use crate::boot_modules::{all_modules, boot_list};
    pub struct DiskFile {
        pub path: String,
        pub data: Vec<u8>,
    }
}
mod ports {
    include!("../../../src/ports.rs");
}
mod release {
    include!("../../../src/release.rs");
}

use std::io::Read;
use std::path::Path;

/// A zstd frame's first four bytes.
const MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 {
        eprintln!("usage: mkinitramfs SRC ARCH BASE OUT");
        std::process::exit(2);
    }
    let (src, arch, base, out) = (Path::new(&args[1]), &args[2], &args[3], &args[4]);
    let base = unpack(&std::fs::read(base).unwrap_or_else(|e| die(&format!("read {base}: {e}"))));
    let built = initramfs::built_on_myos(src, arch);
    let count = built.len();
    let (raw, replaced) = initramfs::overlay(&base, built);
    // Every file built here is one the system has: a name the base lacks
    // is a file the packer and this build do not agree on.
    if replaced != count {
        die(&format!("{count} files built, {replaced} of them in the base archive"));
    }
    let packed = pack(&raw);
    std::fs::write(out, &packed).unwrap_or_else(|e| die(&format!("write {out}: {e}")));
    println!("mkinitramfs: {out}: {count} files built here in an archive of {} MiB ({} MiB packed)", raw.len() >> 20, packed.len() >> 20);
}

fn die(why: &str) -> ! {
    eprintln!("mkinitramfs: {why}");
    std::process::exit(1);
}

/// The archive in `data`: decoded when it is a zstd frame, as it is
/// otherwise (the kernel takes a raw one too).
fn unpack(data: &[u8]) -> Vec<u8> {
    if !data.starts_with(&MAGIC) {
        return data.to_vec();
    }
    let mut raw = Vec::new();
    ruzstd::decoding::StreamingDecoder::new(data)
        .unwrap_or_else(|e| die(&format!("the base archive's zstd frame: {e}")))
        .read_to_end(&mut raw)
        .unwrap_or_else(|e| die(&format!("the base archive's zstd frame: {e}")));
    raw
}

/// `raw` as one zstd frame with its size in the header and a checksum
/// after the last block, which the kernel asks of a boot archive. The
/// encoder writes the checksum and no size: the frame header gets it here.
///
/// ```text
/// magic (4) | descriptor (1) | window (1) | size (8, added) | blocks | checksum (4)
/// ```
///
/// The descriptor's two high bits say how wide the size field is (3: eight
/// bytes); the field goes after the window descriptor, there being no
/// dictionary id.
fn pack(raw: &[u8]) -> Vec<u8> {
    let mut frame = ruzstd::encoding::compress_to_vec(raw, ruzstd::encoding::CompressionLevel::Fastest);
    // As the encoder writes it: no size, a window descriptor (not a single
    // segment), a checksum, no dictionary id.
    assert!(frame.starts_with(&MAGIC) && frame[4] == 0x04, "an unexpected zstd frame header");
    frame[4] |= 0xc0;
    frame.splice(6..6, (raw.len() as u64).to_le_bytes());
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    use initramfs::Entry;

    fn entry(name: &str, data: &[u8], ino: u64, nlink: u32) -> Entry {
        Entry { name: name.into(), data: data.to_vec(), ino, nlink, mode: 0o100755 }
    }

    /// A frame as the kernel checks one: its size in the header, a
    /// checksum that matches, the bytes back.
    #[test]
    fn packed_frame_has_its_size_and_checksum() {
        let raw: Vec<u8> = (0..300_000u32).flat_map(|i| (i / 7).to_le_bytes()).collect();
        let frame = pack(&raw);
        let mut input = &frame[..];
        let mut dec = ruzstd::decoding::FrameDecoder::new();
        dec.init(&mut input).unwrap();
        assert_eq!(dec.content_size(), raw.len() as u64);
        assert!(frame[4] & 0x04 != 0);
        dec.decode_blocks(&mut input, ruzstd::decoding::BlockDecodingStrategy::All).unwrap();
        let mut back = Vec::new();
        dec.read_to_end(&mut back).unwrap();
        assert!(back == raw);
        assert_eq!(dec.get_checksum_from_data(), dec.get_calculated_checksum());
        assert!(unpack(&frame) == raw);
    }

    /// A built file takes the place of the base's; the rest stays, and a
    /// hard-link group keeps its bytes when the name that carried them in
    /// the archive is the one replaced.
    #[test]
    fn overlay_replaces_by_name() {
        let base = initramfs::overlay(
            &initramfs::overlay(&[], vec![]).0,
            vec![
                entry("bin/a", b"link", 1, 2),
                entry("bin/b", b"", 1, 2),
                entry("lib/modules/hello", b"old", 2, 1),
                entry("etc/rc", b"rc", 3, 1),
            ],
        )
        .0;
        let (raw, replaced) =
            initramfs::overlay(&base, vec![entry("lib/modules/hello", b"new", 1, 1), entry("bin/a", b"alone", 2, 1)]);
        assert_eq!(replaced, 2);
        let got = initramfs::overlay(&raw, vec![]).0;
        // Read back and written again: the same archive but for its inode numbers.
        assert_eq!(got.len(), raw.len());
        let text = String::from_utf8_lossy(&raw);
        for want in ["new", "alone", "link", "rc"] {
            assert!(text.contains(want), "{want} is missing");
        }
        assert!(!text.contains("old"));
    }
}
