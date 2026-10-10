//! Runtime loader for in-memory ELF modules.
//!
//! There is no dynamic linker against kernel `.dynsym`. The kernel copies
//! PT_LOAD segments into the heap, applies relative relocs, looks up
//! `module_init`, and calls it with a [`myos_abi::KernelApi`]. Boot modules
//! are the initramfs's `/lib/modules/<name>` that `/lib/modules/boot.list`
//! names, loaded in that order; later ones come from `insmod`.
//! `elf::image_span` / `elf::realize_as` are also used to load the userspace
//! `init` ELF (no `module_init`).

pub mod elf;
mod registry;
mod services;
mod threads;

use crate::console;
use alloc::alloc::{Layout, alloc, dealloc};
use myos_abi::{
    ABI_VERSION, FramebufferInfo, FsBind, KernelApi, ModuleBlkOps, ModuleChrOps, ModuleConsoleOps,
    PathStat, PersonalityOps, StrRef,
};

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
    framebuffer_info: api_framebuffer_info,
    console_register: api_console_register,
    personality_register: api_personality_register,
    personality_exec: api_personality_exec,
    native_syscall: api_native_syscall,
    copy_from_user: api_copy_from_user,
    user_buffer_ok: api_user_buffer_ok,
    current_tid: api_current_tid,
    current_pid: api_current_pid,
    current_ppid: api_current_ppid,
    task_is_live_user: api_task_is_live_user,
    task_has_root: api_task_has_root,
    path_resolve: api_path_resolve,
    vfs_stat: api_vfs_stat,
    vfs_listdir: api_vfs_listdir,
    vfs_mkdir: api_vfs_mkdir,
    vfs_rmdir: api_vfs_rmdir,
    vfs_unlink: api_vfs_unlink,
    vfs_rename: api_vfs_rename,
    vfs_symlink: api_vfs_symlink,
    vfs_readlink: api_vfs_readlink,
    open_path: api_open_path,
    chdir_path: api_chdir_path,
    fd_pread: api_fd_pread,
    fd_kind: api_fd_kind,
    fd_poll_bits: api_fd_poll_bits,
    fd_dup_min: api_fd_dup_min,
    fd_dup2: api_fd_dup2,
    fd_close: api_fd_close,
    fd_write: api_fd_write,
    pipe_open: api_pipe_open,
    mmap: api_mmap,
    signal_get_action: api_signal_get_action,
    signal_set_action: api_signal_set_action,
    signal_blocked: api_signal_blocked,
    signal_set_blocked: api_signal_set_blocked,
    signal_pending: api_signal_pending,
    signal_take: api_signal_take,
    signal_kill: api_signal_kill,
    signal_sigsuspend: api_signal_sigsuspend,
    signal_interrupt_wait: api_signal_interrupt_wait,
    signal_terminate: api_signal_terminate,
    fpu_save: api_fpu_save,
    fpu_restore: api_fpu_restore,
    thread_pointer_get: api_thread_pointer_get,
    thread_pointer_set: api_thread_pointer_set,
    thread_spawn_from: api_thread_spawn_from,
    thread_exit: api_thread_exit,
    wait_addr: api_wait_addr,
    wake_addr: api_wake_addr,
    task_sleep_until: api_task_sleep_until,
    task_yield: api_task_yield,
    wall_time_us: api_wall_time_us,
    rng_fill: api_rng_fill,
    dt_mmio_find: api_dt_mmio_find,
    vfs_read: api_vfs_read,
    vfs_write: api_vfs_write,
    tty_ctl_read: api_tty_ctl_read,
    tty_ctl_write: api_tty_ctl_write,
    fd_path: api_fd_path,
    blk_unregister: api_blk_unregister,
    service_register: api_service_register,
    service_lookup: api_service_lookup,
    thread_spawn: api_thread_spawn,
    wake: api_wake,
    fork_from: api_fork_from,
    mmap_discard: api_mmap_discard,
    vfs_set_times: api_vfs_set_times,
    thread_place: api_thread_place,
    fd_lockctl: api_fd_lockctl,
    power_register: api_power_register,
    current_uid: api_current_uid,
    irq_enable: api_irq_enable,
    console_input: api_console_input,
    vfs_statfs: api_vfs_statfs,
};

