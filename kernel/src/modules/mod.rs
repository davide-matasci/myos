//! Runtime loader for in-memory ELF modules.
//!
//! There is no filesystem and no dynamic linker against kernel `.dynsym`.
//! The kernel copies PT_LOAD segments into the heap, applies relative
//! relocs, looks up `module_init`, and calls it with a [`myos_abi::KernelApi`].
//! `elf::image_span` / `elf::realize` are also used to load the userspace
//! `init` ELF (no `module_init`).

pub mod elf;
mod registry;

use crate::console;
use alloc::alloc::{alloc, dealloc, Layout};
use myos_abi::{KernelApi, ABI_VERSION};

const HELLO_IMAGE: &[u8] = include_bytes!(env!("HELLO_MODULE_PATH"));
const FAT_IMAGE: &[u8] = include_bytes!(env!("FAT_MODULE_PATH"));

static API: KernelApi = KernelApi {
    abi_version: ABI_VERSION,
    _reserved: 0,
    write_str: api_write_str,
    alloc: api_alloc,
    dealloc: api_dealloc,
    blk_read: api_blk_read,
    vfs_register: api_vfs_register,
};

/// Load the hello module that was baked into the kernel at build time.
pub fn load_embedded_hello() {
    if let Err(e) = load("hello", HELLO_IMAGE) {
        console::status_fail(&alloc::format!("hello module: {e}"));
    }
}

/// Load the FAT16 module baked into the kernel. Registers `/msg` via
/// `vfs_register` after reading the virtio-blk disk. Failure is logged
/// (`fat mod failed`) and is not a kernel panic.
pub fn load_embedded_fat() {
    if let Err(e) = load("fat", FAT_IMAGE) {
        console::status_fail(&alloc::format!("fat module: {e}"));
    }
}

/// Load modules Limine mapped from `module_path` (ESP `boot/hello`).
///
/// Uses the same ELF loader as the baked-in image. Does not panic on
/// failure: embedded hello already proved the loader.
/// Userspace ELFs in the module list (`MissingInit`) are skipped quietly
/// so bootfs can reuse the same Limine modules.
pub fn load_limine_modules() {
    let Some(resp) = crate::limine_boot::MODULES.response() else {
        return;
    };
    const NAMES: [&str; 4] = [
        "hello-limine",
        "hello-limine-1",
        "hello-limine-2",
        "hello-limine-3",
    ];
    for (i, file) in resp.modules().iter().enumerate() {
        // Limine already mapped `address..address+size`.
        let bytes = file.data();
        let name = NAMES.get(i).copied().unwrap_or("hello-limine");
        match load(name, bytes) {
            Ok(()) => {
                console::status_ok("limine module");
            }
            Err(elf::LoadError::MissingInit) => {}
            Err(e) => {
                console::status_fail(&alloc::format!("limine module: {e}"));
            }
        }
    }
}

/// Load `image` (an ELF file already in memory) and run `module_init`.
pub fn load(name: &'static str, image: &[u8]) -> Result<(), elf::LoadError> {
    let loaded = elf::load(image)?;
    let rc = match loaded.init {
        Some(init) => unsafe { init(&API) },
        None => {
            unsafe { loaded.free() };
            return Err(elf::LoadError::MissingInit);
        }
    };
    if rc != 0 {
        unsafe { loaded.free() };
        return Err(elf::LoadError::InitFailed(rc));
    }
    registry::register(LoadedModule {
        name,
        base: loaded.base as usize,
        size: loaded.size,
        init: loaded.init,
        exit: loaded.exit,
    });
    debug_assert!(by_name(name).is_some());
    let _ = count();
    Ok(())
}

pub use registry::{by_name, count, LoadedModule};

unsafe extern "C" fn api_write_str(ptr: *const u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    for &b in bytes {
        console::write_byte(b);
    }
}

unsafe extern "C" fn api_alloc(size: usize, align: usize) -> *mut u8 {
    if size == 0 {
        return core::ptr::null_mut();
    }
    let Ok(layout) = Layout::from_size_align(size, align.max(1)) else {
        return core::ptr::null_mut();
    };
    unsafe { alloc(layout) }
}

unsafe extern "C" fn api_dealloc(ptr: *mut u8, size: usize, align: usize) {
    if ptr.is_null() || size == 0 {
        return;
    }
    let Ok(layout) = Layout::from_size_align(size, align.max(1)) else {
        return;
    };
    unsafe { dealloc(ptr, layout) }
}

unsafe extern "C" fn api_blk_read(lba: u64, buf: *mut u8, len: usize) -> i32 {
    if len == 0 {
        return 0;
    }
    if buf.is_null() {
        return -1;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(buf, len) };
    match crate::blk::read(lba, slice) {
        Ok(()) => 0,
        Err(()) => -1,
    }
}

unsafe extern "C" fn api_vfs_register(
    name: *const u8,
    name_len: usize,
    data: *const u8,
    data_len: usize,
) -> i32 {
    if name.is_null() || name_len == 0 {
        return -1;
    }
    if data_len != 0 && data.is_null() {
        return -1;
    }
    let name_bytes = unsafe { core::slice::from_raw_parts(name, name_len) };
    let Ok(name) = core::str::from_utf8(name_bytes) else {
        return -1;
    };
    let src: &[u8] = if data_len == 0 {
        &[]
    } else {
        unsafe { core::slice::from_raw_parts(data, data_len) }
    };
    let leaked: &'static [u8] = alloc::boxed::Box::leak(src.to_vec().into_boxed_slice());
    if crate::fs::register("bootfs", name, leaked) {
        0
    } else {
        -1
    }
}
