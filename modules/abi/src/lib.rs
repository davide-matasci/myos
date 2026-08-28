//! Function-pointer table the kernel hands to a loadable module.
//!
//! This is the modular ABI. Modules do not link against kernel `.dynsym`;
//! they receive a [`KernelApi`] from `module_init` and call through it.

#![no_std]

/// Bump this when [`KernelApi`] layout or meaning changes.
pub const ABI_VERSION: u32 = 2;

/// Kernel services visible to a module.
///
/// Layout is frozen by `repr(C)`. New functions are appended; never reorder.
#[repr(C)]
pub struct KernelApi {
    pub abi_version: u32,
    pub _reserved: u32,
    pub write_str: unsafe extern "C" fn(*const u8, usize),
    pub alloc: unsafe extern "C" fn(usize, usize) -> *mut u8,
    pub dealloc: unsafe extern "C" fn(*mut u8, usize, usize),
    pub blk_read: unsafe extern "C" fn(lba: u64, buf: *mut u8, len: usize) -> i32,
    pub vfs_register: unsafe extern "C" fn(
        name: *const u8,
        name_len: usize,
        data: *const u8,
        data_len: usize,
    ) -> i32,
}

/// `module_init` — required. Return 0 on success.
pub type ModuleInit = unsafe extern "C" fn(*const KernelApi) -> i32;

/// `module_exit` — optional cleanup.
pub type ModuleExit = unsafe extern "C" fn();
