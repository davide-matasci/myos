//! FAT16 root reader. Registers fstype `"fat"`; `mount(2)` binds a blk id.
//!
//! Root-only, FAT16 only (no subdirs, no FAT32). Does not automount.
//! `/msg` on bootfs is provided by the kernel, not this module.
//!
//! One volume, read whole at bind time and kept behind a lock that a bind
//! or a file hook holds for its whole run; a hook copies out what it
//! returns, so nothing of the volume outlives the lock (`lookup` always
//! fails, the bytes go through `read`).

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use myos_abi::{ApiCell, ABI_VERSION, KernelApi, Lock, ModuleVfsOps, VfsStatInfo};

const SECTOR: usize = 512;
const MAX_ENTRIES: usize = 32;
const NAME_CAP: usize = 12;
const FILE_CAP: usize = 4096;

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;

#[repr(C)]
struct Entry {
    name: [u8; NAME_CAP],
    name_len: u8,
    cluster: u16,
    size: u32,
    data: [u8; FILE_CAP],
    loaded: bool,
}

impl Entry {
    const EMPTY: Self = Self {
        name: [0; NAME_CAP],
        name_len: 0,
        cluster: 0,
        size: 0,
        data: [0; FILE_CAP],
        loaded: false,
    };
}

#[repr(C)]
struct FatVol {
    ready: bool,
    dev: u32,
    fat_lba: u64,
    data_lba: u64,
    spc: u8,
    count: u8,
    entries: [Entry; MAX_ENTRIES],
}

static VOL: Lock<FatVol> = Lock::new(FatVol {
    ready: false,
    dev: 0,
    fat_lba: 0,
    data_lba: 0,
    spc: 0,
    count: 0,
    entries: [Entry::EMPTY; MAX_ENTRIES],
});

