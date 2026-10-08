//! Block devices: the registry filled by driver modules (`virtio_blk`,
//! `nvme`, `usb_storage`) through `KernelApi::blk_register`, the partitions
//! the kernel finds on them, and the sector / byte I/O the VFS (`/dev/…`)
//! and the filesystem modules go through.
//!
//! The kernel has no block driver of its own: a disk appears when a module
//! registers it with a name (`vda`, `nvme0n1`, …) and a [`ModuleBlkOps`]
//! table, and is addressed by the id that returned. `/dev/<name>/` is a
//! directory: `data` is the whole disk, `p<N>` the GPT partition in entry
//! `N` ([`gpt`]), a device of its own (an id like a disk's) whose I/O goes
//! to its range of the disk. What is read is kept in the block cache
//! ([`cache`]), by disk.
//!
//! A disk's partition table is read on the first look at `/dev` or
//! `/proc/partitions` after it registered ([`scan`]), not in
//! [`register`]: the driver registers with its own locks held, and the
//! reads would wait on them.

mod cache;
mod gpt;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use myos_abi::ModuleBlkOps;
use spin::Mutex;

pub use cache::{frames as cache_frames, release as release_cache};

/// Disks and partitions together.
pub const MAX_DEVS: usize = 64;
pub const SECTOR: usize = 512;
const NAME_MAX: usize = 15;
/// UTF-16 units of a GPT partition name.
pub const LABEL_MAX: usize = 36;

/// Where a disk's partition table stands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scan {
    /// Not read yet (a new disk, or a rescan).
    Pending,
    /// Being read by [`scan`].
    Running,
    Done,
}

/// A GPT partition: its entry's number (from 1) and what the entry says.
#[derive(Clone, Copy)]
pub struct Part {
    /// The disk's id.
    pub disk: u32,
    pub number: u32,
    pub first: u64,
    pub sectors: u64,
    pub type_guid: [u8; 16],
    pub uuid: [u8; 16],
    pub label: [u16; LABEL_MAX],
}

#[derive(Clone, Copy)]
enum Kind {
    /// A driver's disk; `serial` tells it from a later disk in the same slot.
    Disk { ops: ModuleBlkOps, ctx: usize, scan: Scan, serial: u64 },
    Part(Part),
}

#[derive(Clone, Copy)]
struct BlkDev {
    /// The disk's name (a partition's too).
    name: [u8; NAME_MAX],
    name_len: u8,
    kind: Kind,
}

impl BlkDev {
    fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("")
    }

    /// The device's path under `/dev`: `sda/data`, `sda/p1`.
    fn path(&self) -> String {
        match self.kind {
            Kind::Disk { .. } => alloc::format!("{}/data", self.name_str()),
            Kind::Part(p) => alloc::format!("{}/p{}", self.name_str(), p.number),
        }
    }

    fn part_of(&self, disk: u32) -> bool {
        matches!(self.kind, Kind::Part(p) if p.disk == disk)
    }
}

static DEVS: Mutex<[Option<BlkDev>; MAX_DEVS]> = Mutex::new([const { None }; MAX_DEVS]);
/// A disk is [`Scan::Pending`].
static PENDING: AtomicBool = AtomicBool::new(false);
/// Disks [`Scan::Running`].
static RUNNING: AtomicU32 = AtomicU32::new(0);
static SERIAL: AtomicU64 = AtomicU64::new(0);

/// Register the disk `/dev/<name>/` backed by `ops` with `ctx`; the device
/// id, or `None` when the table is full, the name is invalid or already
/// taken. Its partitions come with the next [`scan`].
pub fn register(name: &str, ops: ModuleBlkOps, ctx: usize) -> Option<u32> {
    if name.is_empty() || name.len() > NAME_MAX || !name.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let mut devs = DEVS.lock();
    if devs.iter().flatten().any(|d| matches!(d.kind, Kind::Disk { .. }) && d.name_str() == name) {
        return None;
    }
    let slot = devs.iter().position(|d| d.is_none())?;
    let mut d = BlkDev {
        name: [0; NAME_MAX],
        name_len: name.len() as u8,
        kind: Kind::Disk { ops, ctx, scan: Scan::Pending, serial: SERIAL.fetch_add(1, Ordering::Relaxed) },
    };
    d.name[..name.len()].copy_from_slice(name.as_bytes());
    devs[slot] = Some(d);
    PENDING.store(true, Ordering::SeqCst);
    Some(slot as u32)
}

