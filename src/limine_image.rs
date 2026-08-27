// GPT disk + FAT16 ESP writer, plus Limine binary fetch.
//
// Included from `build.rs` (`include!`) and compiled into the host crate.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const LIMINE_VERSION: &str = "12.6.1";
pub const LIMINE_TARBALL_URL: &str =
    "https://github.com/limine-bootloader/limine/releases/download/v12.6.1/limine-binary.tar.gz";
pub const LIMINE_TARBALL_SHA256: &str =
    "07d054e6297d8c41bee74ddd30024696e4ad811e7e73be28d98dc0a6168fbfeb";

pub const LIMINE_CONF: &str = "\
timeout: 0

/myos
    protocol: limine
    path: boot():/boot/kernel
";

const SECTOR: usize = 512;
const IMAGE_BYTES: usize = 64 * 1024 * 1024;
const BIOS_BOOT_START_LBA: u64 = 2048;
const BIOS_BOOT_END_LBA: u64 = 4095;
const ESP_START_LBA: u64 = 4096;

pub struct LimineFiles {
    pub dir: PathBuf,
}

impl LimineFiles {
    pub fn bootx64(&self) -> PathBuf {
        self.dir.join("BOOTX64.EFI")
    }
    pub fn bootaa64(&self) -> PathBuf {
        self.dir.join("BOOTAA64.EFI")
    }
    pub fn bios_sys(&self) -> PathBuf {
        self.dir.join("limine-bios.sys")
    }
    pub fn tool(&self) -> PathBuf {
        self.dir.join("limine")
    }
}

pub fn fetch_limine(cache_dir: &Path) -> LimineFiles {
    fs::create_dir_all(cache_dir).expect("create limine cache");
    let marker = cache_dir.join("BOOTX64.EFI");
    if !marker.is_file() {
        let tar_path = cache_dir.join("limine-binary.tar.gz");
        download(LIMINE_TARBALL_URL, &tar_path);
        verify_sha256(&tar_path, LIMINE_TARBALL_SHA256);
        let status = Command::new("tar")
            .args(["-xzf"])
            .arg(&tar_path)
            .arg("-C")
            .arg(cache_dir)
            .arg("--strip-components=1")
            .status()
            .expect("failed to spawn tar");
        if !status.success() {
            panic!("failed to unpack Limine binary tarball");
        }
    }
    compile_limine_tool(cache_dir);
    let files = LimineFiles {
        dir: cache_dir.to_path_buf(),
    };
    for p in [files.bootx64(), files.bootaa64(), files.bios_sys()] {
        if !p.is_file() {
            panic!("Limine file missing: {}", p.display());
        }
    }
    files
}

fn download(url: &str, dest: &Path) {
    let status = Command::new("curl")
        .args(["-fsSL", "--retry", "3", "-o"])
        .arg(dest)
        .arg(url)
        .status()
        .expect("failed to spawn curl (needed to fetch Limine binaries)");
    if !status.success() {
        panic!("curl failed to download {url}");
    }
}

fn verify_sha256(path: &Path, expected: &str) {
    let out = Command::new("sha256sum")
        .arg(path)
        .output()
        .expect("sha256sum");
    let text = String::from_utf8_lossy(&out.stdout);
    let got = text.split_whitespace().next().unwrap_or("");
    if got != expected {
        panic!(
            "Limine tarball sha256 mismatch: got {got}, expected {expected}"
        );
    }
}

fn compile_limine_tool(dir: &Path) {
    let out = dir.join("limine");
    if out.is_file() {
        return;
    }
    let c = dir.join("limine.c");
    let status = Command::new("cc")
        .args(["-std=c99", "-O2", "-D_FILE_OFFSET_BITS=64", "-o"])
        .arg(&out)
        .arg(&c)
        .status()
        .expect("failed to spawn cc for limine host tool");
    if !status.success() {
        panic!("failed to compile limine host tool (need a C compiler)");
    }
}

