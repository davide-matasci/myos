//! In-kernel virtio-blk: sector reads for the FAT16 module.
//!
//! Public surface is `init` + `read`. Transports live under `arch`
//! (PCI legacy I/O on x86, virtio-mmio v2 on AArch64). DMA addresses
//! for descriptors are frame phys / HHDM VA minus `hhdm_offset()`, never
//! `kernel_virt_to_phys`.

pub(crate) mod virtq;

pub fn init() {
    crate::arch::virtio_blk_init();
}

/// Read `buf.len()` bytes starting at `lba`. `buf.len()` must be a multiple
/// of 512. Fails if no virtio-blk device was found (does not panic).
pub fn read(lba: u64, buf: &mut [u8]) -> Result<(), ()> {
    if buf.len() % 512 != 0 {
        return Err(());
    }
    if buf.is_empty() {
        return Ok(());
    }
    crate::arch::virtio_blk_read(lba, buf)
}

/// DMA page layout used by both transports: header, status, one sector.
pub(crate) const DMA_STATUS: usize = 16;
pub(crate) const DMA_DATA: usize = 512;
pub(crate) const SECTOR: usize = 512;
pub(crate) const VIRTIO_BLK_T_IN: u32 = 0;
