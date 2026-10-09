//! Formatting: a fresh, empty FAT32 volume over a whole sector device, by
//! fstool's formatter (`Fat32::format`: its geometry, both FATs, the FSInfo
//! sector and the backup boot sector), which wants a byte stream: [`Stream`]
//! is one over the sectors. The boot disk's ESP is made with it on the host
//! (`src/limine_disk.rs`) and by `mkfs.fat` in myos.

use alloc::vec;

use fstool::block::BlockDevice;
use fstool::fs::fat::{Fat32, FatFormatOpts, FatKind};
use fstool::io::{self, Read, Seek, SeekFrom, Write};

use crate::{Error, Result, SectorDriver};

/// Format all of `dev` as an empty FAT32 volume labelled `label` (11
/// bytes, space padded) with serial number `id`.
pub fn format<D: SectorDriver + Send>(dev: &mut D, label: &[u8; 11], id: u32) -> Result<()> {
    let ss = u64::from(dev.sector_size());
    let size = dev.sector_count() * ss;
    let opts = FatFormatOpts {
        kind: FatKind::Fat32,
        total_sectors: u32::try_from(size / 512).map_err(|_| Error::TooBig)?,
        volume_id: id,
        volume_label: *label,
        root_entries: None,
    };
    let mut stream = Stream { dev, pos: 0, size, ss };
    Fat32::format(&mut stream, &opts).map_err(|_| Error::Io)?;
    stream.dev.flush().map_err(|_| Error::Io)
}

/// The sectors of `dev` as bytes from 0 to `size`: whole sectors go to the
/// device as they are, a part of one is read, changed and written back.
struct Stream<'a, D> {
    dev: &'a mut D,
    pos: u64,
    size: u64,
    ss: u64,
}

fn io_err() -> io::Error {
    io::Error::other("sector device")
}

impl<D: SectorDriver> Stream<'_, D> {
    /// The sector holding `pos`, and where `pos` is in it.
    fn at(&self) -> (u64, usize) {
        (self.pos / self.ss, (self.pos % self.ss) as usize)
    }
}

impl<D: SectorDriver> Read for Stream<'_, D> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.size.saturating_sub(self.pos);
        let want = (buf.len() as u64).min(left) as usize;
        if want == 0 {
            return Ok(0);
        }
        let (lba, off) = self.at();
        let ss = self.ss as usize;
        let n = if off == 0 && want >= ss {
            let n = want / ss * ss;
            self.dev.read_sectors(lba, &mut buf[..n]).map_err(|_| io_err())?;
            n
        } else {
            let mut sec = vec![0u8; ss];
            self.dev.read_sectors(lba, &mut sec).map_err(|_| io_err())?;
            let n = want.min(ss - off);
            buf[..n].copy_from_slice(&sec[off..off + n]);
            n
        };
        self.pos += n as u64;
        Ok(n)
    }
}

impl<D: SectorDriver> Write for Stream<'_, D> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let left = self.size.saturating_sub(self.pos);
        let want = (buf.len() as u64).min(left) as usize;
        if want == 0 {
            return if buf.is_empty() { Ok(0) } else { Err(io_err()) };
        }
        let (lba, off) = self.at();
        let ss = self.ss as usize;
        let n = if off == 0 && want >= ss {
            let n = want / ss * ss;
            self.dev.write_sectors(lba, &buf[..n]).map_err(|_| io_err())?;
            n
        } else {
            let mut sec = vec![0u8; ss];
            self.dev.read_sectors(lba, &mut sec).map_err(|_| io_err())?;
            let n = want.min(ss - off);
            sec[off..off + n].copy_from_slice(&buf[..n]);
            self.dev.write_sectors(lba, &sec).map_err(|_| io_err())?;
            n
        };
        self.pos += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.dev.flush().map_err(|_| io_err())
    }
}

impl<D: SectorDriver> Seek for Stream<'_, D> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let to = match pos {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(d) => self.size.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = to.ok_or_else(io_err)?;
        Ok(self.pos)
    }
}

impl<D: SectorDriver + Send> BlockDevice for Stream<'_, D> {
    fn block_size(&self) -> u32 {
        self.ss as u32
    }

    fn total_size(&self) -> u64 {
        self.size
    }

    fn sync(&mut self) -> fstool::Result<()> {
        self.dev.flush().map_err(|_| fstool::Error::Io(io_err()))
    }
}