/// Modules that print their own `[ OK ]` line (only when they found a
/// device, or with their own wording); the loader announces the others.
const SELF_REPORTING: &[&str] = &["console", "virtio_blk", "nvme", "virtio_net", "netfs", "pci_enum", "acpi", "xhci", "usb_hub", "usb_storage"];

/// Load the boot modules: the names in the initramfs's `/lib/modules/boot.list`
/// (one per line), each from `/lib/modules/<name>`, in that order. Called
/// once the initramfs is unpacked; the boot drive holds only the kernel and
/// the initramfs. Failures are logged, never fatal.
pub fn load_boot_modules() {
    let Some(list) = crate::fs::read_all(BOOT_LIST, 4096) else {
        console::status_fail(&alloc::format!("modules: no {BOOT_LIST}"));
        return;
    };
    let mut loaded = 0usize;
    for name in core::str::from_utf8(&list).unwrap_or("").lines().map(str::trim) {
        if name.is_empty() {
            continue;
        }
        match insmod(&alloc::format!("/lib/modules/{name}")) {
            Ok(()) => {
                loaded += 1;
                if !SELF_REPORTING.contains(&name) {
                    console::status_ok(name);
                }
            }
            Err(e) => console::status_fail(&alloc::format!("module {name}: {e}")),
        }
    }
    console::status_ok(&alloc::format!("boot modules: {loaded}"));
}

/// The boot modules, in load order ([`load_boot_modules`]).
const BOOT_LIST: &str = "/lib/modules/boot.list";

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

/// Load `image` (an ELF file already in memory) and run `module_init`. The
/// module is in the registry while its init runs, so what it registers is
/// counted against it; a failed init takes it out again.
pub fn load(name: &'static str, image: &[u8]) -> Result<(), elf::LoadError> {
    let loaded = elf::load(image)?;
    let Some(init) = loaded.init else {
        unsafe { loaded.free() };
        return Err(elf::LoadError::MissingInit);
    };
    registry::register(LoadedModule {
        name,
        base: loaded.base as usize,
        size: loaded.size,
        exit: loaded.exit,
        rescan: loaded.rescan,
        registrations: 0,
    });
    let rc = registry::as_module(name, || unsafe { init(&API) });
    if rc != 0 {
        // Whatever it registered before failing stays (there is no
        // unregister); the image goes only when it registered nothing.
        if let Ok(Some(_)) = registry::remove_if_free(name) {
            unsafe { loaded.free() };
        }
        return Err(elf::LoadError::InitFailed(rc));
    }
    debug_assert!(by_name(name).is_some());
    Ok(())
}

/// `rmmod`: unload the module `name` when nothing it registered is in
/// place: run its `module_exit` and free its image.
pub fn rmmod(name: &str) -> Result<(), RmmodError> {
    let module = match registry::remove_if_free(name) {
        Ok(Some(m)) => m,
        Ok(None) => return Err(RmmodError::NotLoaded),
        Err(n) => return Err(RmmodError::Busy(n)),
    };
    if let Some(exit) = module.exit {
        registry::as_module(module.name, || unsafe { exit() });
    }
    unsafe { elf::free_image(module.base as *mut u8, module.size) };
    Ok(())
}

pub enum RmmodError {
    NotLoaded,
    Busy(u32),
}

impl core::fmt::Display for RmmodError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotLoaded => write!(f, "no module of that name is loaded"),
            Self::Busy(n) => write!(f, "in use: {n} registration(s) (a device, a filesystem, a mount, ...)"),
        }
    }
}

/// After a `rescan` of `/proc/pci`: every module with a `module_rescan`
/// probes for devices that appeared since its init and registers them.
pub fn rescan_all() {
    for m in registry::all() {
        if let Some(rescan) = m.rescan {
            registry::as_module(m.name, || unsafe { rescan() });
        }
    }
}

