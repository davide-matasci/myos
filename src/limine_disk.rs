// The boot disk (`docs/install.md`): a GPT with a BIOS boot partition, a
// FAT32 ESP with two boot slots, and an ext2 data partition. Included from
// `limine_image.rs`.

/// The ESP: two slots of the kernel and an initramfs with the Linux layer
/// (some 50 MiB each), with room to grow.
const ESP_BYTES: u64 = 512 * 1024 * 1024;
/// The data partition: state an upgrade keeps (`docs/install.md`).
const DATA_BYTES: u64 = 64 * 1024 * 1024;
const MIB_LBA: u64 = 1024 * 1024 / SECTOR as u64;
const ESP_END_LBA: u64 = ESP_START_LBA + ESP_BYTES / SECTOR as u64 - 1;
const DATA_START_LBA: u64 = ESP_END_LBA + 1;
const DATA_END_LBA: u64 = DATA_START_LBA + DATA_BYTES / SECTOR as u64 - 1;
/// The partitions, then a MiB for the backup GPT.
const IMAGE_BYTES: u64 = (DATA_END_LBA + 1 + MIB_LBA) * SECTOR as u64;

const BIOS_BOOT_TYPE: [u8; 16] = [
    0x48, 0x61, 0x68, 0x21, 0x49, 0x64, 0x6F, 0x6E, 0x74, 0x4E, 0x65, 0x65, 0x64, 0x45, 0x46, 0x49,
];
const ESP_TYPE: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];
const LINUX_DATA_TYPE: [u8; 16] = [
    0xAF, 0x3D, 0xC6, 0x0F, 0x83, 0x84, 0x72, 0x47, 0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47, 0x7D, 0xE4,
];

/// A partition of [`write_gpt`]: its entry (from 0), type and unique GUIDs,
/// first and last LBA, attributes and name.
struct GptPart<'a> {
    entry: usize,
    type_guid: [u8; 16],
    uuid: [u8; 16],
    first: u64,
    last: u64,
    attrs: u64,
    name: &'a str,
}

/// Write a protective MBR and the primary and backup GPT of `parts` to `f`,
/// a disk of `total_lba` sectors; nothing else of the disk is touched.
fn write_gpt(f: &fs::File, total_lba: u64, parts: &[GptPart]) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    let mut entries = [0u8; 128 * 128];
    for p in parts {
        let e = &mut entries[p.entry * 128..(p.entry + 1) * 128];
        write_gpt_entry(e, &p.type_guid, &p.uuid, p.first, p.last, p.attrs, p.name);
    }
    let crc = crc32(&entries);
    let backup_lba = total_lba - 1;
    let mut head = vec![0u8; 34 * SECTOR];
    write_protective_mbr(&mut head, total_lba);
    head[SECTOR..SECTOR + 92].copy_from_slice(&gpt_header(1, backup_lba, 2, crc, total_lba));
    head[2 * SECTOR..].copy_from_slice(&entries);
    let mut tail = vec![0u8; 33 * SECTOR];
    tail[..entries.len()].copy_from_slice(&entries);
    tail[32 * SECTOR..32 * SECTOR + 92].copy_from_slice(&gpt_header(backup_lba, 1, backup_lba - 32, crc, total_lba));
    f.write_all_at(&head, 0)?;
    f.write_all_at(&tail, (backup_lba - 32) * SECTOR as u64)
}

/// A partition of a disk image file, as the sector device `fatvol` and
/// `ext2fs` take.
struct PartDev<'a> {
    file: &'a fs::File,
    first: u64,
    sectors: u64,
}

impl fatvol::SectorDriver for PartDev<'_> {
    type Error = std::io::Error;
    fn sector_size(&self) -> u32 {
        SECTOR as u32
    }
    fn sector_count(&self) -> u64 {
        self.sectors
    }
    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> std::io::Result<()> {
        std::os::unix::fs::FileExt::read_exact_at(self.file, buf, (self.first + lba) * SECTOR as u64)
    }
    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> std::io::Result<()> {
        std::os::unix::fs::FileExt::write_all_at(self.file, buf, (self.first + lba) * SECTOR as u64)
    }
}

