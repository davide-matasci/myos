//! The boot archives compressed (the initramfs and the tests module:
//! `src/initramfs.rs` writes each as one zstd frame, issue #621): Limine
//! loads them as they are, and they are decoded here once, at boot, into a
//! run of frames of their own that the kernel keeps for its life. The tree
//! serves their files by reference from there (`fs::cpio`), as it served
//! them from Limine's mapping when they were raw; a raw archive still is.
//!
//! The decoder is the `ruzstd` crate (MIT), its `hash` feature checking the
//! frame's checksum: a damaged archive stops the boot with a line saying
//! so, not a tree with a corrupt file in it.

use ruzstd::decoding::{BlockDecodingStrategy, FrameDecoder};
use ruzstd::io::Read;

/// A zstd frame's first four bytes.
const MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

/// The archive `data` as the tree reads it: decoded when it is a zstd
/// frame, `data` itself otherwise. `Err` says what was wrong with a frame.
pub fn unpack(data: &'static [u8]) -> Result<&'static [u8], &'static str> {
    if !data.starts_with(&MAGIC) {
        return Ok(data);
    }
    let mut input = data;
    let mut frame = FrameDecoder::new();
    frame.init(&mut input).map_err(|_| "a bad zstd frame header")?;
    // The packer writes the size in the header and a checksum after the
    // last block; a frame without either is not one of ours.
    let size = frame.content_size() as usize;
    if size == 0 || !frame_has_checksum(data) {
        return Err("a zstd frame without its size or checksum");
    }
    let pages = size.div_ceil(crate::mm::PAGE as usize);
    let phys = crate::mm::alloc_contiguous_frames(pages).ok_or("no room for the decoded archive")?;
    // Frames from the bump cursor are never handed out again: the slice
    // lives as long as the kernel, as Limine's mapping did.
    let out: &'static mut [u8] = unsafe {
        core::slice::from_raw_parts_mut((phys + crate::limine_boot::hhdm_offset()) as *mut u8, size)
    };
    let mut done = 0;
    while !frame.is_finished() || frame.can_collect() > 0 {
        frame
            .decode_blocks(&mut input, BlockDecodingStrategy::UptoBytes(1 << 20))
            .map_err(|_| "a damaged zstd block")?;
        done += frame.read(&mut out[done..]).map_err(|_| "a zstd frame larger than its size")?;
    }
    if done != size {
        return Err("a zstd frame shorter than its size");
    }
    if frame.get_checksum_from_data() != frame.get_calculated_checksum() {
        return Err("a zstd frame whose checksum does not match");
    }
    Ok(out)
}

/// Whether the frame header says a checksum follows the last block
/// (`Content_Checksum_flag`, bit 2 of the descriptor after the magic).
fn frame_has_checksum(data: &[u8]) -> bool {
    data.get(4).is_some_and(|d| d & 0x04 != 0)
}
