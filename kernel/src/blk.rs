//! Block devices: the registry filled by driver modules (`virtio_blk`,
//! `nvme`) through `KernelApi::blk_register`, and the sector / byte I/O the
//! VFS (`/dev/<name>`) and the filesystem modules go through.
//!
//! The kernel has no block driver of its own: a device appears when a
//! module registers it with a name (`vda`, `nvme0n1`, …) and a
//! [`ModuleBlkOps`] table, and is addressed by the id that returned.

use myos_abi::ModuleBlkOps;
use spin::Mutex;

pub const MAX_DISKS: usize = 16;
pub const SECTOR: usize = 512;
const NAME_MAX: usize = 15;

#[derive(Clone, Copy)]
struct BlkDev {
    name: [u8; NAME_MAX],
    name_len: u8,
    ops: ModuleBlkOps,
    ctx: usize,
}

static DEVS: Mutex<[Option<BlkDev>; MAX_DISKS]> = Mutex::new([const { None }; MAX_DISKS]);

/// Register `/dev/<name>` backed by `ops` with `ctx`; the device id, or
/// `None` when the table is full, the name is invalid or already taken.
pub fn register(name: &str, ops: ModuleBlkOps, ctx: usize) -> Option<u32> {
    if name.is_empty() || name.len() > NAME_MAX || !name.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let mut devs = DEVS.lock();
    if devs.iter().flatten().any(|d| d.name_str() == name) {
        return None;
    }
    let slot = devs.iter().position(|d| d.is_none())?;
    let mut d = BlkDev {
        name: [0; NAME_MAX],
        name_len: name.len() as u8,
        ops,
        ctx,
    };
    d.name[..name.len()].copy_from_slice(name.as_bytes());
    devs[slot] = Some(d);
    Some(slot as u32)
}

impl BlkDev {
    fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("")
    }
}

/// Number of device ids in use (`0..count()` may have holes).
pub fn count() -> u32 {
    let devs = DEVS.lock();
    devs.iter().rposition(|d| d.is_some()).map_or(0, |i| i as u32 + 1)
}

/// The device's `/dev` name, copied into `out`; its length.
pub fn name(dev: u32, out: &mut [u8]) -> Option<usize> {
    let devs = DEVS.lock();
    let d = devs.get(dev as usize)?.as_ref()?;
    let n = (d.name_len as usize).min(out.len());
    out[..n].copy_from_slice(&d.name[..n]);
    Some(n)
}

/// The device registered as `/dev/<name>`.
pub fn by_name(name: &str) -> Option<u32> {
    let devs = DEVS.lock();
    devs.iter()
        .position(|d| d.as_ref().is_some_and(|d| d.name_str() == name))
        .map(|i| i as u32)
}

fn get(dev: u32) -> Option<BlkDev> {
    *DEVS.lock().get(dev as usize)?
}

/// Disk size in 512-byte sectors, if the driver knows it.
pub fn capacity_sectors(dev: u32) -> Option<u64> {
    let d = get(dev)?;
    let n = unsafe { (d.ops.capacity_sectors)(d.ctx) };
    (n != 0).then_some(n)
}

/// Disk size in bytes.
pub fn capacity_bytes(dev: u32) -> Option<u64> {
    capacity_sectors(dev).map(|s| s.saturating_mul(SECTOR as u64))
}

/// Read `buf.len()` bytes starting at `lba`. `buf.len()` must be a multiple
/// of 512. Fails if the device does not exist (does not panic).
pub fn read(dev: u32, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
    if buf.len() % SECTOR != 0 {
        return Err(());
    }
    if buf.is_empty() {
        return Ok(());
    }
    let d = get(dev).ok_or(())?;
    let rc = unsafe { (d.ops.read)(d.ctx, lba, buf.as_mut_ptr(), buf.len()) };
    if rc == 0 { Ok(()) } else { Err(()) }
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
    let d = get(dev).ok_or(())?;
    let rc = unsafe { (d.ops.write)(d.ctx, lba, buf.as_ptr(), buf.len()) };
    if rc == 0 { Ok(()) } else { Err(()) }
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
