fn write_protective_mbr(disk: &mut [u8], total_lba: u64) {
    disk[510] = 0x55;
    disk[511] = 0xAA;
    let p = &mut disk[446..462];
    p[0] = 0x00;
    p[1] = 0x00;
    p[2] = 0x02;
    p[3] = 0x00;
    p[4] = 0xEE;
    p[5] = 0xFF;
    p[6] = 0xFF;
    p[7] = 0xFF;
    p[8..12].copy_from_slice(&1u32.to_le_bytes());
    let sectors = (total_lba - 1).min(u32::MAX as u64) as u32;
    p[12..16].copy_from_slice(&sectors.to_le_bytes());
}

fn write_gpt_entry(
    e: &mut [u8],
    type_guid: &[u8; 16],
    unique_guid: &[u8; 16],
    start: u64,
    end: u64,
    attrs: u64,
    name: &str,
) {
    e[0..16].copy_from_slice(type_guid);
    e[16..32].copy_from_slice(unique_guid);
    e[32..40].copy_from_slice(&start.to_le_bytes());
    e[40..48].copy_from_slice(&end.to_le_bytes());
    e[48..56].copy_from_slice(&attrs.to_le_bytes());
    let name: Vec<u16> = name.encode_utf16().collect();
    for (i, c) in name.iter().take(36).enumerate() {
        e[56 + i * 2..56 + i * 2 + 2].copy_from_slice(&c.to_le_bytes());
    }
}

/// A GPT header for a disk of `total_lba` sectors with 128 entries of 128
/// bytes, usable from LBA 34 to the backup entries.
fn gpt_header(this_lba: u64, alt_lba: u64, entries_lba: u64, entries_crc: u32, total_lba: u64) -> [u8; 92] {
    let mut h = [0u8; 92];
    h[0..8].copy_from_slice(b"EFI PART");
    h[8..12].copy_from_slice(&0x00010000u32.to_le_bytes());
    h[12..16].copy_from_slice(&92u32.to_le_bytes());
    // crc at 16..20 left zero for now
    h[24..32].copy_from_slice(&this_lba.to_le_bytes());
    h[32..40].copy_from_slice(&alt_lba.to_le_bytes());
    h[40..48].copy_from_slice(&34u64.to_le_bytes());
    h[48..56].copy_from_slice(&(total_lba - 34).to_le_bytes());
    // disk GUID
    h[56..72].copy_from_slice(&[
        0x73, 0x6F, 0x79, 0x6D, 0x00, 0x00, 0x00, 0x40, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x01,
    ]);
    h[72..80].copy_from_slice(&entries_lba.to_le_bytes());
    h[80..84].copy_from_slice(&128u32.to_le_bytes());
    h[84..88].copy_from_slice(&128u32.to_le_bytes());
    h[88..92].copy_from_slice(&entries_crc.to_le_bytes());
    let crc = crc32(&h);
    h[16..20].copy_from_slice(&crc.to_le_bytes());
    h
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
