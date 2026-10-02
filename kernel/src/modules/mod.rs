//! Runtime loader for in-memory ELF modules.
//!
//! There is no dynamic linker against kernel `.dynsym`. The kernel copies
//! PT_LOAD segments into the heap, applies relative relocs, looks up
//! `module_init`, and calls it with a [`myos_abi::KernelApi`]. Boot modules
//! come from Limine's module list (`boot/modules/<name>` in limine.conf,
//! loaded in that order); later ones from `insmod` (`/lib/modules`).
//! `elf::image_span` / `elf::realize_as` are also used to load the userspace
//! `init` ELF (no `module_init`).

pub mod elf;
mod registry;

use crate::console;
use alloc::alloc::{Layout, alloc, dealloc};
use myos_abi::{ABI_VERSION, FsBind, KernelApi, ModuleBlkOps, ModuleChrOps};

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
    proc_register: api_proc_register,
    acpi_rsdp: api_acpi_rsdp,
    hhdm_offset: api_hhdm_offset,
    proc_set_writer: api_proc_set_writer,
    pci_irq_enable: api_pci_irq_enable,
    wake_any: api_wake_any,
    wait_seq: api_wait_seq,
    block_until: api_block_until,
    monotonic_ns: api_monotonic_ns,
    blk_register: api_blk_register,
    pci_find_class: api_pci_find_class,
};

/// Modules that print their own `[ OK ]` line (only when they found a
/// device, or with their own wording); the loader announces the others.
const SELF_REPORTING: &[&str] = &["virtio_blk", "nvme", "virtio_net", "netfs", "pci_enum", "acpi"];

/// Load the modules Limine placed in RAM (`module_path` entries of
/// limine.conf, in order). Each is named after its path's last component.
///
/// Non-module files in the list are skipped: the `initramfs` cpio archive
/// (bootfs parses it) and userspace ELFs (`MissingInit`), so bootfs can reuse
/// the same Limine modules. Failures are logged, never fatal.
pub fn load_limine_modules() {
    let Some(resp) = crate::limine_boot::MODULES.response() else {
        return;
    };
    let mut loaded = 0usize;
    for file in resp.modules().iter() {
        // Limine already mapped `address..address+size`.
        let bytes = file.data();
        let base = file.path().rsplit('/').next().unwrap_or("");
        if base.is_empty() || base == "initramfs" {
            continue;
        }
        // Not an ELF (a stray data file): skip without a status line.
        if bytes.len() < 4 || &bytes[..4] != b"\x7fELF" {
            continue;
        }
        let name: &'static str = alloc::boxed::Box::leak(alloc::string::String::from(base).into_boxed_str());
        match load(name, bytes) {
            Ok(()) => {
                loaded += 1;
                if !SELF_REPORTING.contains(&name) {
                    console::status_ok(name);
                }
            }
            Err(elf::LoadError::MissingInit) => {}
            Err(e) => {
                console::status_fail(&alloc::format!("limine module {name}: {e}"));
            }
        }
    }
    console::status_ok(&alloc::format!("limine modules: {loaded}"));
}

/// `insmod`: load the module at `path` (a plain ELF file, named after the
/// path's last component). Errors: not found / too large, bad image, a module
/// of that name is already loaded, or `module_init` failed.
pub fn insmod(path: &str) -> Result<(), InsmodError> {
    const MODULE_MAX: usize = 4 << 20;
    let base = path.rsplit('/').next().unwrap_or("");
    if base.is_empty() {
        return Err(InsmodError::NotFound);
    }
    if by_name(base).is_some() {
        return Err(InsmodError::Exists);
    }
    let bytes = crate::fs::read_all(path, MODULE_MAX).ok_or(InsmodError::NotFound)?;
    let name: &'static str = alloc::boxed::Box::leak(alloc::string::String::from(base).into_boxed_str());
    load(name, &bytes).map_err(InsmodError::Load)
}

pub enum InsmodError {
    NotFound,
    Exists,
    Load(elf::LoadError),
}

