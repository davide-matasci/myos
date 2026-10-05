//! The block cache: recently read 4 KiB chunks of the block devices, kept
//! in page frames, so data read again (a toolchain's libraries, mapped by
//! every process that runs it) comes from memory instead of the disk.
//!
//! Every read and write of [`super::read`] / [`super::write`] goes through
//! it. A read takes the chunks it finds here and reads the others from the
//! device, a run of missing ones in one request, then keeps them. A write
//! goes to the device and updates the chunks kept of what it covers. The
//! least recently used chunk makes room for a new one (a clock over the
//! entries), and the frames go back to the allocator when it runs out
//! ([`release`]).

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use super::SECTOR;
use crate::mm;

/// A chunk: a page of the device.
const CHUNK: usize = 4096;
const CHUNK_SECTORS: u64 = (CHUNK / SECTOR) as u64;
/// Most chunks one device request reads.
const MAX_RUN: u64 = (super::MAX_REQUEST / CHUNK) as u64;
/// The share of usable memory the cache may hold.
const RAM_SHARE: u64 = 8;

struct Entry {
    key: (u32, u64),
    frame: u64,
    /// Read since the clock hand last passed it.
    used: bool,
}

struct Cache {
    /// (device, chunk) -> index in `entries`.
    index: BTreeMap<(u32, u64), usize>,
    entries: Vec<Entry>,
    hand: usize,
    /// How many writes there have been: a chunk read from the device is
    /// kept only if none came meanwhile (it could be older than the disk).
    writes: u64,
}

static CACHE: Mutex<Cache> = Mutex::new(Cache {
    index: BTreeMap::new(),
    entries: Vec::new(),
    hand: 0,
    writes: 0,
});

/// Run `f` with the cache locked and interrupts off on this CPU. Nothing
/// allocates a frame in there: the allocator may call [`release`].
fn locked<R>(f: impl FnOnce(&mut Cache) -> R) -> R {
    let flags = crate::arch::irq_save();
    crate::arch::irq_off();
    let r = f(&mut CACHE.lock());
    crate::arch::irq_restore(flags);
    r
}

fn capacity() -> usize {
    (mm::usable_frames() / RAM_SHARE) as usize
}

/// The bytes of chunk `chunk` that `[lba, lba + len)` (bytes) covers: the
/// range in the chunk and the one in the caller's buffer.
fn overlap(chunk: u64, lba: u64, len: usize) -> (core::ops::Range<usize>, core::ops::Range<usize>) {
    let start = (chunk * CHUNK as u64).max(lba * SECTOR as u64);
    let end = ((chunk + 1) * CHUNK as u64).min(lba * SECTOR as u64 + len as u64);
    let in_chunk = (start - chunk * CHUNK as u64) as usize..(end - chunk * CHUNK as u64) as usize;
    let in_buf = (start - lba * SECTOR as u64) as usize..(end - lba * SECTOR as u64) as usize;
    (in_chunk, in_buf)
}

fn chunk_bytes(frame: u64) -> &'static mut [u8] {
    unsafe { core::slice::from_raw_parts_mut(mm::hhdm(frame), CHUNK) }
}

/// Read sectors `lba..` of device `dev` (`sectors` long) into `buf`, from
/// the cache or with `device` (a sector read).
pub fn read(
    dev: u32,
    lba: u64,
    buf: &mut [u8],
    sectors: u64,
    device: impl Fn(u64, &mut [u8]) -> Result<(), ()>,
) -> Result<(), ()> {
    let first = lba / CHUNK_SECTORS;
    let end = (lba + (buf.len() / SECTOR) as u64).div_ceil(CHUNK_SECTORS);
    // Whole chunks only: a last part one past the disk's end is read as is.
    let whole_end = end.min(sectors / CHUNK_SECTORS);
    let mut chunk = first;
    while chunk < end {
        let hit = locked(|c| {
            let &i = c.index.get(&(dev, chunk))?;
            c.entries[i].used = true;
            let (from, to) = overlap(chunk, lba, buf.len());
            buf[to].copy_from_slice(&chunk_bytes(c.entries[i].frame)[from]);
            Some(())
        });
        if hit.is_some() {
            chunk += 1;
            continue;
        }
        if chunk >= whole_end {
            let (_, to) = overlap(chunk, lba, buf.len());
            let at = lba + (to.start / SECTOR) as u64;
            device(at, &mut buf[to])?;
            chunk += 1;
            continue;
        }
        // The chunks missing from here on, read together.
        let run_end = locked(|c| {
            let mut e = chunk + 1;
            while e < whole_end && e - chunk < MAX_RUN && !c.index.contains_key(&(dev, e)) {
                e += 1;
            }
            e
        });
        let writes = locked(|c| c.writes);
        let mut data = vec![0u8; (run_end - chunk) as usize * CHUNK];
        device(chunk * CHUNK_SECTORS, &mut data)?;
        for (k, bytes) in (chunk..run_end).zip(data.chunks(CHUNK)) {
            let (from, to) = overlap(k, lba, buf.len());
            buf[to].copy_from_slice(&bytes[from]);
        }
        keep(dev, chunk, &data, writes);
        chunk = run_end;
    }
    Ok(())
}

