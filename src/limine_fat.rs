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
