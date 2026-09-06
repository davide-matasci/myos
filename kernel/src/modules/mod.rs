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
use alloc::alloc::{Layout, alloc, dealloc};
use myos_abi::{ABI_VERSION, FsBind, KernelApi, ModuleChrOps};

const HELLO_IMAGE: &[u8] = include_bytes!(env!("HELLO_MODULE_PATH"));
const FAT_IMAGE: &[u8] = include_bytes!(env!("FAT_MODULE_PATH"));
const STUBFS_IMAGE: &[u8] = include_bytes!(env!("STUBFS_MODULE_PATH"));
const EXT2_IMAGE: &[u8] = include_bytes!(env!("EXT2_MODULE_PATH"));
const VIRTIO_NET_IMAGE: &[u8] = include_bytes!(env!("VIRTIO_NET_MODULE_PATH"));
const NETFS_IMAGE: &[u8] = include_bytes!(env!("NETFS_MODULE_PATH"));

static API: KernelApi = KernelApi {
    abi_version: ABI_VERSION,
    _reserved: 0,
    write_str: api_write_str,
    alloc: api_alloc,
    dealloc: api_dealloc,
    blk_read: api_blk_read,
    vfs_register: api_vfs_register,
    vfs_register_static: api_vfs_register_static,
    vfs_mount: api_vfs_mount,
    blk_write: api_blk_write,
    blk_count: api_blk_count,
    fs_register: api_fs_register,
    blk_read_at: api_blk_read_at,
    blk_write_at: api_blk_write_at,
    pci_cfg_read32: api_pci_cfg_read32,
    pci_cfg_write32: api_pci_cfg_write32,
    pci_enable: api_pci_enable,
    pci_find: api_pci_find,
    pci_bar_map: api_pci_bar_map,
    dma_alloc: api_dma_alloc,
    dev_register: api_dev_register,
    copy_to_user: api_copy_to_user,
};

/// Load the hello module that was baked into the kernel at build time.
pub fn load_embedded_hello() {
    match load("hello", HELLO_IMAGE) {
        Ok(()) => console::status_ok("hello"),
        Err(e) => console::status_fail(&alloc::format!("hello module: {e}")),
    }
}

/// Load the stubfs module (registers `/disk` via vfs_mount).
pub fn load_embedded_stubfs() {
    match load("stubfs", STUBFS_IMAGE) {
        Ok(()) => console::status_ok("stubfs"),
        Err(e) => console::status_fail(&alloc::format!("stubfs module: {e}")),
    }
}

/// Load the FAT16 module baked into the kernel. Registers fstype `"fat"`;
/// userspace `mount` binds a disk. Failure is logged and is not a panic.
pub fn load_embedded_fat() {
    match load("fat", FAT_IMAGE) {
        Ok(()) => console::status_ok("fat"),
        Err(e) => console::status_fail(&alloc::format!("fat module: {e}")),
    }
}

/// Load the ext2 module baked into the kernel. Registers fstype `"ext2"`;
/// userspace `mount` binds a disk after `mkfs.ext2`. Failure is logged.
pub fn load_embedded_ext2() {
    match load("ext2", EXT2_IMAGE) {
        Ok(()) => console::status_ok("ext2"),
        Err(e) => console::status_fail(&alloc::format!("ext2 module: {e}")),
    }
}

/// Load the virtio-net module. Probes virtio-pci and registers `/dev/netN`.
pub fn load_embedded_virtio_net() {
    // Module self-reports `[ OK ] virtio-net` only when a NIC is registered.
    if let Err(e) = load("virtio_net", VIRTIO_NET_IMAGE) {
        console::status_fail(&alloc::format!("virtio-net module: {e}"));
    }
}

