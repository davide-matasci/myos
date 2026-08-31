//! FAT16 root reader mounted at `/fat` via `KernelApi::vfs_mount`.
//!
//! Root-only, FAT16 only (no subdirs, no FAT32). Also registers `/msg` on
//! bootfs so existing CI (`user/ok` → `fat ok`) keeps working.

#![no_std]
#![no_main]

use myos_abi::{KernelApi, ModuleVfsOps, VfsStatInfo, ABI_VERSION};

const SECTOR: usize = 512;
const MAX_ENTRIES: usize = 32;
const NAME_CAP: usize = 12;
const FILE_CAP: usize = 4096;

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;

struct Entry {
    name: [u8; NAME_CAP],
    name_len: u8,
    cluster: u16,
    size: u32,
    data: [u8; FILE_CAP],
    loaded: bool,
}

struct FatVol {
    ready: bool,
    fat_lba: u64,
    data_lba: u64,
    spc: u8,
    count: u8,
    entries: [Entry; MAX_ENTRIES],
}

static mut VOL: FatVol = FatVol {
    ready: false,
    fat_lba: 0,
    data_lba: 0,
    spc: 0,
    count: 0,
    entries: unsafe { core::mem::MaybeUninit::zeroed().assume_init() },
};

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api: *const KernelApi) -> i32 {
    unsafe {
        if api.is_null() {
            return -1;
        }
        let api = &*api;
        if api.abi_version != ABI_VERSION {
            return -2;
        }
        match run(api) {
            Ok(()) => {
                let msg = b"fat mod ok\n";
                (api.write_str)(msg.as_ptr(), msg.len());
                0
            }
            Err(e) => e,
        }
    }
}

unsafe fn run(api: &KernelApi) -> Result<(), i32> {
    init_volume(api)?;
    let ops = ModuleVfsOps {
        lookup: fat_lookup,
        stat: fat_stat,
        listdir: fat_listdir,
        register: None,
    };
    let rc = (api.vfs_mount)(
        b"fat".as_ptr(),
        3,
        b"fat".as_ptr(),
        3,
        &ops as *const ModuleVfsOps,
    );
    if rc != 0 {
        return Err(rc);
    }
    if let Some(data) = entry_bytes(b"msg") {
        vfs_register(api, b"msg", data)?;
    }
    Ok(())
}

unsafe fn init_volume(api: &KernelApi) -> Result<(), i32> {
    let mut sec = [0u8; SECTOR];
    blk_read(api, 0, &mut sec)?;

    let bps = u16_le(&sec, 11) as usize;
    if bps != SECTOR {
        return Err(-3);
    }
    let spc = sec[13];
    if spc == 0 {
        return Err(-3);
    }
    let reserved = u16_le(&sec, 14) as u64;
    let fats = sec[16];
    if fats == 0 {
        return Err(-3);
    }
    let root_ents = u16_le(&sec, 17) as u32;
    let totsec16 = u16_le(&sec, 19) as u32;
    let fat_sz16 = u16_le(&sec, 22) as u64;
    if fat_sz16 == 0 {
        return Err(-3);
    }
    let _totsec = if totsec16 != 0 {
        totsec16
    } else {
        u32_le(&sec, 32)
    };

    let root_sectors = (root_ents * 32).div_ceil(SECTOR as u32);
    let root_lba = reserved + u64::from(fats) * fat_sz16;
    let data_lba = root_lba + u64::from(root_sectors);

    let vol = &mut *core::ptr::addr_of_mut!(VOL);
    vol.fat_lba = reserved;
    vol.data_lba = data_lba;
    vol.spc = spc;
    vol.count = 0;

    for s in 0..root_sectors {
        blk_read(api, root_lba + u64::from(s), &mut sec)?;
        let mut i = 0;
        while i + 32 <= SECTOR {
            let ent = &sec[i..i + 32];
            if ent[0] == 0 {
                return finish_volume(api);
            }
            i += 32;
            if ent[0] == 0xE5 {
                continue;
            }
            let attr = ent[11];
            if attr == 0x0F || attr & 0x18 != 0 {
                continue;
            }
            let cluster = u16_le(ent, 26);
            let size = u32_le(ent, 28);
            if size == 0 || size as usize > FILE_CAP {
                continue;
            }
            let name_len = short_name_len(&ent[0..8]);
            if name_len == 0 || vol.count as usize >= MAX_ENTRIES {
                continue;
            }
            let idx = vol.count as usize;
            vol.entries[idx].name[..name_len].copy_from_slice(&ent[0..name_len]);
            for b in &mut vol.entries[idx].name[..name_len] {
                *b = b.to_ascii_lowercase();
            }
            vol.entries[idx].name_len = name_len as u8;
            vol.entries[idx].cluster = cluster;
            vol.entries[idx].size = size;
            vol.entries[idx].loaded = false;
            vol.count += 1;
        }
    }
    finish_volume(api)
}

unsafe fn finish_volume(api: &KernelApi) -> Result<(), i32> {
    let vol = &mut *core::ptr::addr_of_mut!(VOL);
    for i in 0..vol.count as usize {
        let cluster = vol.entries[i].cluster;
        let size = vol.entries[i].size as usize;
        let n = read_file(
            api,
            vol.fat_lba,
            vol.data_lba,
            vol.spc,
            cluster,
            size,
            &mut vol.entries[i].data,
        )?;
        if n != size {
            return Err(-5);
        }
        vol.entries[i].loaded = true;
    }
    vol.ready = true;
    Ok(())
}

