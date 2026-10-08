//! The GUID partition table (UEFI spec, chapter 5): a protective MBR in
//! sector 0, the header in sector 1 and the entry array it points to,
//! copied at the end of the disk (the backup header in the last sector).
//! Both the header and the array carry a CRC32; when the primary's fail,
//! the backup is read. MBR partition tables are not read.

use alloc::vec;
use alloc::vec::Vec;

use super::{LABEL_MAX, SECTOR};

/// A used entry.
pub struct Entry {
    /// Its index in the array, from 1: the `p<N>` of its device.
    pub number: u32,
    pub first: u64,
    /// Inclusive.
    pub last: u64,
    pub type_guid: [u8; 16],
    pub uuid: [u8; 16],
    pub label: [u16; LABEL_MAX],
}

/// The array a header may point to, at most (128 entries of 128 bytes is
/// what every tool writes).
const MAX_ARRAY: usize = 256 * 1024;

struct Header {
    first_usable: u64,
    last_usable: u64,
    entries_lba: u64,
    entries: u32,
    entry_size: u32,
    entries_crc: u32,
}

/// The used entries of the GPT on disk `dev` (`sectors` long); none
/// without a valid one.
pub fn read(dev: u32, sectors: u64) -> Vec<Entry> {
    let mut mbr = [0u8; SECTOR];
    if sectors < 3 || super::read(dev, 0, &mut mbr).is_err() || !protective(&mbr) {
        return Vec::new();
    }
    for lba in [1, sectors - 1] {
        if let Some(entries) = header(dev, lba, sectors).and_then(|h| entries(dev, &h)) {
            return entries;
        }
    }
    Vec::new()
}

/// A boot signature and an entry of type 0xEE (a hybrid MBR's other
/// entries are ignored).
fn protective(mbr: &[u8; SECTOR]) -> bool {
    mbr[510..512] == [0x55, 0xAA] && (0..4).any(|i| mbr[446 + 16 * i + 4] == 0xEE)
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// The header in sector `lba`, if it is one: signature, size, CRC, its own
/// LBA, and ranges inside the disk.
fn header(dev: u32, lba: u64, sectors: u64) -> Option<Header> {
    let mut sec = [0u8; SECTOR];
    super::read(dev, lba, &mut sec).ok()?;
    let size = u32_at(&sec, 12) as usize;
    if &sec[..8] != b"EFI PART" || !(92..=SECTOR).contains(&size) {
        return None;
    }
    let crc = u32_at(&sec, 16);
    sec[16..20].fill(0);
    if crc32(&sec[..size]) != crc || u64_at(&sec, 24) != lba {
        return None;
    }
    let h = Header {
        first_usable: u64_at(&sec, 40),
        last_usable: u64_at(&sec, 48),
        entries_lba: u64_at(&sec, 72),
        entries: u32_at(&sec, 80),
        entry_size: u32_at(&sec, 84),
        entries_crc: u32_at(&sec, 88),
    };
    let bytes = h.entries as usize * h.entry_size as usize;
    let array_end = h.entries_lba.checked_add(bytes.div_ceil(SECTOR) as u64)?;
    let ok = h.first_usable <= h.last_usable
        && h.last_usable < sectors
        && h.entry_size >= 128
        && h.entry_size % 8 == 0
        && bytes <= MAX_ARRAY
        && h.entries_lba >= 2
        && array_end <= sectors;
    ok.then_some(h)
}

/// The used entries of `h`'s array, if its CRC matches.
fn entries(dev: u32, h: &Header) -> Option<Vec<Entry>> {
    let bytes = h.entries as usize * h.entry_size as usize;
    let mut array = vec![0u8; bytes.div_ceil(SECTOR) * SECTOR];
    super::read(dev, h.entries_lba, &mut array).ok()?;
    if crc32(&array[..bytes]) != h.entries_crc {
        return None;
    }
    let mut out = Vec::new();
    for (i, e) in array[..bytes].chunks(h.entry_size as usize).enumerate() {
        let type_guid: [u8; 16] = e[..16].try_into().unwrap();
        if type_guid == [0; 16] {
            continue;
        }
        let (first, last) = (u64_at(e, 32), u64_at(e, 40));
        // An entry outside the usable range is not one to read or write.
        if first > last || first < h.first_usable || last > h.last_usable {
            continue;
        }
        let mut label = [0u16; LABEL_MAX];
        for (k, unit) in label.iter_mut().enumerate() {
            *unit = u16::from_le_bytes([e[56 + 2 * k], e[57 + 2 * k]]);
        }
        out.push(Entry {
            number: i as u32 + 1,
            first,
            last,
            type_guid,
            uuid: e[16..32].try_into().unwrap(),
            label,
        });
    }
    Some(out)
}

/// The CRC32 of the GPT (IEEE 802.3, reflected).
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

/// A GUID as text: its first three fields are little-endian.
pub fn guid_text(g: &[u8; 16]) -> alloc::string::String {
    alloc::format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        u32::from_le_bytes([g[0], g[1], g[2], g[3]]),
        u16::from_le_bytes([g[4], g[5]]),
        u16::from_le_bytes([g[6], g[7]]),
        g[8],
        g[9],
        g[10],
        g[11],
        g[12],
        g[13],
        g[14],
        g[15]
    )
}