pub struct DiskFile {
    pub path: String,
    pub data: Vec<u8>,
}

/// GPT disk with a BIOS boot partition and a FAT16 ESP. `efi_name` is e.g. `BOOTX64.EFI`.
pub fn write_esp_image(
    dest: &Path,
    kernel: &[u8],
    efi_name: &str,
    efi_bytes: &[u8],
    bios_sys: Option<&[u8]>,
) {
    let mut files = vec![
        DiskFile {
            path: format!("EFI/BOOT/{efi_name}"),
            data: efi_bytes.to_vec(),
        },
        DiskFile {
            path: "boot/kernel".into(),
            data: kernel.to_vec(),
        },
        DiskFile {
            path: "boot/limine/limine.conf".into(),
            data: LIMINE_CONF.as_bytes().to_vec(),
        },
        DiskFile {
            path: "EFI/BOOT/limine.conf".into(),
            data: LIMINE_CONF.as_bytes().to_vec(),
        },
    ];
    if let Some(sys) = bios_sys {
        files.push(DiskFile {
            path: "boot/limine/limine-bios.sys".into(),
            data: sys.to_vec(),
        });
    }
    let image = build_gpt_fat16(&files);
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(dest, &image).unwrap_or_else(|e| panic!("write {}: {e}", dest.display()));
}

pub fn bios_install(limine_tool: &Path, image: &Path) {
    let status = Command::new(limine_tool)
        .arg("bios-install")
        .arg(image)
        .status()
        .expect("failed to spawn limine bios-install");
    if !status.success() {
        panic!("limine bios-install failed for {}", image.display());
    }
}

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
        0,
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

fn format_and_write_fat16(part: &mut [u8], files: &[DiskFile]) -> () {
    let total_sectors = (part.len() / SECTOR) as u32;
    let reserved = 1u16;
    let fats = 2u8;
    let root_entries = 512u16;
    let spc = 8u8;
    let root_sectors = (root_entries as u32 * 32).div_ceil(SECTOR as u32);
    let mut fat_sectors = 128u32;
    for _ in 0..8 {
        let data_sectors = total_sectors
            .saturating_sub(reserved as u32)
            .saturating_sub(fats as u32 * fat_sectors)
            .saturating_sub(root_sectors);
        let clusters = data_sectors / spc as u32;
        let needed = ((clusters + 2) * 2).div_ceil(SECTOR as u32);
        if needed <= fat_sectors {
            break;
        }
        fat_sectors = needed;
    }
    let data_start = reserved as u32 + fats as u32 * fat_sectors + root_sectors;
    let data_sectors = total_sectors - data_start;
    let clusters = data_sectors / spc as u32;
    if clusters < 4085 || clusters > 65524 {
        panic!("FAT16 cluster count {clusters} out of range (fat_sectors={fat_sectors})");
    }
    part[0] = 0xEB;
    part[1] = 0x3C;
    part[2] = 0x90;
    part[3..11].copy_from_slice(b"MSDOS5.0");
    part[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes());
    part[13] = spc;
    part[14..16].copy_from_slice(&reserved.to_le_bytes());
    part[16] = fats;
    part[17..19].copy_from_slice(&root_entries.to_le_bytes());
    if total_sectors < 0x10000 {
        part[19..21].copy_from_slice(&(total_sectors as u16).to_le_bytes());
    } else {
        part[19..21].copy_from_slice(&0u16.to_le_bytes());
        part[32..36].copy_from_slice(&total_sectors.to_le_bytes());
    }
    part[21] = 0xF8;
    part[22..24].copy_from_slice(&(fat_sectors as u16).to_le_bytes());
    part[24..26].copy_from_slice(&32u16.to_le_bytes());
    part[26..28].copy_from_slice(&16u16.to_le_bytes());
    part[36] = 0x80;
    part[38] = 0x29;
    part[39..43].copy_from_slice(&0x6D79_6F73u32.to_le_bytes());
    part[43..54].copy_from_slice(b"MYOS       ");
    part[54..62].copy_from_slice(b"FAT16   ");
    part[510] = 0x55;
    part[511] = 0xAA;
    let fat0 = reserved as usize * SECTOR;
    let fat1 = fat0 + fat_sectors as usize * SECTOR;
    let root_off = fat1 + fat_sectors as usize * SECTOR;
    let data_off = root_off + root_sectors as usize * SECTOR;
    write_fat16_ent(part, fat0, 0, 0xFFF8);
    write_fat16_ent(part, fat0, 1, 0xFFFF);
    let mut next_cluster: u16 = 2;
    let mut root = RootDir {
        off: root_off,
        len: root_sectors as usize * SECTOR,
        used: 0,
    };
    for f in files {
        put_file(part, fat0, data_off, spc, clusters as u16, &mut next_cluster, &mut root, &f.path, &f.data);
    }
    let fat_bytes = fat_sectors as usize * SECTOR;
    part.copy_within(fat0..fat0 + fat_bytes, fat1);
}

