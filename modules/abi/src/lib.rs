//! Function-pointer table the kernel hands to a loadable module.
//!
//! This is the modular ABI. Modules do not link against kernel `.dynsym`;
//! they receive a [`KernelApi`] from `module_init` and call through it.

#![no_std]

/// Bump this when [`KernelApi`] layout or meaning changes.
pub const ABI_VERSION: u32 = 4;

/// Stat blob exchanged with module VFS hooks (matches kernel layout).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VfsStatInfo {
    pub mode: u32,
    pub size: u32,
    pub ino: u32,
    pub nlink: u32,
}

/// Module-provided VFS backend hooks. Function pointers may be null only where
/// noted. All paths are relative to the mount prefix (no leading slash).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ModuleVfsOps {
    pub lookup: unsafe extern "C" fn(
        path: *const u8,
        path_len: usize,
        out_data: *mut *const u8,
        out_len: *mut usize,
    ) -> i32,
    pub stat: unsafe extern "C" fn(
        path: *const u8,
        path_len: usize,
        out: *mut VfsStatInfo,
    ) -> i32,
    pub listdir: unsafe extern "C" fn(
        path: *const u8,
        path_len: usize,
        buf: *mut u8,
        buf_len: usize,
        out_len: *mut usize,
    ) -> i32,
    /// Optional; null if the mount is read-only at runtime.
    pub register: Option<
        unsafe extern "C" fn(
            name: *const u8,
            name_len: usize,
            data: *const u8,
            data_len: usize,
        ) -> i32,
    >,
}

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
    /// Register `name` on the bootfs mount (`/name`). Data is copied into a
    /// leaked kernel buffer and remains until reboot.
    pub vfs_register: unsafe extern "C" fn(
        name: *const u8,
        name_len: usize,
        data: *const u8,
        data_len: usize,
    ) -> i32,
    /// Register on bootfs without copying (`data` must outlive the kernel).
    pub vfs_register_static: unsafe extern "C" fn(
        name: *const u8,
        name_len: usize,
        data: *const u8,
        data_len: usize,
    ) -> i32,
    /// Attach a module VFS backend at `/prefix/…`.
    pub vfs_mount: unsafe extern "C" fn(
        name: *const u8,
        name_len: usize,
        prefix: *const u8,
        prefix_len: usize,
        ops: *const ModuleVfsOps,
    ) -> i32,
}

/// `module_init` — required. Return 0 on success.
pub type ModuleInit = unsafe extern "C" fn(*const KernelApi) -> i32;

/// `module_exit` — optional cleanup.
pub type ModuleExit = unsafe extern "C" fn();