/// Load netfs: Plan 9 `/net` plus `/dev/netd` channel for userspace netd.
pub fn load_embedded_netfs() {
    // Module self-reports `[ OK ] netfs` only when both hooks register.
    if let Err(e) = load("netfs", NETFS_IMAGE) {
        console::status_fail(&alloc::format!("netfs module: {e}"));
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
        // The initramfs module is a newc cpio archive, not a kernel module:
        // fs::init_limine (bootfs) parses it and registers userspace into the
        // /bin and /lib mounts. Skip it here so it is not probed as ELF.
        if file.path().rsplit('/').next() == Some("initramfs") {
            continue;
        }
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

pub use registry::{LoadedModule, by_name, count};

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

unsafe extern "C" fn api_blk_read(dev: u32, lba: u64, buf: *mut u8, len: usize) -> i32 {
    if len == 0 {
        return 0;
    }
    if buf.is_null() {
        return -1;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(buf, len) };
    match crate::blk::read(dev, lba, slice) {
        Ok(()) => 0,
        Err(()) => -1,
    }
}

unsafe extern "C" fn api_blk_write(dev: u32, lba: u64, buf: *const u8, len: usize) -> i32 {
    if len == 0 {
        return 0;
    }
    if buf.is_null() {
        return -1;
    }
    let slice = unsafe { core::slice::from_raw_parts(buf, len) };
    match crate::blk::write(dev, lba, slice) {
        Ok(()) => 0,
        Err(()) => -1,
    }
}

unsafe extern "C" fn api_blk_count() -> u32 {
    crate::blk::count()
}

unsafe extern "C" fn api_blk_read_at(dev: u32, offset: u64, buf: *mut u8, len: usize) -> i32 {
    if len == 0 {
        return 0;
    }
    if buf.is_null() {
        return -1;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(buf, len) };
    match crate::blk::read_bytes(dev, offset, slice) {
        Ok(n) => n.min(i32::MAX as usize) as i32,
        Err(()) => -1,
    }
}

unsafe extern "C" fn api_blk_write_at(dev: u32, offset: u64, buf: *const u8, len: usize) -> i32 {
    if len == 0 {
        return 0;
    }
    if buf.is_null() {
        return -1;
    }
    let slice = unsafe { core::slice::from_raw_parts(buf, len) };
    match crate::blk::write_bytes(dev, offset, slice) {
        Ok(n) => n.min(i32::MAX as usize) as i32,
        Err(()) => -1,
    }
}

unsafe extern "C" fn api_fs_register(name: *const u8, name_len: usize, bind: FsBind) -> i32 {
    if name.is_null() || name_len == 0 {
        return -1;
    }
    let name_bytes = unsafe { core::slice::from_raw_parts(name, name_len) };
    let Ok(name) = core::str::from_utf8(name_bytes) else {
        return -1;
    };
    if crate::fs::register_fstype(name, bind) {
        0
    } else {
        -1
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

unsafe extern "C" fn api_vfs_register_static(
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
    let bytes: &'static [u8] = if data_len == 0 {
        &[]
    } else {
        unsafe { core::slice::from_raw_parts(data, data_len) }
    };
    if crate::fs::register_static("bootfs", name, bytes) {
        0
    } else {
        -1
    }
}

unsafe extern "C" fn api_vfs_mount(
    name: *const u8,
    name_len: usize,
    prefix: *const u8,
    prefix_len: usize,
    ops: *const myos_abi::ModuleVfsOps,
) -> i32 {
    if name.is_null() || name_len == 0 || prefix.is_null() || ops.is_null() {
        return -1;
    }
    let name_bytes = unsafe { core::slice::from_raw_parts(name, name_len) };
    let prefix_bytes = unsafe { core::slice::from_raw_parts(prefix, prefix_len) };
    let Ok(name) = core::str::from_utf8(name_bytes) else {
        return -1;
    };
    let Ok(prefix) = core::str::from_utf8(prefix_bytes) else {
        return -1;
    };
    let ops = unsafe { *ops };
    if ops.lookup as usize == 0 {
        return -1;
    }
    if crate::fs::mount_module(name, prefix, ops) {
        0
    } else {
        -1
    }
}

unsafe extern "C" fn api_pci_cfg_read32(bus: u8, slot: u8, func: u8, off: u8) -> u32 {
    crate::pci::cfg_read32(bus, slot, func, off)
}

unsafe extern "C" fn api_pci_cfg_write32(bus: u8, slot: u8, func: u8, off: u8, val: u32) {
    crate::pci::cfg_write32(bus, slot, func, off, val)
}

unsafe extern "C" fn api_pci_enable(bus: u8, slot: u8, func: u8) {
    crate::pci::enable(bus, slot, func)
}

unsafe extern "C" fn api_pci_find(
    vendor: u16,
    device: u16,
    index: u32,
    bus: *mut u8,
    slot: *mut u8,
    func: *mut u8,
) -> i32 {
    if bus.is_null() || slot.is_null() || func.is_null() {
        return -1;
    }
    match crate::pci::find(vendor, device, index) {
        Some(bdf) => {
            unsafe {
                *bus = bdf.bus;
                *slot = bdf.slot;
                *func = bdf.func;
            }
            0
        }
        None => -1,
    }
}

unsafe extern "C" fn api_pci_bar_map(
    bus: u8,
    slot: u8,
    func: u8,
    bar: u8,
    va: *mut usize,
    size: *mut u64,
) -> i32 {
    if va.is_null() || size.is_null() {
        return -1;
    }
    match crate::pci::bar_map(bus, slot, func, bar) {
        Some((mapped, sz)) => {
            unsafe {
                *va = mapped;
                *size = sz;
            }
            0
        }
        None => -1,
    }
}

unsafe extern "C" fn api_dma_alloc(n_pages: usize, phys: *mut u64) -> *mut u8 {
    if phys.is_null() {
        return core::ptr::null_mut();
    }
    match crate::blk::virtq::alloc_pages(n_pages) {
        Some((p, va)) => {
            unsafe {
                *phys = p;
            }
            va
        }
        None => core::ptr::null_mut(),
    }
}

unsafe extern "C" fn api_dev_register(
    name: *const u8,
    name_len: usize,
    ops: *const ModuleChrOps,
) -> i32 {
    if name.is_null() || name_len == 0 || ops.is_null() {
        return -1;
    }
    let name_bytes = unsafe { core::slice::from_raw_parts(name, name_len) };
    let Ok(name) = core::str::from_utf8(name_bytes) else {
        return -1;
    };
    let ops = unsafe { *ops };
    if crate::fs::register_chrdev(name, ops) {
        0
    } else {
        -1
    }
}

unsafe extern "C" fn api_copy_to_user(dst_user: usize, src: *const u8, len: usize) -> i32 {
    if dst_user == 0 || src.is_null() {
        return -1;
    }
    if len == 0 {
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts(src, len) };
    let aspace = crate::task::current_aspace();
    if crate::user::copy_to_user(aspace, dst_user, slice) {
        0
    } else {
        -1
    }
}