/// The `/dev` paths of disk `dev` and of its partitions.
fn paths_of(dev: u32) -> Vec<String> {
    let devs = DEVS.lock();
    devs.iter()
        .enumerate()
        .filter_map(|(i, d)| d.as_ref().filter(|d| i as u32 == dev || d.part_of(dev)))
        .map(BlkDev::path)
        .collect()
}

/// Take disk `dev` and its partitions out of the table (`/dev/<name>/`
/// disappears, the ids may be reused): `Err(())` while a filesystem is
/// mounted from one of them or an fd is open on one, or when there is no
/// such disk.
pub fn unregister(dev: u32) -> Result<(), ()> {
    if !matches!(get(dev).map(|d| d.kind), Some(Kind::Disk { .. })) {
        return Err(());
    }
    if paths_of(dev).iter().any(|p| crate::fs::blk_in_use(p)) {
        return Err(());
    }
    let mut devs = DEVS.lock();
    for slot in devs.iter_mut() {
        if slot.is_some_and(|d| d.part_of(dev)) {
            *slot = None;
        }
    }
    match devs.get_mut(dev as usize) {
        Some(slot @ Some(_)) => {
            *slot = None;
            drop(devs);
            cache::forget(dev);
            Ok(())
        }
        _ => Err(()),
    }
}

/// Read the partition tables of the disks that need it ([`Scan::Pending`]),
/// and wait for the ones another task is reading. Called before a look at
/// `/dev` or `/proc/partitions`, where the caller may wait on the disk.
pub fn scan() {
    while PENDING.swap(false, Ordering::SeqCst) {
        loop {
            let claimed = {
                let mut devs = DEVS.lock();
                devs.iter_mut().enumerate().find_map(|(i, d)| match d {
                    Some(BlkDev { kind: Kind::Disk { scan, serial, .. }, .. }) if *scan == Scan::Pending => {
                        *scan = Scan::Running;
                        RUNNING.fetch_add(1, Ordering::SeqCst);
                        Some((i as u32, *serial))
                    }
                    _ => None,
                })
            };
            let Some((disk, serial)) = claimed else {
                break;
            };
            let entries = capacity_sectors(disk).map(|n| gpt::read(disk, n)).unwrap_or_default();
            add_parts(disk, serial, &entries);
            RUNNING.fetch_sub(1, Ordering::SeqCst);
        }
    }
    while RUNNING.load(Ordering::SeqCst) != 0 {
        crate::task::yield_now();
    }
}

/// The partitions [`scan`] read off disk `disk`, unless it went away
/// meanwhile (`serial` names the registration that was read).
fn add_parts(disk: u32, serial: u64, entries: &[gpt::Entry]) {
    let mut devs = DEVS.lock();
    let Some(Some(d)) = devs.get_mut(disk as usize) else {
        return;
    };
    match &mut d.kind {
        Kind::Disk { scan, serial: s, .. } if *scan == Scan::Running && *s == serial => *scan = Scan::Done,
        _ => return,
    }
    let (name, name_len) = (d.name, d.name_len);
    for e in entries {
        let Some(slot) = devs.iter().position(|d| d.is_none()) else {
            crate::console::write_str("blk: no room for more partitions\n");
            return;
        };
        let part = Part {
            disk,
            number: e.number,
            first: e.first,
            sectors: e.last - e.first + 1,
            type_guid: e.type_guid,
            uuid: e.uuid,
            label: e.label,
        };
        devs[slot] = Some(BlkDev { name, name_len, kind: Kind::Part(part) });
    }
}