/// Keep the chunks `data` holds from `first` on, unless a write came since
/// `writes`.
fn keep(dev: u32, first: u64, data: &[u8], writes: u64) {
    let cap = capacity();
    for (k, bytes) in (first..).zip(data.chunks(CHUNK)) {
        // A frame for it, taken before the lock when the cache still grows.
        let spare = (locked(|c| c.entries.len()) < cap).then(|| mm::alloc_frame());
        let unused = locked(|c| {
            if c.writes != writes || c.index.contains_key(&(dev, k)) {
                return spare;
            }
            let i = match spare {
                Some(frame) if c.entries.len() < cap => {
                    c.entries.push(Entry { key: (dev, k), frame, used: false });
                    c.entries.len() - 1
                }
                _ => {
                    let Some(i) = evict(c) else {
                        return spare;
                    };
                    c.entries[i].key = (dev, k);
                    i
                }
            };
            chunk_bytes(c.entries[i].frame).copy_from_slice(bytes);
            c.index.insert((dev, k), i);
            if c.entries[i].frame == spare.unwrap_or(0) { None } else { spare }
        });
        if let Some(frame) = unused {
            mm::free_frame(frame);
        }
    }
}

/// The entry the clock hand finds first not read since it last passed,
/// taken out of the index for reuse.
fn evict(c: &mut Cache) -> Option<usize> {
    let n = c.entries.len();
    for _ in 0..2 * n {
        let i = c.hand % n;
        c.hand = i + 1;
        if core::mem::take(&mut c.entries[i].used) {
            continue;
        }
        c.index.remove(&c.entries[i].key);
        return Some(i);
    }
    None
}

/// After sectors `lba..` of `dev` were written with `buf`: the kept chunks
/// it covers take the new bytes.
pub fn wrote(dev: u32, lba: u64, buf: &[u8]) {
    let first = lba / CHUNK_SECTORS;
    let end = (lba + (buf.len() / SECTOR) as u64).div_ceil(CHUNK_SECTORS);
    locked(|c| {
        c.writes += 1;
        for chunk in first..end {
            if let Some(&i) = c.index.get(&(dev, chunk)) {
                let (to, from) = overlap(chunk, lba, buf.len());
                chunk_bytes(c.entries[i].frame)[to].copy_from_slice(&buf[from]);
            }
        }
    });
}

/// Forget device `dev` (it went away; its id may come back as another).
pub fn forget(dev: u32) {
    let freed = locked(|c| {
        c.writes += 1;
        let (gone, kept): (Vec<Entry>, Vec<Entry>) = core::mem::take(&mut c.entries).into_iter().partition(|e| e.key.0 == dev);
        c.entries = kept;
        c.index = c.entries.iter().enumerate().map(|(i, e)| (e.key, i)).collect();
        c.hand = 0;
        gone
    });
    for e in freed {
        mm::free_frame(e.frame);
    }
}

/// Give every frame back (the allocator ran out): how many there were.
pub fn release() -> usize {
    let freed = locked(|c| {
        c.writes += 1;
        c.index.clear();
        c.hand = 0;
        core::mem::take(&mut c.entries)
    });
    let n = freed.len();
    for e in freed {
        mm::free_frame(e.frame);
    }
    n
}

/// Frames the cache holds (`/proc/meminfo`).
pub fn frames() -> usize {
    locked(|c| c.entries.len())
}
