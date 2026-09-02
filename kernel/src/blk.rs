//! In-kernel virtio-blk: sector I/O for `/dev/vd*` and filesystem modules.
//!
//! Public surface is `init`, `count`, `capacity_sectors`, `read`, `write`,
//! plus unaligned byte I/O. Transports live under `arch` (PCI legacy I/O on
//! x86, virtio-mmio v2 on AArch64/RISC-V). DMA addresses for descriptors
//! are frame phys / HHDM VA minus `hhdm_offset()`, never `kernel_virt_to_phys`.

pub(crate) mod virtq;

pub const MAX_DISKS: usize = 8;
/// Block ids for `/dev/nvme*`; virtio stays `0..count()`.
pub const NVME_ID_BASE: u32 = 0x100;

pub fn init() {
    crate::arch::virtio_blk_init();
}

/// Number of probed virtio-blk devices.
pub fn count() -> u32 {
    crate::arch::virtio_blk_count()
}

/// Disk size in 512-byte sectors, if the virtio config exposed it.
pub fn capacity_sectors(dev: u32) -> Option<u64> {
    if dev >= NVME_ID_BASE {
        return crate::nvme::capacity_sectors(dev - NVME_ID_BASE);
    }
    crate::arch::virtio_blk_capacity(dev)
}

/// Disk size in bytes (saturating at `u32::MAX` for VFS stat).
pub fn capacity_bytes(dev: u32) -> Option<u64> {
    capacity_sectors(dev).map(|s| s.saturating_mul(SECTOR as u64))
}

/// Read `buf.len()` bytes starting at `lba`. `buf.len()` must be a multiple
/// of 512. Fails if the device was not found (does not panic).
pub fn read(dev: u32, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
    if buf.len() % SECTOR != 0 {
        return Err(());
    }
    if buf.is_empty() {
        return Ok(());
    }
    if dev >= NVME_ID_BASE {
        return crate::nvme::read(dev - NVME_ID_BASE, lba, buf);
    }
    crate::arch::virtio_blk_read(dev, lba, buf)
}

/// Write `buf.len()` bytes starting at `lba`. `buf.len()` must be a multiple
/// of 512.
pub fn write(dev: u32, lba: u64, buf: &[u8]) -> Result<(), ()> {
    if buf.len() % SECTOR != 0 {
        return Err(());
    }
    if buf.is_empty() {
        return Ok(());
    }
    if dev >= NVME_ID_BASE {
        return crate::nvme::write(dev - NVME_ID_BASE, lba, buf);
    }
    crate::arch::virtio_blk_write(dev, lba, buf)
}

/// Byte-granular read at `offset`. Partial sectors are handled internally.
/// Returns bytes copied (0 at/past EOF if capacity is known).
pub fn read_bytes(dev: u32, offset: u64, buf: &mut [u8]) -> Result<usize, ()> {
    if buf.is_empty() {
        return Ok(0);
    }
    let cap = capacity_bytes(dev);
    if let Some(cap) = cap {
        if offset >= cap {
            return Ok(0);
        }
    }
    let want = match cap {
        Some(cap) => buf.len().min((cap - offset) as usize),
        None => buf.len(),
    };
    let mut done = 0usize;
    while done < want {
        let abs = offset + done as u64;
        let lba = abs / SECTOR as u64;
        let off = (abs as usize) % SECTOR;
        let mut sec = [0u8; SECTOR];
        read(dev, lba, &mut sec)?;
        let take = (SECTOR - off).min(want - done);
        buf[done..done + take].copy_from_slice(&sec[off..off + take]);
        done += take;
    }
    Ok(done)
}

/// Byte-granular write at `offset` via read-modify-write of partial sectors.
pub fn write_bytes(dev: u32, offset: u64, buf: &[u8]) -> Result<usize, ()> {
    if buf.is_empty() {
        return Ok(0);
    }
    let cap = capacity_bytes(dev);
    if let Some(cap) = cap {
        if offset >= cap {
            return Err(());
        }
    }
    let want = match cap {
        Some(cap) => buf.len().min((cap - offset) as usize),
        None => buf.len(),
    };
    let mut done = 0usize;
    while done < want {
        let abs = offset + done as u64;
        let lba = abs / SECTOR as u64;
        let off = (abs as usize) % SECTOR;
        let take = (SECTOR - off).min(want - done);
        if off == 0 && take == SECTOR {
            write(dev, lba, &buf[done..done + take])?;
        } else {
            let mut sec = [0u8; SECTOR];
            read(dev, lba, &mut sec)?;
            sec[off..off + take].copy_from_slice(&buf[done..done + take]);
            write(dev, lba, &sec)?;
        }
        done += take;
    }
    Ok(done)
}

/// DMA page layout used by both transports: header, status, one sector.
pub(crate) const DMA_STATUS: usize = 16;
pub(crate) const DMA_DATA: usize = 512;
pub(crate) const SECTOR: usize = 512;
pub(crate) const VIRTIO_BLK_T_IN: u32 = 0;
pub(crate) const VIRTIO_BLK_T_OUT: u32 = 1;

pub fn nvme_read(ctrl: u32, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
    crate::nvme::read(ctrl, lba, buf)
}

pub fn nvme_write(ctrl: u32, lba: u64, buf: &[u8]) -> Result<(), ()> {
    crate::nvme::write(ctrl, lba, buf)
}

pub fn nvme_count() -> u32 {
    crate::nvme::count()
}

pub fn nvme_capacity_bytes(ctrl: u32) -> Option<u64> {
    crate::nvme::capacity_sectors(ctrl).map(|s| s.saturating_mul(SECTOR as u64))
}
