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
    // FAT 8.3 names are stored uppercase. Lowercase inputs like "boot" are
    // valid short names once uppercased; treating lowercase as illegal forced
    // BOOT~1 + LFN, broke directory reuse, and left Limine unable to find
    // limine-bios.sys (silent BIOS hang / zero serial in QEMU).
    let stem_up: String = stem.chars().map(|c| c.to_ascii_uppercase()).collect();
    let ext_up: String = ext.chars().map(|c| c.to_ascii_uppercase()).collect();
    let stem_ok = stem_up.len() <= 8
        && ext_up.len() <= 3
        && stem_up.chars().all(is_short_char)
        && ext_up.chars().all(is_short_char);
    if stem_ok {
        let mut out = [b' '; 11];
        for (i, b) in stem_up.bytes().take(8).enumerate() {
            out[i] = b;
        }
        for (i, b) in ext_up.bytes().take(3).enumerate() {
            out[8 + i] = b;
        }
        return (out, false);
    }
    let mut out = [b' '; 11];
    let cleaned: String = stem
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .take(6)
        .collect();
    let bytes = cleaned.as_bytes();
    out[..bytes.len()].copy_from_slice(bytes);
    out[bytes.len()] = b'~';
    out[bytes.len() + 1] = b'1';
    let ext_c: String = ext
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .take(3)
        .collect();
    for (i, b) in ext_c.bytes().enumerate() {
        out[8 + i] = b;
    }
    (out, true)
}

fn is_short_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}