fn write_fat16_ent(part: &mut [u8], fat0: usize, idx: u16, val: u16) {
    let off = fat0 + idx as usize * 2;
    part[off..off + 2].copy_from_slice(&val.to_le_bytes());
}

struct RootDir {
    off: usize,
    len: usize,
    used: usize,
}

fn put_file(part: &mut [u8], fat0: usize, data_off: usize, spc: u8, max_cluster: u16, next_cluster: &mut u16, root: &mut RootDir, path: &str, data: &[u8]) {
    let comps: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    put_path(part, fat0, data_off, spc, max_cluster, next_cluster, root, &comps, data);
}

fn put_path(part: &mut [u8], fat0: usize, data_off: usize, spc: u8, max_cluster: u16, next_cluster: &mut u16, root: &mut RootDir, comps: &[&str], file_data: &[u8]) {
    let mut dir_cluster: u16 = 0;
    for (i, comp) in comps.iter().enumerate() {
        let is_last = i + 1 == comps.len();
        if let Some(cl) = lookup_name(part, fat0, data_off, spc, dir_cluster, root, comp) {
            if is_last { panic!("file already exists: {comp}"); }
            dir_cluster = cl;
            continue;
        }
        if is_last {
            let first = alloc_file(part, fat0, data_off, spc, max_cluster, next_cluster, file_data);
            add_dirent(part, fat0, data_off, spc, dir_cluster, root, comp, first, file_data.len() as u32, false);
        } else {
            let cl = alloc_dir(part, fat0, data_off, spc, max_cluster, next_cluster, dir_cluster);
            add_dirent(part, fat0, data_off, spc, dir_cluster, root, comp, cl, 0, true);
            dir_cluster = cl;
        }
    }
}

fn cluster_bytes(spc: u8) -> usize { spc as usize * SECTOR }
fn cluster_off(data_off: usize, spc: u8, cl: u16) -> usize { data_off + (cl as usize - 2) * cluster_bytes(spc) }

fn alloc_clusters(part: &mut [u8], fat0: usize, max_cluster: u16, next_cluster: &mut u16, count: u16) -> u16 {
    if count == 0 { return 0; }
    let first = *next_cluster;
    if first as u32 + count as u32 - 1 > max_cluster as u32 + 1 { panic!("FAT16 out of clusters"); }
    for i in 0..count {
        let cl = first + i;
        let val = if i + 1 == count { 0xFFFF } else { cl + 1 };
        write_fat16_ent(part, fat0, cl, val);
    }
    *next_cluster = first + count;
    first
}