/// Read the partition tables again (after `/proc/pci` rescan): of every
/// disk none of whose partitions is mounted or open, the partitions go and
/// come back from the next [`scan`] as the disk has them now.
pub fn rescan() {
    let disks: Vec<u32> = {
        let devs = DEVS.lock();
        (0..MAX_DEVS as u32)
            .filter(|&i| matches!(devs[i as usize], Some(BlkDev { kind: Kind::Disk { scan: Scan::Done, .. }, .. })))
            .collect()
    };
    for disk in disks {
        let parts: Vec<String> = paths_of(disk).into_iter().filter(|p| !p.ends_with("/data")).collect();
        if parts.iter().any(|p| crate::fs::blk_in_use(p)) {
            continue;
        }
        let mut devs = DEVS.lock();
        for slot in devs.iter_mut() {
            if slot.is_some_and(|d| d.part_of(disk)) {
                *slot = None;
            }
        }
        if let Some(Some(BlkDev { kind: Kind::Disk { scan, .. }, .. })) = devs.get_mut(disk as usize) {
            *scan = Scan::Pending;
            PENDING.store(true, Ordering::SeqCst);
        }
    }
}

/// Number of device ids in use (`0..count()` may have holes).
pub fn count() -> u32 {
    let devs = DEVS.lock();
    devs.iter().rposition(|d| d.is_some()).map_or(0, |i| i as u32 + 1)
}

/// The disks' names, by id.
pub fn disks() -> Vec<String> {
    let devs = DEVS.lock();
    devs.iter()
        .flatten()
        .filter(|d| matches!(d.kind, Kind::Disk { .. }))
        .map(|d| String::from(d.name_str()))
        .collect()
}

/// The disk registered as `/dev/<name>/`.
pub fn disk_by_name(name: &str) -> Option<u32> {
    let devs = DEVS.lock();
    devs.iter()
        .position(|d| d.as_ref().is_some_and(|d| matches!(d.kind, Kind::Disk { .. }) && d.name_str() == name))
        .map(|i| i as u32)
}

/// The partitions of disk `disk`, by number.
pub fn parts(disk: u32) -> Vec<Part> {
    let devs = DEVS.lock();
    let mut parts: Vec<Part> = devs
        .iter()
        .flatten()
        .filter_map(|d| match d.kind {
            Kind::Part(p) if p.disk == disk => Some(p),
            _ => None,
        })
        .collect();
    parts.sort_by_key(|p| p.number);
    parts
}

/// The device at `/dev/<path>`: `sda/data` (the disk) or `sda/p1`.
pub fn by_path(path: &str) -> Option<u32> {
    let (disk, member) = path.split_once('/')?;
    let disk = disk_by_name(disk)?;
    if member == "data" {
        return Some(disk);
    }
    let number: u32 = member.strip_prefix('p')?.parse().ok()?;
    if member != alloc::format!("p{number}") {
        return None;
    }
    let devs = DEVS.lock();
    devs.iter()
        .position(|d| d.is_some_and(|d| matches!(d.kind, Kind::Part(p) if p.disk == disk && p.number == number)))
        .map(|i| i as u32)
}

fn get(dev: u32) -> Option<BlkDev> {
    *DEVS.lock().get(dev as usize)?
}

/// The disk under `dev` and where on it `n` sectors from `lba` of `dev`
/// are: `dev` itself, or a partition's range (`None` past its end).
fn on_disk(dev: u32, lba: u64, n: u64) -> Option<(u32, ModuleBlkOps, usize, u64)> {
    match get(dev)?.kind {
        Kind::Disk { ops, ctx, .. } => Some((dev, ops, ctx, lba)),
        Kind::Part(p) => {
            if lba.checked_add(n)? > p.sectors {
                return None;
            }
            match get(p.disk)?.kind {
                Kind::Disk { ops, ctx, .. } => Some((p.disk, ops, ctx, p.first + lba)),
                Kind::Part(_) => None,
            }
        }
    }
}

/// Device size in 512-byte sectors: a partition's, or the disk's if the
/// driver knows it.
pub fn capacity_sectors(dev: u32) -> Option<u64> {
    let n = match get(dev)?.kind {
        Kind::Disk { ops, ctx, .. } => unsafe { (ops.capacity_sectors)(ctx) },
        Kind::Part(p) => p.sectors,
    };
    (n != 0).then_some(n)
}

/// Disk size in bytes.
pub fn capacity_bytes(dev: u32) -> Option<u64> {
    capacity_sectors(dev).map(|s| s.saturating_mul(SECTOR as u64))
}