fn short_name_len(name8: &[u8]) -> usize {
    let mut end = 0usize;
    for (i, &b) in name8.iter().enumerate() {
        if b == b' ' {
            break;
        }
        end = i + 1;
    }
    end
}

fn entry_index(name: &str) -> Option<usize> {
    let vol = unsafe { &*core::ptr::addr_of!(VOL) };
    if !vol.ready {
        return None;
    }
    for i in 0..vol.count as usize {
        let ent = &vol.entries[i];
        let n = ent.name_len as usize;
        if n == name.len() && &ent.name[..n] == name.as_bytes() {
            return Some(i);
        }
    }
    None
}

fn entry_bytes(name: &[u8]) -> Option<&'static [u8]> {
    let name = core::str::from_utf8(name).ok()?;
    let idx = entry_index(name)?;
    let vol = unsafe { &*core::ptr::addr_of!(VOL) };
    let ent = &vol.entries[idx];
    if !ent.loaded {
        return None;
    }
    let len = ent.size as usize;
    Some(unsafe { core::slice::from_raw_parts(ent.data.as_ptr(), len) })
}

unsafe extern "C" fn fat_lookup(
    path: *const u8,
    path_len: usize,
    out_data: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    if path.is_null() || out_data.is_null() || out_len.is_null() {
        return -1;
    }
    let path = match core::str::from_utf8(core::slice::from_raw_parts(path, path_len)) {
        Ok(p) => p,
        Err(_) => return -1,
    };
    let Some(data) = entry_bytes(path.as_bytes()) else {
        return -1;
    };
    *out_data = data.as_ptr();
    *out_len = data.len();
    0
}

unsafe extern "C" fn fat_stat(path: *const u8, path_len: usize, out: *mut VfsStatInfo) -> i32 {
    if out.is_null() {
        return -1;
    }
    let path = match core::str::from_utf8(core::slice::from_raw_parts(path, path_len)) {
        Ok(p) => p,
        Err(_) => return -1,
    };
    if path.is_empty() || path == "." || path == ".." {
        (*out).mode = S_IFDIR | 0o755;
        (*out).size = 0;
        (*out).ino = 1;
        (*out).nlink = 2;
        return 0;
    }
    let Some(idx) = entry_index(path) else {
        return -1;
    };
    let vol = &*core::ptr::addr_of!(VOL);
    let ent = &vol.entries[idx];
    (*out).mode = S_IFREG | 0o444;
    (*out).size = ent.size;
    (*out).ino = (idx as u32) + 2;
    (*out).nlink = 1;
    0
}

unsafe extern "C" fn fat_listdir(
    path: *const u8,
    path_len: usize,
    buf: *mut u8,
    buf_len: usize,
    out_len: *mut usize,
) -> i32 {
    if buf.is_null() || out_len.is_null() {
        return -1;
    }
    let rel = match core::str::from_utf8(core::slice::from_raw_parts(path, path_len)) {
        Ok(p) => p,
        Err(_) => return -1,
    };
    if !rel.is_empty() && rel != "." {
        return -1;
    }
    let vol = &*core::ptr::addr_of!(VOL);
    if !vol.ready {
        return -1;
    }
    let mut n = 0usize;
    for i in 0..vol.count as usize {
        let ent = &vol.entries[i];
        let name = &ent.name[..ent.name_len as usize];
        let need = name.len() + 1;
        if n + need > buf_len {
            break;
        }
        core::ptr::copy_nonoverlapping(name.as_ptr(), buf.add(n), name.len());
        n += name.len();
        *buf.add(n) = b'\n';
        n += 1;
    }
    *out_len = n;
    0
}

unsafe fn read_file(
    api: &KernelApi,
    fat_lba: u64,
    data_lba: u64,
    spc: u8,
    mut cluster: u16,
    file_size: usize,
    out: &mut [u8],
) -> Result<usize, i32> {
    let mut copied = 0usize;
    let mut sec = [0u8; SECTOR];
    while copied < file_size {
        if cluster < 2 || cluster >= 0xFFF8 {
            break;
        }
        let lba = data_lba + u64::from(cluster - 2) * u64::from(spc);
        for s in 0..spc {
            if copied >= file_size {
                break;
            }
            blk_read(api, lba + u64::from(s), &mut sec)?;
            let n = (file_size - copied).min(SECTOR);
            out[copied..copied + n].copy_from_slice(&sec[..n]);
            copied += n;
        }
        cluster = fat_next(api, fat_lba, cluster)?;
    }
    if copied != file_size {
        return Err(-5);
    }
    Ok(copied)
}

unsafe fn fat_next(api: &KernelApi, fat_lba: u64, cluster: u16) -> Result<u16, i32> {
    let off = cluster as u64 * 2;
    let mut sec = [0u8; SECTOR];
    blk_read(api, fat_lba + off / SECTOR as u64, &mut sec)?;
    let e = (off as usize) % SECTOR;
    Ok(u16_le(&sec, e))
}

unsafe fn blk_read(api: &KernelApi, lba: u64, buf: &mut [u8; SECTOR]) -> Result<(), i32> {
    let rc = (api.blk_read)(lba, buf.as_mut_ptr(), SECTOR);
    if rc == 0 {
        Ok(())
    } else {
        Err(-1)
    }
}

unsafe fn vfs_register(api: &KernelApi, name: &[u8], data: &[u8]) -> Result<(), i32> {
    let rc = (api.vfs_register)(name.as_ptr(), name.len(), data.as_ptr(), data.len());
    if rc == 0 {
        Ok(())
    } else {
        Err(-7)
    }
}

fn u16_le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