fn alloc_file(part: &mut [u8], fat0: usize, data_off: usize, spc: u8, max_cluster: u16, next_cluster: &mut u16, data: &[u8]) -> u16 {
    if data.is_empty() { return 0; }
    let cb = cluster_bytes(spc);
    let n = (data.len() + cb - 1) / cb;
    let first = alloc_clusters(part, fat0, max_cluster, next_cluster, n as u16);
    let mut cl = first;
    let mut off = 0;
    while off < data.len() {
        let nbyte = (data.len() - off).min(cb);
        let dest = cluster_off(data_off, spc, cl);
        part[dest..dest + nbyte].copy_from_slice(&data[off..off + nbyte]);
        off += nbyte;
        if off < data.len() {
            let ent = u16::from_le_bytes(part[fat0 + cl as usize * 2..fat0 + cl as usize * 2 + 2].try_into().unwrap());
            cl = ent;
        }
    }
    first
}

fn alloc_dir(part: &mut [u8], fat0: usize, data_off: usize, spc: u8, max_cluster: u16, next_cluster: &mut u16, parent: u16) -> u16 {
    let cl = alloc_clusters(part, fat0, max_cluster, next_cluster, 1);
    let dest = cluster_off(data_off, spc, cl);
    let cb = cluster_bytes(spc);
    part[dest..dest + cb].fill(0);
    write_short_dirent(&mut part[dest..dest + 32], b".          ", 0x10, cl, 0);
    let parent_cl = if parent == 0 { 0 } else { parent };
    write_short_dirent(&mut part[dest + 32..dest + 64], b"..         ", 0x10, parent_cl, 0);
    cl
}

fn lookup_name(part: &[u8], _fat0: usize, data_off: usize, spc: u8, dir_cluster: u16, root: &RootDir, name: &str) -> Option<u16> {
    let dir_bytes: &[u8] = if dir_cluster == 0 {
        &part[root.off..root.off + root.len]
    } else {
        let dest = cluster_off(data_off, spc, dir_cluster);
        &part[dest..dest + cluster_bytes(spc)]
    };
    let want = name.as_bytes();
    let mut i = 0;
    while i + 32 <= dir_bytes.len() {
        let ent = &dir_bytes[i..i + 32];
        if ent[0] == 0 { break; }
        if ent[0] == 0xE5 || ent[11] == 0x0F { i += 32; continue; }
        let long = collect_lfn(dir_bytes, i);
        let short = decode_short(ent);
        if long.as_bytes() == want || short.eq_ignore_ascii_case(name) {
            if ent[11] & 0x10 != 0 {
                let cl = u16::from_le_bytes(ent[26..28].try_into().unwrap());
                return Some(cl);
            }
            return Some(0);
        }
        i += 32;
    }
    None
}

fn collect_lfn(dir: &[u8], short_off: usize) -> String {
    let mut chars = Vec::new();
    if short_off < 32 { return String::new(); }
    let mut off = short_off;
    loop {
        if off < 32 { break; }
        off -= 32;
        let ent = &dir[off..off + 32];
        if ent[11] != 0x0F { break; }
        for pos in [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30] {
            let lo = ent[pos];
            let hi = ent[pos + 1];
            let c = u16::from_le_bytes([lo, hi]);
            if c == 0 || c == 0xFFFF { break; }
            if let Some(ch) = char::from_u32(c as u32) { chars.push(ch); }
        }
        if ent[0] & 0x40 != 0 { break; }
    }
    chars.into_iter().collect()
}

fn decode_short(ent: &[u8]) -> String {
    let name = core::str::from_utf8(&ent[0..8]).unwrap_or("").trim_end();
    let ext = core::str::from_utf8(&ent[8..11]).unwrap_or("").trim_end();
    if ext.is_empty() { name.to_string() } else { format!("{name}.{ext}") }
}