impl core::fmt::Display for InsmodError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotFound => write!(f, "no such file (or larger than 4 MiB)"),
            Self::Exists => write!(f, "a module of that name is already loaded"),
            Self::Load(e) => write!(f, "{e}"),
        }
    }
}

/// `/proc/modules`: one loaded module name per line.
pub fn modules_text() -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::new();
    for m in registry::all() {
        out.extend_from_slice(m.name.as_bytes());
        out.push(b'\n');
    }
    out
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
    registry::register(LoadedModule { name });
    debug_assert!(by_name(name).is_some());
    Ok(())
}

pub use registry::{LoadedModule, by_name};

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
    match crate::mm::alloc_contiguous_pages(n_pages) {
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

unsafe extern "C" fn api_proc_register(
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
    if crate::fs::procfs_register(name, leaked) {
        0
    } else {
        -1
    }
}

unsafe extern "C" fn api_acpi_rsdp() -> usize {
    crate::limine_boot::rsdp_va().unwrap_or(0)
}

unsafe extern "C" fn api_hhdm_offset() -> u64 {
    crate::limine_boot::hhdm_offset()
}

unsafe extern "C" fn api_proc_set_writer(
    name: *const u8,
    name_len: usize,
    writer: Option<unsafe extern "C" fn(*const u8, usize) -> i32>,
) -> i32 {
    if name.is_null() || name_len == 0 {
        return -1;
    }
    let name_bytes = unsafe { core::slice::from_raw_parts(name, name_len) };
    let Ok(name) = core::str::from_utf8(name_bytes) else {
        return -1;
    };
    if crate::fs::procfs_set_writer(name, writer) {
        0
    } else {
        -1
    }
}

unsafe extern "C" fn api_pci_irq_enable(
    bus: u8,
    slot: u8,
    func: u8,
    name: *const u8,
    name_len: usize,
    handler: myos_abi::IrqHandler,
    ctx: *mut core::ffi::c_void,
    msix_entry: *mut u16,
) -> i32 {
    if msix_entry.is_null() {
        return -1;
    }
    let name = if name.is_null() || name_len == 0 {
        "pci"
    } else {
        let bytes = unsafe { core::slice::from_raw_parts(name, name_len) };
        core::str::from_utf8(bytes).unwrap_or("pci")
    };
    let Some(route) = crate::irq::pci_irq_setup(bus, slot, func) else {
        return -1;
    };
    if !crate::irq::register(route.irq, name, handler, ctx as usize) {
        return -1;
    }
    unsafe {
        *msix_entry = route.msix_entry.unwrap_or(myos_abi::MYOS_IRQ_INTX);
    }
    0
}

unsafe extern "C" fn api_wake_any() {
    crate::task::wake_any();
}

unsafe extern "C" fn api_wait_seq() -> u64 {
    crate::task::wait_seq()
}

unsafe extern "C" fn api_block_until(key: usize, seq: u64, deadline_ns: u64) {
    crate::task::block_until(key, seq, deadline_ns);
}

unsafe extern "C" fn api_monotonic_ns() -> u64 {
    crate::time::monotonic_ns()
}

unsafe extern "C" fn api_blk_register(
    name: *const u8,
    name_len: usize,
    ops: *const ModuleBlkOps,
    ctx: usize,
) -> i32 {
    if name.is_null() || name_len == 0 || ops.is_null() {
        return -1;
    }
    let name_bytes = unsafe { core::slice::from_raw_parts(name, name_len) };
    let Ok(name) = core::str::from_utf8(name_bytes) else {
        return -1;
    };
    let ops = unsafe { *ops };
    match crate::blk::register(name, ops, ctx) {
        Some(id) => id as i32,
        None => -1,
    }
}

unsafe extern "C" fn api_pci_find_class(
    class: u8,
    subclass: u8,
    index: u32,
    bus: *mut u8,
    slot: *mut u8,
    func: *mut u8,
) -> i32 {
    if bus.is_null() || slot.is_null() || func.is_null() {
        return -1;
    }
    match crate::pci::find_class(class, subclass, index) {
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
