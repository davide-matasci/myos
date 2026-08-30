//! Mount a flat read-only namespace at `/disk` via `KernelApi::vfs_mount`.

#![no_std]
#![no_main]

use myos_abi::{KernelApi, ModuleVfsOps, VfsStatInfo, ABI_VERSION};

const FILES: &[(&str, &[u8])] = &[("ping", b"disk ok\n")];

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;

fn find(name: &str) -> Option<&'static [u8]> {
    for (n, data) in FILES {
        if *n == name {
            return Some(*data);
        }
    }
    None
}

unsafe extern "C" fn stub_lookup(
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
    let Some(data) = find(path) else {
        return -1;
    };
    *out_data = data.as_ptr();
    *out_len = data.len();
    0
}

unsafe extern "C" fn stub_stat(path: *const u8, path_len: usize, out: *mut VfsStatInfo) -> i32 {
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
    let Some(data) = find(path) else {
        return -1;
    };
    (*out).mode = S_IFREG | 0o444;
    (*out).size = data.len() as u32;
    (*out).ino = 2;
    (*out).nlink = 1;
    0
}

unsafe extern "C" fn stub_listdir(
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
    let mut n = 0usize;
    for (name, _) in FILES {
        let bytes = name.as_bytes();
        let need = bytes.len() + 1;
        if n + need > buf_len {
            break;
        }
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf.add(n), bytes.len());
        n += bytes.len();
        *buf.add(n) = b'\n';
        n += 1;
    }
    *out_len = n;
    0
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api: *const KernelApi) -> i32 {
    if api.is_null() {
        return -1;
    }
    let api = unsafe { &*api };
    if api.abi_version != ABI_VERSION {
        return -2;
    }
    // Build ops here (not in a static): AArch64 ET_EXEC modules do not relocate
    // fn pointers in .rodata, so kernel callbacks need slide-correct addresses.
    let ops = ModuleVfsOps {
        lookup: stub_lookup,
        stat: stub_stat,
        listdir: stub_listdir,
        register: None,
    };
    let rc = unsafe {
        (api.vfs_mount)(
            b"stubfs".as_ptr(),
            6,
            b"disk".as_ptr(),
            4,
            &ops as *const ModuleVfsOps,
        )
    };
    if rc != 0 {
        return rc;
    }
    let msg = b"stubfs mod ok\n";
    unsafe { (api.write_str)(msg.as_ptr(), msg.len()) };
    0
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