fn add_dirent(part: &mut [u8], fat0: usize, data_off: usize, spc: u8, dir_cluster: u16, root: &mut RootDir, name: &str, cluster: u16, size: u32, is_dir: bool) {
    let (short, lfn_needed) = make_short_name(name);
    let checksum = lfn_checksum(&short);
    let mut entries: Vec<[u8; 32]> = Vec::new();
    if lfn_needed {
        let utf16: Vec<u16> = name.encode_utf16().collect();
        let n = (utf16.len() + 12) / 13;
        for seq in (1..=n).rev() {
            let mut e = [0u8; 32];
            let idx = seq - 1;
            e[0] = seq as u8;
            if seq == n { e[0] |= 0x40; }
            e[11] = 0x0F;
            e[13] = checksum;
            let chunk = &utf16[idx * 13..utf16.len().min(idx * 13 + 13)];
            let mut slots = [0xFFFFu16; 13];
            for (i, c) in chunk.iter().enumerate() { slots[i] = *c; }
            if chunk.len() < 13 { slots[chunk.len()] = 0; }
            let positions = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
            for (i, pos) in positions.iter().enumerate() {
                e[*pos..*pos + 2].copy_from_slice(&slots[i].to_le_bytes());
            }
            entries.push(e);
        }
    }
    let mut short_e = [0u8; 32];
    let attr = if is_dir { 0x10 } else { 0x20 };
    write_short_dirent(&mut short_e, &short, attr, cluster, size);
    entries.push(short_e);
    append_entries(part, fat0, data_off, spc, dir_cluster, root, &entries);
}

fn append_entries(part: &mut [u8], _fat0: usize, data_off: usize, spc: u8, dir_cluster: u16, root: &mut RootDir, entries: &[[u8; 32]]) {
    let need = entries.len() * 32;
    if dir_cluster == 0 {
        if root.used + need > root.len { panic!("root directory full"); }
        for e in entries {
            let o = root.off + root.used;
            part[o..o + 32].copy_from_slice(e);
            root.used += 32;
        }
        return;
    }
    let dest = cluster_off(data_off, spc, dir_cluster);
    let dir = &mut part[dest..dest + cluster_bytes(spc)];
    let mut used = 0;
    while used + 32 <= dir.len() {
        if dir[used] == 0 { break; }
        used += 32;
    }
    if used + need > dir.len() { panic!("subdirectory full"); }
    for e in entries {
        dir[used..used + 32].copy_from_slice(e);
        used += 32;
    }
}

fn write_short_dirent(e: &mut [u8], name11: &[u8], attr: u8, cluster: u16, size: u32) {
    e[..11].copy_from_slice(name11);
    e[11] = attr;
    e[26..28].copy_from_slice(&cluster.to_le_bytes());
    e[28..32].copy_from_slice(&size.to_le_bytes());
}

fn lfn_checksum(name: &[u8; 11]) -> u8 {
    let mut sum = 0u8;
    for b in name { sum = ((sum & 1) << 7).wrapping_add(sum >> 1).wrapping_add(*b); }
    sum
}

fn make_short_name(name: &str) -> ([u8; 11], bool) {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !name.starts_with('.') => (s, e),
        _ => (name, ""),
    };
    let stem_ok = stem.len() <= 8 && ext.len() <= 3 && stem.chars().all(is_short_char) && ext.chars().all(is_short_char) && stem.chars().all(|c| !c.is_ascii_lowercase()) && ext.chars().all(|c| !c.is_ascii_lowercase());
    if stem_ok {
        let mut out = [b' '; 11];
        for (i, b) in stem.bytes().take(8).enumerate() { out[i] = b; }
        for (i, b) in ext.bytes().take(3).enumerate() { out[8 + i] = b; }
        return (out, false);
    }
    let mut out = [b' '; 11];
    let cleaned: String = stem.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).take(6).collect();
    let bytes = cleaned.as_bytes();
    out[..bytes.len()].copy_from_slice(bytes);
    out[bytes.len()] = b'~';
    out[bytes.len() + 1] = b'1';
    let ext_c: String = ext.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).take(3).collect();
    for (i, b) in ext_c.bytes().enumerate() { out[8 + i] = b; }
    (out, true)
}

fn is_short_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}
