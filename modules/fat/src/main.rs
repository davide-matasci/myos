//! FAT16 root reader. Speaks only through [`myos_abi::KernelApi`].
//!
//! Root-only, FAT16 only (no subdirs, no FAT32). Registers `/msg` from
//! the 8.3 file `MSG`.

#![no_std]
#![no_main]

use myos_abi::{KernelApi, ABI_VERSION};

const SECTOR: usize = 512;
const MSG_NAME: &[u8; 8] = b"MSG     ";

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
            Ok(()) => 0,
            Err(e) => e,
        }
    }
}

unsafe fn run(api: &KernelApi) -> Result<(), i32> {
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

    let (first_cluster, file_size) = find_msg(api, root_lba, root_sectors)?;
    if file_size == 0 {
        vfs_register(api, b"msg", &[])?;
        return Ok(());
    }
    if file_size > 4096 {
        return Err(-6);
    }

    let mut file = [0u8; 4096];
    let n = read_file(
        api,
        reserved,
        data_lba,
        spc,
        first_cluster,
        file_size,
        &mut file,
    )?;
    vfs_register(api, b"msg", &file[..n])?;
    Ok(())
}

unsafe fn find_msg(
    api: &KernelApi,
    root_lba: u64,
    root_sectors: u32,
) -> Result<(u16, usize), i32> {
    let mut sec = [0u8; SECTOR];
    for s in 0..root_sectors {
        blk_read(api, root_lba + u64::from(s), &mut sec)?;
        let mut i = 0;
        while i + 32 <= SECTOR {
            let ent = &sec[i..i + 32];
            if ent[0] == 0 {
                return Err(-4);
            }
            i += 32;
            if ent[0] == 0xE5 {
                continue;
            }
            let attr = ent[11];
            if attr == 0x0F || attr & 0x18 != 0 {
                continue;
            }
            if !match_msg(&ent[0..8]) {
                continue;
            }
            let cluster = u16_le(ent, 26);
            let size = u32_le(ent, 28) as usize;
            return Ok((cluster, size));
        }
    }
    Err(-4)
}

fn match_msg(name8: &[u8]) -> bool {
    if name8.len() < 8 {
        return false;
    }
    for i in 0..8 {
        if name8[i].to_ascii_uppercase() != MSG_NAME[i] {
            return false;
        }
    }
    true
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