static API: ApiCell = ApiCell::new();

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api: *const KernelApi) -> i32 {
    let Some(api) = (unsafe { api.as_ref() }) else {
        return -1;
    };
    if api.abi_version != ABI_VERSION {
        return -2;
    }
    unsafe { API.set(api) };
    match run(api) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

fn run(api: &KernelApi) -> Result<(), i32> {
    let rc = api.fs_register("fat", fat_bind);
    if rc != 0 {
        return Err(rc);
    }
    Ok(())
}

unsafe extern "C" fn fat_bind(dev_id: u32, ops: *mut ModuleVfsOps) -> i32 {
    if ops.is_null() {
        return -1;
    }
    let Some(api) = API.try_get() else {
        return -1;
    };
    let mut vol = VOL.lock();
    // Re-bind is allowed: CI may mount twice, and /ok retries every vd*.
    vol.ready = false;
    vol.count = 0;
    match init_volume(api, &mut vol, dev_id) {
        Ok(()) => {
            unsafe {
                *ops = ModuleVfsOps {
                    lookup: fat_lookup,
                    stat: fat_stat,
                    listdir: fat_listdir,
                    register: None,
                    read: Some(fat_read),
                    write: None,
                    create: None,
                    truncate: None,
                    mkdir: None,
                    rmdir: None,
                    unlink: None,
                    rename: None,
                    symlink: None,
                    readlink: None,
                    release: None,
                    mmap: None,
                    poll: None,
                    open: None,
                    set_times: None,
                    unmount: None,
                    unlink_keep: None,
                    read_ino: None,
                    write_ino: None,
                    stat_ino: None,
                    forget_ino: None,
                    set_size: None,
                    set_size_ino: None,
                    file_id: None,
                    set_times_ino: None,
                };
            }
            0
        }
        Err(e) => e,
    }
}

fn init_volume(api: &KernelApi, vol: &mut FatVol, dev: u32) -> Result<(), i32> {
    let mut sec = [0u8; SECTOR];
    blk_read(api, dev, 0, &mut sec)?;

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

    vol.dev = dev;
    vol.fat_lba = reserved;
    vol.data_lba = data_lba;
    vol.spc = spc;
    vol.count = 0;

    for s in 0..root_sectors {
        blk_read(api, dev, root_lba + u64::from(s), &mut sec)?;
        let mut i = 0;
        while i + 32 <= SECTOR {
            let ent = &sec[i..i + 32];
            if ent[0] == 0 {
                return finish_volume(api, vol);
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
    finish_volume(api, vol)
}

fn finish_volume(api: &KernelApi, vol: &mut FatVol) -> Result<(), i32> {
    let nent = (vol.count as usize).min(MAX_ENTRIES);
    vol.count = nent as u8;
    for i in 0..nent {
        let cluster = vol.entries[i].cluster;
        let size = vol.entries[i].size as usize;
        let n = read_file(
            api,
            vol.dev,
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

fn entry_index(vol: &FatVol, name: &str) -> Option<usize> {
    if !vol.ready {
        return None;
    }
    let nent = (vol.count as usize).min(MAX_ENTRIES);
    for i in 0..nent {
        let ent = &vol.entries[i];
        let n = ent.name_len as usize;
        if n == 0 || n > NAME_CAP {
            continue;
        }
        if n == name.len() && &ent.name[..n] == name.as_bytes() {
            return Some(i);
        }
    }
    None
}

/// The bytes of the file `name`, once `finish_volume` read them.
fn entry_bytes<'a>(vol: &'a FatVol, name: &str) -> Option<&'a [u8]> {
    let ent = &vol.entries[entry_index(vol, name)?];
    if !ent.loaded {
        return None;
    }
    let len = (ent.size as usize).min(FILE_CAP);
    Some(&ent.data[..len])
}

unsafe extern "C" fn fat_lookup(_: *const u8, _: usize, _: *mut *const u8, _: *mut usize) -> i32 {
    -1
}

unsafe extern "C" fn fat_read(path: *const u8, path_len: usize, pos: usize, buf: *mut u8, cap: usize) -> i32 {
    if path.is_null() || buf.is_null() {
        return -1;
    }
    let path = match core::str::from_utf8(unsafe { core::slice::from_raw_parts(path, path_len) }) {
        Ok(p) => p,
        Err(_) => return -1,
    };
    let vol = VOL.lock();
    let Some(data) = entry_bytes(&vol, path) else {
        return -1;
    };
    let n = cap.min(data.len().saturating_sub(pos));
    if n != 0 {
        unsafe { core::ptr::copy_nonoverlapping(data[pos..].as_ptr(), buf, n) };
    }
    n as i32
}

unsafe extern "C" fn fat_stat(path: *const u8, path_len: usize, out: *mut VfsStatInfo) -> i32 {
    let Some(out) = (unsafe { out.as_mut() }) else {
        return -1;
    };
    let path = match core::str::from_utf8(unsafe { core::slice::from_raw_parts(path, path_len) }) {
        Ok(p) => p,
        Err(_) => return -1,
    };
    if path.is_empty() || path == "." || path == ".." {
        out.mode = S_IFDIR | 0o755;
        out.size = 0;
        out.ino = 1;
        out.nlink = 2;
        return 0;
    }
    let vol = VOL.lock();
    let Some(idx) = entry_index(&vol, path) else {
        return -1;
    };
    let ent = &vol.entries[idx];
    out.mode = S_IFREG | 0o444;
    out.size = ent.size;
    out.ino = (idx as u32) + 2;
    out.nlink = 1;
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
    let rel = match core::str::from_utf8(unsafe { core::slice::from_raw_parts(path, path_len) }) {
        Ok(p) => p,
        Err(_) => return -1,
    };
    if !rel.is_empty() && rel != "." {
        return -1;
    }
    let vol = VOL.lock();
    if !vol.ready {
        return -1;
    }
    let out = unsafe { core::slice::from_raw_parts_mut(buf, buf_len) };
    let mut n = 0usize;
    let nent = (vol.count as usize).min(MAX_ENTRIES);
    for i in 0..nent {
        let ent = &vol.entries[i];
        let name_len = ent.name_len as usize;
        if name_len == 0 || name_len > NAME_CAP {
            continue;
        }
        let name = &ent.name[..name_len];
        let need = name.len() + 1;
        if n + need > out.len() {
            break;
        }
        out[n..n + name.len()].copy_from_slice(name);
        n += name.len();
        out[n] = b'\n';
        n += 1;
    }
    unsafe { *out_len = n };
    0
}

fn read_file(
    api: &KernelApi,
    dev: u32,
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
            blk_read(api, dev, lba + u64::from(s), &mut sec)?;
            let n = (file_size - copied).min(SECTOR);
            out[copied..copied + n].copy_from_slice(&sec[..n]);
            copied += n;
        }
        cluster = fat_next(api, dev, fat_lba, cluster)?;
    }
    if copied != file_size {
        return Err(-5);
    }
    Ok(copied)
}

fn fat_next(api: &KernelApi, dev: u32, fat_lba: u64, cluster: u16) -> Result<u16, i32> {
    let off = cluster as u64 * 2;
    let mut sec = [0u8; SECTOR];
    blk_read(api, dev, fat_lba + off / SECTOR as u64, &mut sec)?;
    let e = (off as usize) % SECTOR;
    Ok(u16_le(&sec, e))
}

fn blk_read(api: &KernelApi, dev: u32, lba: u64, buf: &mut [u8; SECTOR]) -> Result<(), i32> {
    let rc = api.blk_read(dev, lba, buf);
    if rc == 0 { Ok(()) } else { Err(-1) }
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