/// Read `buf.len()` bytes starting at `lba`, through the block cache.
/// `buf.len()` must be a multiple of 512. Fails if the device does not
/// exist, or the range goes past a partition's end (does not panic).
pub fn read(dev: u32, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
    if buf.len() % SECTOR != 0 {
        return Err(());
    }
    if buf.is_empty() {
        return Ok(());
    }
    let (disk, ops, ctx, lba) = on_disk(dev, lba, (buf.len() / SECTOR) as u64).ok_or(())?;
    let device = |lba: u64, buf: &mut [u8]| {
        let rc = unsafe { (ops.read)(ctx, lba, buf.as_mut_ptr(), buf.len()) };
        if rc == 0 { Ok(()) } else { Err(()) }
    };
    // The cache keeps whole pages of the disk: it needs to know its end.
    match capacity_sectors(disk) {
        Some(sectors) => cache::read(disk, lba, buf, sectors, device),
        None => device(lba, buf),
    }
}

/// Write `buf.len()` bytes starting at `lba`. `buf.len()` must be a multiple
/// of 512; a partition refuses a range past its end.
pub fn write(dev: u32, lba: u64, buf: &[u8]) -> Result<(), ()> {
    if buf.len() % SECTOR != 0 {
        return Err(());
    }
    if buf.is_empty() {
        return Ok(());
    }
    let (disk, ops, ctx, lba) = on_disk(dev, lba, (buf.len() / SECTOR) as u64).ok_or(())?;
    let rc = unsafe { (ops.write)(ctx, lba, buf.as_ptr(), buf.len()) };
    cache::wrote(disk, lba, buf);
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
        // Whole sectors go to the driver in one request.
        let whole = whole_sectors(off, want - done);
        if whole > 0 {
            read(dev, lba, &mut buf[done..done + whole])?;
            done += whole;
            continue;
        }
        let mut sec = [0u8; SECTOR];
        read(dev, lba, &mut sec)?;
        let take = (SECTOR - off).min(want - done);
        buf[done..done + take].copy_from_slice(&sec[off..off + take]);
        done += take;
    }
    Ok(done)
}

/// Most sectors one driver request carries.
const MAX_REQUEST: usize = 64 * 1024;

/// How many bytes from sector offset `off` on, with `left` to go, are whole
/// sectors to move in one request (0 when `off` is inside a sector or less
/// than a sector is left).
fn whole_sectors(off: usize, left: usize) -> usize {
    if off != 0 {
        return 0;
    }
    (left / SECTOR * SECTOR).min(MAX_REQUEST)
}

/// Byte-granular write at `offset`: whole sectors in one request, partial
/// ones by read-modify-write.
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
        let whole = whole_sectors(off, want - done);
        if whole > 0 {
            write(dev, lba, &buf[done..done + whole])?;
            done += whole;
            continue;
        }
        let take = (SECTOR - off).min(want - done);
        let mut sec = [0u8; SECTOR];
        read(dev, lba, &mut sec)?;
        sec[off..off + take].copy_from_slice(&buf[done..done + take]);
        write(dev, lba, &sec)?;
        done += take;
    }
    Ok(done)
}

/// The text of `/proc/partitions`: a line per partition, `sda/p1`, its
/// first sector, its size in bytes, its type and unique GUIDs and its name
/// in quotes (a `"`, `\` or control character in it as `?`).
pub fn partitions_text() -> String {
    scan();
    let mut out = String::new();
    for name in disks() {
        let Some(disk) = disk_by_name(&name) else {
            continue;
        };
        for p in parts(disk) {
            let label: String = char::decode_utf16(p.label.iter().copied().take_while(|&u| u != 0))
                .map(|c| match c {
                    Ok(c) if c != '"' && c != '\\' && !c.is_control() => c,
                    _ => '?',
                })
                .collect();
            out += &alloc::format!(
                "{name}/p{} {} {} {} {} \"{label}\"\n",
                p.number,
                p.first,
                p.sectors * SECTOR as u64,
                gpt::guid_text(&p.type_guid),
                gpt::guid_text(&p.uuid)
            );
        }
    }
    out
}
