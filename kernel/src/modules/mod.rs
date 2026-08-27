//! Runtime loader for in-memory ELF modules.
//!
//! There is no filesystem and no dynamic linker against kernel `.dynsym`.
//! The kernel copies PT_LOAD segments into the heap, applies relative
//! relocs, looks up `module_init`, and calls it with a [`myos_abi::KernelApi`].

mod elf;
mod registry;

use crate::arch::SerialPort;
use alloc::alloc::{alloc, dealloc, Layout};
use core::fmt::Write;
use myos_abi::{KernelApi, ABI_VERSION};

const HELLO_IMAGE: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_HELLO_hello"));

static API: KernelApi = KernelApi {
    abi_version: ABI_VERSION,
    _reserved: 0,
    write_str: api_write_str,
    alloc: api_alloc,
    dealloc: api_dealloc,
};

/// Load the hello module that was baked into the kernel at build time.
pub fn load_embedded_hello() {
    if let Err(e) = load("hello", HELLO_IMAGE) {
        let mut serial = SerialPort::new();
        let _ = writeln!(serial, "mod load failed: {e}");
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
    let mut serial = SerialPort::new();
    for &b in bytes {
        serial.write_byte(b);
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