impl ext2fs::Device for PartDev<'_> {
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> bool {
        std::os::unix::fs::FileExt::read_exact_at(self.file, buf, self.first * SECTOR as u64 + offset).is_ok()
    }
    fn write(&mut self, offset: u64, buf: &[u8]) -> bool {
        std::os::unix::fs::FileExt::write_all_at(self.file, buf, self.first * SECTOR as u64 + offset).is_ok()
    }
}

/// Format the partition of `dev` as an empty FAT32 volume: 4 KiB clusters,
/// two FATs, the root directory in cluster 2, the FSInfo sector and the
/// backup boot sector. `hidden` is the partition's first LBA.
fn format_fat32(dev: &mut PartDev, hidden: u32) -> std::io::Result<()> {
    use fatvol::SectorDriver;
    const RESERVED: u32 = 32;
    const SPC: u32 = 8;
    let total = dev.sectors as u32;
    // The FAT holds an entry (4 bytes) per cluster, two reserved ones too.
    let mut fat_sectors = 1u32;
    loop {
        let clusters = (total - RESERVED - 2 * fat_sectors) / SPC;
        let need = ((clusters + 2) * 4).div_ceil(SECTOR as u32);
        if need <= fat_sectors {
            break;
        }
        fat_sectors = need;
    }
    let mut boot = [0u8; SECTOR];
    boot[..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    boot[3..11].copy_from_slice(b"MYOS    ");
    boot[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes());
    boot[13] = SPC as u8;
    boot[14..16].copy_from_slice(&(RESERVED as u16).to_le_bytes());
    boot[16] = 2;
    boot[21] = 0xF8;
    boot[24..26].copy_from_slice(&63u16.to_le_bytes());
    boot[26..28].copy_from_slice(&255u16.to_le_bytes());
    boot[28..32].copy_from_slice(&hidden.to_le_bytes());
    boot[32..36].copy_from_slice(&total.to_le_bytes());
    boot[36..40].copy_from_slice(&fat_sectors.to_le_bytes());
    // Root directory cluster, FSInfo sector, backup boot sector.
    boot[44..48].copy_from_slice(&2u32.to_le_bytes());
    boot[48..50].copy_from_slice(&1u16.to_le_bytes());
    boot[50..52].copy_from_slice(&6u16.to_le_bytes());
    boot[64] = 0x80;
    boot[66] = 0x29;
    boot[67..71].copy_from_slice(&0x4D59_4F53u32.to_le_bytes());
    boot[71..82].copy_from_slice(b"NO NAME    ");
    boot[82..90].copy_from_slice(b"FAT32   ");
    boot[510..512].copy_from_slice(&[0x55, 0xAA]);
    let mut fsinfo = [0u8; SECTOR];
    fsinfo[..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
    fsinfo[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
    // Free count and next free cluster unknown.
    fsinfo[488..496].fill(0xFF);
    fsinfo[508..512].copy_from_slice(&0xAA55_0000u32.to_le_bytes());
    for at in [0, 6] {
        dev.write_sectors(at, &boot)?;
        dev.write_sectors(at + 1, &fsinfo)?;
    }
    // Media, end of chain (reserved), the root directory's one cluster.
    let mut fat = [0u8; SECTOR];
    fat[..12].copy_from_slice(&[0xF8, 0xFF, 0xFF, 0x0F, 0xFF, 0xFF, 0xFF, 0x0F, 0xFF, 0xFF, 0xFF, 0x0F]);
    for copy in 0..2 {
        dev.write_sectors(u64::from(RESERVED + copy * fat_sectors), &fat)?;
    }
    Ok(())
}

/// The boot disk image at `dest`, sparse (only what is written takes space):
///
/// - partition 1, BIOS boot (1 MiB at LBA 2048), where `limine bios-install`
///   puts its stage 2;
/// - partition 2, the ESP: FAT32, 512 MiB, holding `files` and the empty
///   `dirs`;
/// - partition 3, the data partition: ext2, 64 MiB, empty.
fn write_boot_disk(dest: &Path, files: &[DiskFile], dirs: &[&str]) {
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // Read and written: fatvol reads what it changes.
    let f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(dest)
        .unwrap_or_else(|e| die(dest, "create", &e));
    f.set_len(IMAGE_BYTES).unwrap_or_else(|e| die(dest, "size", &e));
    let uuid = |n: u8| [0x73, 0x6F, 0x79, 0x6D, 0x00, 0x00, 0x00, 0x40, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, n];
    let parts = [
        GptPart {
            entry: 0,
            type_guid: BIOS_BOOT_TYPE,
            uuid: uuid(3),
            first: BIOS_BOOT_START_LBA,
            last: BIOS_BOOT_END_LBA,
            // Legacy BIOS bootable.
            attrs: 4,
            name: "BIOS Boot",
        },
        GptPart {
            entry: 1,
            type_guid: ESP_TYPE,
            uuid: uuid(2),
            first: ESP_START_LBA,
            last: ESP_END_LBA,
            // Required by the platform.
            attrs: 1,
            name: "EFI System",
        },
        GptPart {
            entry: 2,
            type_guid: LINUX_DATA_TYPE,
            uuid: uuid(4),
            first: DATA_START_LBA,
            last: DATA_END_LBA,
            attrs: 0,
            name: "myos data",
        },
    ];
    write_gpt(&f, IMAGE_BYTES / SECTOR as u64, &parts).unwrap_or_else(|e| die(dest, "GPT", &e));

    let sectors = |first: u64, last: u64| last - first + 1;
    let mut esp = PartDev { file: &f, first: ESP_START_LBA, sectors: sectors(ESP_START_LBA, ESP_END_LBA) };
    format_fat32(&mut esp, ESP_START_LBA as u32).unwrap_or_else(|e| die(dest, "format the ESP", &e));
    let mut vol = fatvol::Fat::mount(esp).unwrap_or_else(|e| die(dest, "mount the ESP", &e));
    let mkdirs = |vol: &mut fatvol::Fat<PartDev>, path: &str| {
        let mut at = String::new();
        for part in path.split('/') {
            at = if at.is_empty() { part.to_string() } else { format!("{at}/{part}") };
            if vol.stat(&at).is_err() {
                vol.mkdir(&at).unwrap_or_else(|e| die(dest, &format!("mkdir {at}"), &e));
            }
        }
    };
    for dir in dirs {
        mkdirs(&mut vol, dir);
    }
    for file in files {
        if let Some((dir, _)) = file.path.rsplit_once('/') {
            mkdirs(&mut vol, dir);
        }
        vol.create(&file.path).unwrap_or_else(|e| die(dest, &format!("create {}", file.path), &e));
        let n = vol.write(&file.path, 0, &file.data).unwrap_or_else(|e| die(dest, &format!("write {}", file.path), &e));
        if n != file.data.len() {
            die(dest, &format!("write {}", file.path), &"the ESP is full");
        }
    }
    vol.unmount().unwrap_or_else(|e| die(dest, "unmount the ESP", &e));

    let mut data = PartDev { file: &f, first: DATA_START_LBA, sectors: sectors(DATA_START_LBA, DATA_END_LBA) };
    ext2fs::mkfs(&mut data, DATA_BYTES).unwrap_or_else(|e| die(dest, "mkfs.ext2 the data partition", &e));
}

fn die(dest: &Path, what: &str, e: &dyn std::fmt::Debug) -> ! {
    panic!("{}: {what}: {e:?}", dest.display())
}

/// Copy the disk image `src` to `dst`, leaving its unwritten (zero) parts
/// out, as [`write_boot_disk`] did.
pub fn copy_sparse(src: &Path, dst: &Path) {
    use std::os::unix::fs::FileExt;
    let what = format!("copy from {}", src.display());
    let from = fs::File::open(src).unwrap_or_else(|e| die(dst, &what, &e));
    let len = from.metadata().unwrap_or_else(|e| die(dst, &what, &e)).len();
    let to = fs::File::create(dst).unwrap_or_else(|e| die(dst, &what, &e));
    to.set_len(len).unwrap_or_else(|e| die(dst, &what, &e));
    let mut buf = vec![0u8; 1024 * 1024];
    let mut at = 0u64;
    while at < len {
        let n = buf.len().min((len - at) as usize);
        from.read_exact_at(&mut buf[..n], at).unwrap_or_else(|e| die(dst, &what, &e));
        for (i, chunk) in buf[..n].chunks(64 * 1024).enumerate() {
            if chunk.iter().any(|&b| b != 0) {
                to.write_all_at(chunk, at + (i * 64 * 1024) as u64).unwrap_or_else(|e| die(dst, &what, &e));
            }
        }
        at += n as u64;
    }
}