/// A registration the running module's call just made.
fn noted(ok: bool) -> i32 {
    if ok {
        registry::note_registration();
        0
    } else {
        -1
    }
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
    noted(crate::fs::register_fstype(name, bind))
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
    noted(crate::fs::register("rootfs", name, leaked))
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
    noted(crate::fs::register_static("rootfs", name, bytes))
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
    noted(crate::fs::mount_module(name, prefix, ops))
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
    noted(crate::fs::register_chrdev(name, ops))
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
    if crate::user::buffer_ok(dst_user, len) && crate::user::copy_to_user(aspace, dst_user, slice) {
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
    noted(crate::fs::procfs_register(name, leaked))
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
    // Clearing a writer provides nothing; setting one does.
    let ok = crate::fs::procfs_set_writer(name, writer);
    if ok && writer.is_some() {
        registry::note_registration();
    }
    if ok { 0 } else { -1 }
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
    registry::note_registration();
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
        Some(id) => {
            registry::note_registration();
            id as i32
        }
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

unsafe extern "C" fn api_framebuffer_info(out: *mut FramebufferInfo) -> i32 {
    if out.is_null() {
        return -1;
    }
    match console::framebuffer_info() {
        Some(info) => {
            unsafe { *out = info };
            0
        }
        None => -1,
    }
}

unsafe extern "C" fn api_console_register(ops: *const ModuleConsoleOps) -> i32 {
    if ops.is_null() {
        return -1;
    }
    let ops = unsafe { *ops };
    noted(console::register(ops))
}

// ---- ABI 13: personalities and the services a syscall layer needs ----------

/// A kernel-memory string argument.
fn str_ref<'a>(s: StrRef) -> Option<&'a str> {
    if s.ptr.is_null() {
        return None;
    }
    let bytes = unsafe { core::slice::from_raw_parts(s.ptr, s.len) };
    core::str::from_utf8(bytes).ok()
}

fn put_str(out: *mut u8, cap: usize, s: &[u8]) -> i32 {
    if out.is_null() || s.len() > cap {
        return -1;
    }
    unsafe { core::ptr::copy_nonoverlapping(s.as_ptr(), out, s.len()) };
    s.len() as i32
}

unsafe extern "C" fn api_personality_register(ops: *const PersonalityOps) -> i32 {
    if ops.is_null() {
        return -1;
    }
    noted(crate::personality::register(unsafe { *ops }))
}

unsafe extern "C" fn api_personality_exec(
    path: StrRef,
    argv: *const StrRef,
    argc: usize,
    envp: *const StrRef,
    envc: usize,
) -> usize {
    let Some(path) = str_ref(path) else {
        return myos_abi::MYOS_SYSERR;
    };
    let strs = |p: *const StrRef, n: usize| -> alloc::vec::Vec<&[u8]> {
        if p.is_null() || n == 0 {
            return alloc::vec::Vec::new();
        }
        unsafe { core::slice::from_raw_parts(p, n) }
            .iter()
            .map(|s| if s.ptr.is_null() { &[][..] } else { unsafe { core::slice::from_raw_parts(s.ptr, s.len) } })
            .collect()
    };
    let args = strs(argv, argc);
    let env = strs(envp, envc);
    crate::personality::exec_with(path, &args, &env)
}

unsafe extern "C" fn api_native_syscall(nr: usize, a0: usize, a1: usize, a2: usize, regs: *mut u64) -> usize {
    let mut regs = crate::user::SyscallRegs::from_ptr(regs);
    crate::user::native_dispatch(nr, a0, a1, a2, &mut regs)
}

unsafe extern "C" fn api_copy_from_user(src_user: usize, dst: *mut u8, len: usize) -> i32 {
    if dst.is_null() {
        return -1;
    }
    if len == 0 {
        return 0;
    }
    let out = unsafe { core::slice::from_raw_parts_mut(dst, len) };
    if crate::user::buffer_ok(src_user, len) && crate::user::copy_from_user(crate::task::current_aspace(), src_user, out) {
        0
    } else {
        -1
    }
}

unsafe extern "C" fn api_user_buffer_ok(ptr: usize, len: usize) -> i32 {
    i32::from(crate::user::buffer_ok(ptr, len))
}

unsafe extern "C" fn api_current_tid() -> usize {
    crate::task::current_tid()
}

unsafe extern "C" fn api_current_pid() -> usize {
    crate::task::current_pid()
}

unsafe extern "C" fn api_current_ppid() -> usize {
    crate::task::current_ppid()
}

unsafe extern "C" fn api_current_uid() -> u32 {
    crate::sec::current_uid()
}

unsafe extern "C" fn api_task_is_live_user(id: usize) -> i32 {
    i32::from(crate::task::is_live_user(id))
}

unsafe extern "C" fn api_task_has_root() -> i32 {
    i32::from(crate::task::has_ns())
}

unsafe extern "C" fn api_path_resolve(path: StrRef, mode: u32, out: *mut u8, cap: usize) -> i32 {
    let Some(path) = str_ref(path) else {
        return -1;
    };
    match mode {
        myos_abi::MYOS_PATH_VIRTUAL => {
            let mut b = [0u8; 256];
            match crate::fs::resolve_user_path_virtual(None, path, &mut b) {
                Some(n) => put_str(out, cap, &b[..n]),
                None => -1,
            }
        }
        myos_abi::MYOS_PATH_REAL => match crate::user::resolve_copied_path(path) {
            Some(p) => put_str(out, cap, p.as_bytes()),
            None => -1,
        },
        myos_abi::MYOS_PATH_REAL_NOFOLLOW => match crate::user::resolve_copied_path_nofollow(path) {
            Some(p) => put_str(out, cap, p.as_bytes()),
            None => -1,
        },
        _ => -1,
    }
}

// The path calls below act for the current process (the Linux layer's
// syscalls): the same checks as the native ones (docs/security.md).
use crate::sec::Rights;

fn may(path: &str, need: Rights) -> bool {
    crate::sec::allowed(path, need)
}

unsafe extern "C" fn api_vfs_stat(path: StrRef, out: *mut PathStat) -> i32 {
    let Some(path) = str_ref(path) else {
        return -1;
    };
    if out.is_null() || crate::sec::rights_on(path).is_empty() {
        return -1;
    }
    match crate::fs::stat(path) {
        Some(st) => {
            let is_dir = st.mode & crate::fs::S_IFMT == 0o040000;
            unsafe {
                *out = PathStat {
                    mode: (st.mode & !0o777) | crate::sec::mode_bits(path, is_dir),
                    nlink: st.nlink,
                    size: st.size as u64,
                    ino: st.ino as u64,
                    dev: st.dev as u64,
                    mtime: st.mtime,
                    atime: st.atime,
                };
            }
            0
        }
        None => -1,
    }
}

unsafe extern "C" fn api_vfs_statfs(path: StrRef, out: *mut myos_abi::VfsStatFs) -> i32 {
    let Some(path) = str_ref(path) else {
        return -1;
    };
    if out.is_null() || crate::sec::rights_on(path).is_empty() {
        return -1;
    }
    match crate::fs::vfs::statfs(path) {
        Some((st, _)) => {
            unsafe { *out = st };
            0
        }
        None => -1,
    }
}

unsafe extern "C" fn api_vfs_listdir(path: StrRef, buf: *mut u8, cap: usize) -> i32 {
    let Some(path) = str_ref(path) else {
        return -1;
    };
    if buf.is_null() {
        return -1;
    }
    if !may(path, Rights::READ) {
        return -1;
    }
    let out = unsafe { core::slice::from_raw_parts_mut(buf, cap) };
    crate::fs::listdir(path, out).min(i32::MAX as usize) as i32
}

unsafe extern "C" fn api_vfs_mkdir(path: StrRef) -> i32 {
    match str_ref(path) {
        Some(p) if may(p, Rights::CREATE) && crate::fs::mkdir(p) => 0,
        _ => -1,
    }
}

unsafe extern "C" fn api_vfs_rmdir(path: StrRef) -> i32 {
    match str_ref(path) {
        Some(p) if may(p, Rights::REMOVE) && crate::fs::rmdir(p) => 0,
        _ => -1,
    }
}

unsafe extern "C" fn api_vfs_unlink(path: StrRef) -> i32 {
    match str_ref(path) {
        Some(p) if may(p, Rights::REMOVE) && crate::fs::unlink(p) => 0,
        _ => -1,
    }
}

unsafe extern "C" fn api_vfs_rename(old: StrRef, new: StrRef) -> i32 {
    match (str_ref(old), str_ref(new)) {
        (Some(o), Some(n)) if crate::sec::may_rename(o, None, n, None, crate::fs::stat(n).is_some()) && crate::fs::rename(o, n) => 0,
        _ => -1,
    }
}

unsafe extern "C" fn api_vfs_set_times(path: StrRef, atime: u64, mtime: u64) -> i32 {
    let time = |t: u64| {
        if t == myos_abi::MYOS_TIME_OMIT { crate::fs::SetTime::Omit } else { crate::fs::SetTime::At(t) }
    };
    match str_ref(path) {
        Some(p) if may(p, Rights::SETATTR) && crate::fs::set_times(p, time(atime), time(mtime)) => 0,
        _ => -1,
    }
}

unsafe extern "C" fn api_vfs_symlink(target: StrRef, link: StrRef) -> i32 {
    match (str_ref(target), str_ref(link)) {
        (Some(t), Some(l)) if may(l, Rights::CREATE) && crate::fs::symlink(t, l) => 0,
        _ => -1,
    }
}

unsafe extern "C" fn api_vfs_readlink(path: StrRef, buf: *mut u8, cap: usize) -> i32 {
    let Some(path) = str_ref(path) else {
        return -1;
    };
    if buf.is_null() {
        return -1;
    }
    if !may(path, Rights::READ) {
        return -1;
    }
    let out = unsafe { core::slice::from_raw_parts_mut(buf, cap) };
    match crate::fs::readlink(path, out) {
        Some(n) => n as i32,
        None => -1,
    }
}

unsafe extern "C" fn api_vfs_read(path: StrRef, pos: usize, buf: *mut u8, cap: usize) -> i32 {
    let (Some(path), false) = (str_ref(path), buf.is_null()) else {
        return -1;
    };
    if !may(path, Rights::READ) {
        return -1;
    }
    let Some(node) = crate::fs::open(path, 0) else {
        return -1;
    };
    let out = unsafe { core::slice::from_raw_parts_mut(buf, cap) };
    crate::fs::read(&node, pos, out).min(i32::MAX as usize) as i32
}

unsafe extern "C" fn api_vfs_write(path: StrRef, pos: usize, buf: *const u8, len: usize) -> i32 {
    let (Some(path), false) = (str_ref(path), buf.is_null()) else {
        return -1;
    };
    if !may(path, Rights::WRITE) {
        return -1;
    }
    let Some(node) = crate::fs::open(path, 1) else {
        return -1;
    };
    let src = unsafe { core::slice::from_raw_parts(buf, len) };
    match crate::fs::write(&node, pos, src) {
        Some(n) => n.min(i32::MAX as usize) as i32,
        None => -1,
    }
}

unsafe extern "C" fn api_open_path(path: StrRef, flags: usize) -> usize {
    match str_ref(path) {
        Some(p) => crate::user::open_path(p, flags),
        None => myos_abi::MYOS_SYSERR,
    }
}

unsafe extern "C" fn api_chdir_path(path: StrRef) -> usize {
    match str_ref(path) {
        Some(p) => crate::user::chdir_path(p),
        None => myos_abi::MYOS_SYSERR,
    }
}

unsafe extern "C" fn api_fd_pread(fd: usize, pos: usize, buf: *mut u8, len: usize) -> i32 {
    let Some(node) = crate::task::fd_file_node(fd).filter(|_| crate::task::fd_readable(fd)) else {
        return -1;
    };
    if buf.is_null() {
        return -1;
    }
    let out = unsafe { core::slice::from_raw_parts_mut(buf, len) };
    crate::fs::read(&node, pos, out).min(i32::MAX as usize) as i32
}

unsafe extern "C" fn api_fd_kind(fd: usize, size: *mut usize) -> i32 {
    match crate::task::fd_kind(fd) {
        Some(crate::task::FdKind::Tty) => myos_abi::MYOS_FD_TTY,
        Some(crate::task::FdKind::Pipe) => myos_abi::MYOS_FD_PIPE,
        Some(crate::task::FdKind::File { size: n }) => {
            if !size.is_null() {
                unsafe { *size = n };
            }
            myos_abi::MYOS_FD_FILE
        }
        None => -1,
    }
}

unsafe extern "C" fn api_fd_poll_bits(fd: usize) -> i32 {
    crate::task::fd_poll_bits(fd).map_or(-1, |b| b as i32)
}

unsafe extern "C" fn api_power_register(action: u32, name: StrRef, method: unsafe extern "C" fn()) -> i32 {
    let (Some(action), Some(name)) = (crate::power::Action::from_raw(action as usize), str_ref(name)) else {
        return -1;
    };
    noted(crate::power::register(action, name, method))
}

unsafe extern "C" fn api_irq_enable(
    irq: u32,
    name: StrRef,
    handler: myos_abi::IrqHandler,
    ctx: *mut core::ffi::c_void,
) -> i32 {
    let name = str_ref(name).filter(|n| !n.is_empty()).unwrap_or("irq");
    noted(crate::irq::enable(irq, name, handler, ctx as usize))
}

unsafe extern "C" fn api_console_input() {
    crate::task::wake(crate::task::KEY_CONSOLE);
}

unsafe extern "C" fn api_fd_lockctl(fd: usize, cmd: usize, lock: *mut myos_abi::MyosLockRange) -> usize {
    match unsafe { lock.as_mut() } {
        Some(lock) => crate::user::lockctl(fd, cmd, lock),
        None => usize::MAX,
    }
}

unsafe extern "C" fn api_fd_dup_min(fd: usize, min: usize) -> i32 {
    crate::task::fd_dup_min(fd, min, false).map_or(-1, |n| n as i32)
}

unsafe extern "C" fn api_fd_dup2(old: usize, new: usize) -> i32 {
    if crate::task::fd_dup2(old, new) { 0 } else { -1 }
}

unsafe extern "C" fn api_fd_close(fd: usize) -> i32 {
    if crate::task::fd_close(fd) { 0 } else { -1 }
}

unsafe extern "C" fn api_fd_write(fd: usize, buf_user: usize, len: usize) -> usize {
    crate::task::fd_write(fd, buf_user, len, None)
}

/// Copy `text` into the module's buffer, cut at `cap`: its full length.
unsafe fn copy_text(text: &[u8], buf: *mut u8, cap: usize) -> i32 {
    let n = text.len().min(cap);
    if n != 0 && !buf.is_null() {
        unsafe { core::ptr::copy_nonoverlapping(text.as_ptr(), buf, n) };
    }
    i32::try_from(text.len()).unwrap_or(i32::MAX)
}

unsafe extern "C" fn api_tty_ctl_read(fd: usize, buf: *mut u8, cap: usize) -> i32 {
    match crate::task::fd_tty_ctl_read(fd) {
        Some(text) => unsafe { copy_text(&text, buf, cap) },
        None => -1,
    }
}

unsafe extern "C" fn api_tty_ctl_write(fd: usize, text: *const u8, len: usize) -> i32 {
    if text.is_null() {
        return -1;
    }
    let text = unsafe { core::slice::from_raw_parts(text, len) };
    if crate::task::fd_tty_ctl_write(fd, text).is_some() { 0 } else { -1 }
}

unsafe extern "C" fn api_fd_path(fd: usize, buf: *mut u8, cap: usize) -> i32 {
    match crate::task::fd_path(fd) {
        Some(path) => unsafe { copy_text(path.as_bytes(), buf, cap) },
        None => -1,
    }
}

unsafe extern "C" fn api_pipe_open(read_fd: *mut usize, write_fd: *mut usize) -> i32 {
    if read_fd.is_null() || write_fd.is_null() {
        return -1;
    }
    match crate::task::pipe_open(false) {
        Some((r, w)) => {
            unsafe {
                *read_fd = r;
                *write_fd = w;
            }
            0
        }
        None => -1,
    }
}

unsafe extern "C" fn api_mmap(addr: usize, len: usize, prot: usize, flags: usize, fd: isize, off: usize) -> usize {
    crate::user::do_mmap(addr, len, prot, flags, fd, off)
}

unsafe extern "C" fn api_signal_get_action(id: usize, sig: u32, handler: *mut usize, flags: *mut u32, mask: *mut u32) {
    let (h, f, m) = crate::task::signal_get_action(id, sig);
    unsafe {
        if !handler.is_null() {
            *handler = h;
        }
        if !flags.is_null() {
            *flags = f;
        }
        if !mask.is_null() {
            *mask = m;
        }
    }
}

unsafe extern "C" fn api_signal_set_action(id: usize, sig: u32, handler: usize, flags: u32, mask: u32, tramp: usize) -> i32 {
    if crate::task::signal_set_action(id, sig, handler, flags, mask, tramp) { 0 } else { -1 }
}

unsafe extern "C" fn api_signal_blocked(id: usize) -> u32 {
    crate::task::signal_blocked(id)
}

unsafe extern "C" fn api_signal_set_blocked(id: usize, mask: u32) {
    crate::task::signal_set_blocked_mask(id, mask)
}

unsafe extern "C" fn api_signal_pending(id: usize) -> u32 {
    crate::task::signal_pending(id)
}

unsafe extern "C" fn api_signal_take(id: usize, set: u32) -> i32 {
    crate::task::signal_take_from(id, set).map_or(-1, |s| s as i32)
}

unsafe extern "C" fn api_signal_kill(pid: isize, sig: u32) -> i32 {
    if crate::signal::kill(pid, sig) { 0 } else { -1 }
}

unsafe extern "C" fn api_signal_sigsuspend(mask: u32) -> usize {
    crate::signal::sigsuspend(mask)
}

unsafe extern "C" fn api_signal_interrupt_wait() -> i32 {
    i32::from(crate::signal::interrupt_wait())
}

unsafe extern "C" fn api_signal_terminate(sig: u32) -> ! {
    crate::signal::terminate(sig)
}

unsafe extern "C" fn api_fpu_save(buf: *mut u8) {
    if !buf.is_null() {
        unsafe { crate::task::fpu::save(buf) }
    }
}

unsafe extern "C" fn api_fpu_restore(buf: *const u8) {
    if !buf.is_null() {
        unsafe { crate::task::fpu::restore(buf) }
    }
}

unsafe extern "C" fn api_thread_pointer_get() -> u64 {
    crate::task::tp::get()
}

unsafe extern "C" fn api_thread_pointer_set(v: u64) {
    crate::task::tp::set(v)
}

unsafe extern "C" fn api_thread_spawn_from(regs: *mut u64, sp: usize, set_tls: i32, tls: u64) -> i32 {
    if regs.is_null() {
        return -1;
    }
    let regs = crate::user::SyscallRegs::from_ptr(regs);
    let mut start = crate::user::caller_regs(&regs);
    start.rsp = sp;
    let tls = (set_tls != 0).then_some(tls);
    crate::task::spawn_thread(start, tls).map_or(-1, |t| t as i32)
}

unsafe extern "C" fn api_fork_from(regs: *mut u64, sp: usize) -> i32 {
    if regs.is_null() {
        return -1;
    }
    let regs = crate::user::SyscallRegs::from_ptr(regs);
    let mut start = crate::user::caller_regs(&regs);
    start.rsp = sp;
    crate::task::fork_current(start).map_or(-1, |p| p as i32)
}

unsafe extern "C" fn api_thread_place(tid: i32) {
    if tid >= 0 {
        crate::task::place_thread(tid as usize);
    }
}

unsafe extern "C" fn api_mmap_discard(addr: usize, len: usize) -> i32 {
    if crate::user::mmap_discard(addr, len) { 0 } else { -1 }
}

unsafe extern "C" fn api_thread_exit(code: u8) -> ! {
    crate::task::thread_exit(code)
}

unsafe extern "C" fn api_wait_addr(addr: usize, expected: u32, deadline_ns: u64) -> i32 {
    use crate::task::AddrWait;
    match crate::task::wait_addr(addr, expected, deadline_ns) {
        AddrWait::Woken => myos_abi::MYOS_WAIT_WOKEN,
        AddrWait::Changed => myos_abi::MYOS_WAIT_CHANGED,
        AddrWait::TimedOut => myos_abi::MYOS_WAIT_TIMEOUT,
        AddrWait::Interrupted => myos_abi::MYOS_WAIT_INTERRUPTED,
        AddrWait::Fault => myos_abi::MYOS_WAIT_FAULT,
    }
}

unsafe extern "C" fn api_wake_addr(addr: usize, max: usize) -> usize {
    crate::task::wake_addr(addr, max)
}

unsafe extern "C" fn api_task_sleep_until(deadline_ns: u64) {
    crate::task::sleep_until(deadline_ns, false);
}

unsafe extern "C" fn api_task_yield() {
    crate::task::yield_now();
}

unsafe extern "C" fn api_wall_time_us() -> u64 {
    match crate::time::timeval() {
        Some((s, us)) if s >= 0 => s as u64 * 1_000_000 + us as u64,
        _ => 0,
    }
}

unsafe extern "C" fn api_dt_mmio_find(compatible: StrRef, index: usize, out: *mut myos_abi::MmioDevice) -> i32 {
    let Some(compat) = str_ref(compatible) else {
        return -1;
    };
    if out.is_null() {
        return -1;
    }
    let Some(dev) = crate::dt::mmio_device(compat, index) else {
        return -1;
    };
    let Some(base) = crate::arch::pci::map_mmio(dev.base, dev.size.max(1)) else {
        return -1;
    };
    let irq = dev.irq.and_then(|s| crate::arch::irq_from_dt(s.cells())).unwrap_or(0);
    unsafe {
        *out = myos_abi::MmioDevice {
            base,
            size: dev.size as usize,
            irq,
        };
    }
    0
}

unsafe extern "C" fn api_rng_fill(buf: *mut u8, len: usize) {
    if buf.is_null() || len == 0 {
        return;
    }
    let out = unsafe { core::slice::from_raw_parts_mut(buf, len) };
    crate::rng::fill(out);
}

// --- ABI 21: hot-pluggable buses (docs/usb.md) ---

unsafe extern "C" fn api_blk_unregister(dev: u32) -> i32 {
    match crate::blk::unregister(dev) {
        Ok(()) => 0,
        Err(()) => myos_abi::MYOS_EBUSY,
    }
}

unsafe extern "C" fn api_service_register(name: StrRef, table: *const core::ffi::c_void) -> i32 {
    let Some(name) = str_ref(name) else {
        return -1;
    };
    noted(services::register(name, table as usize))
}

unsafe extern "C" fn api_service_lookup(name: StrRef) -> *const core::ffi::c_void {
    let Some(name) = str_ref(name) else {
        return core::ptr::null();
    };
    match services::lookup(name) {
        Some(table) => {
            // The table's owner may call back into the module from now on:
            // it stays loaded (a registration pins it).
            registry::note_registration();
            table as *const core::ffi::c_void
        }
        None => core::ptr::null(),
    }
}

unsafe extern "C" fn api_thread_spawn(
    name: StrRef,
    entry: unsafe extern "C" fn(*mut core::ffi::c_void),
    ctx: *mut core::ffi::c_void,
) -> i32 {
    noted(threads::spawn(str_ref(name).unwrap_or("module"), entry, ctx))
}

unsafe extern "C" fn api_wake(key: usize) {
    crate::task::wake(key);
}
