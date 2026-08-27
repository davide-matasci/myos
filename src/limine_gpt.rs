fn build_gpt_fat16(files: &[DiskFile]) -> Vec<u8> {
    let mut disk = vec![0u8; IMAGE_BYTES];
    let total_lba = (IMAGE_BYTES / SECTOR) as u64;
    let backup_lba = total_lba - 1;
    let esp_end = backup_lba - 33;
    let esp_lbas = esp_end - ESP_START_LBA + 1;

    write_protective_mbr(&mut disk, total_lba);
    let mut entries = [0u8; 128 * 128];
    // Partition 0: BIOS boot (no filesystem); limine bios-install embeds here.
    write_gpt_entry(
        &mut entries[0..128],
        &[
            0x48, 0x61, 0x68, 0x21, 0x49, 0x64, 0x6F, 0x6E, 0x74, 0x4E, 0x65, 0x65, 0x64, 0x45,
            0x46, 0x49,
        ],
        &[
            0x73, 0x6F, 0x79, 0x6D, 0x00, 0x00, 0x00, 0x40, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x03,
        ],
        BIOS_BOOT_START_LBA,
        BIOS_BOOT_END_LBA,
        4, // legacy-BIOS-bootable (bit 2)
        "BIOS Boot",
    );
    // Partition 1: FAT16 ESP
    write_gpt_entry(
        &mut entries[128..256],
        &[
            0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E,
            0xC9, 0x3B,
        ],
        &[
            0x73, 0x6F, 0x79, 0x6D, 0x00, 0x00, 0x00, 0x40, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x02,
        ],
        ESP_START_LBA,
        esp_end,
        1,
        "EFI System",
    );
    let entries_crc = crc32(&entries);
    write_gpt_header(&mut disk, 1, backup_lba, 2, entries_crc, total_lba, true);
    disk[2 * SECTOR..2 * SECTOR + entries.len()].copy_from_slice(&entries);
    let backup_entries_lba = backup_lba - 32;
    let off = backup_entries_lba as usize * SECTOR;
    disk[off..off + entries.len()].copy_from_slice(&entries);
    write_gpt_header(
        &mut disk,
        backup_lba,
        1,
        backup_entries_lba,
        entries_crc,
        total_lba,
        false,
    );

    let part_off = ESP_START_LBA as usize * SECTOR;
    let part = &mut disk[part_off..part_off + esp_lbas as usize * SECTOR];
    format_and_write_fat16(part, files);
    disk
}

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

fn write_gpt_header(
    disk: &mut [u8],
    this_lba: u64,
    alt_lba: u64,
    entries_lba: u64,
    entries_crc: u32,
    total_lba: u64,
    primary: bool,
) {
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
    let off = this_lba as usize * SECTOR;
    disk[off..off + 92].copy_from_slice(&h);
    let _ = primary;
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
