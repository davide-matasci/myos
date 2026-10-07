//! Function-pointer table the kernel hands to a loadable module.
//!
//! This is the modular ABI. Modules do not link against kernel `.dynsym`;
//! they receive a [`KernelApi`] from `module_init` and call through it.
//! The helpers a module keeps its own state in are here too: the table in
//! an [`ApiCell`], the rest behind a [`Lock`].

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

use core::sync::atomic::{AtomicPtr, Ordering};

mod lock;

pub use lock::{Lock, LockGuard};

/// Bump this when [`KernelApi`] layout or meaning changes. 19 took `fd_ioctl`
/// out of the table and the ioctl hook out of [`ModuleChrOps`]: there is no
/// ioctl, a device's state is its `ctl` file and `poll` says when it is ready.
/// 20 added [`ModuleVfsOps::open`] (a file one program holds at a time).
/// 21 added what hot-pluggable buses need (`docs/usb.md`): `blk_unregister`,
/// the service registry modules reach each other through
/// (`service_register` / `service_lookup`), module threads (`thread_spawn`)
/// and targeted wakes (`wake`), plus the USB bus types below.
/// 22 added [`KernelApi::fork_from`] (posix_spawn's child on its own stack).
/// 23 added [`KernelApi::mmap_discard`] (`madvise(MADV_DONTNEED)`).
/// 24 added `mtime` to [`VfsStatInfo`] and [`PathStat`].
/// 25 added `atime` to both, [`ModuleVfsOps::set_times`] and
/// [`KernelApi::vfs_set_times`] (`utimensat`).
/// 26 added [`ModuleVfsOps::unmount`] (`umount(2)`).
/// 27 added [`KernelApi::thread_place`] (a new thread's own CPU).
/// 28 added the hooks that keep a file unlinked while it is held
/// ([`ModuleVfsOps::unlink_keep`] and the `*_ino` ones).
/// 29 added [`ModuleVfsOps::set_size`] and `set_size_ino` (`ftruncate`).
/// 30 added [`ModuleVfsOps::file_id`] and `set_times_ino` (a filesystem
/// with file ids has its open files used by them, not by their paths) and
/// [`KernelApi::fd_lockctl`] (record locks).
/// 31 added [`KernelApi::power_register`] (power-off and reboot methods).
pub const ABI_VERSION: u32 = 31;

/// A time argument of [`ModuleVfsOps::set_times`] / [`KernelApi::vfs_set_times`]
/// that keeps the current value.
pub const MYOS_TIME_OMIT: u64 = u64::MAX;

/// `KernelApi::block_until` key woken by every `wake`, including `wake_any`.
pub const MYOS_WAIT_ANY: usize = usize::MAX;

/// Device interrupt handler (`KernelApi::pci_irq_enable`): runs in interrupt
/// context on the BSP with interrupts masked; must not block or allocate.
pub type IrqHandler = unsafe extern "C" fn(ctx: *mut core::ffi::c_void);
/// `pci_irq_enable` out value: the device is on legacy INTx (read its ISR
/// register in the handler to deassert the line).
pub const MYOS_IRQ_INTX: u16 = 0xFFFF;

/// Stat blob exchanged with module VFS hooks (matches kernel layout).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VfsStatInfo {
    pub mode: u32,
    pub size: u32,
    pub ino: u32,
    pub nlink: u32,
    /// Last modification, in seconds since the epoch (0: not kept).
    pub mtime: u64,
    /// Last access as set by `set_times` (reads need not change it), in
    /// seconds since the epoch (0: not kept).
    pub atime: u64,
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
    pub stat: unsafe extern "C" fn(path: *const u8, path_len: usize, out: *mut VfsStatInfo) -> i32,
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
    /// Optional read at `pos`. Bytes read (>=0) or negative error.
    /// If None, the kernel copies from `lookup` (FAT).
    pub read: Option<
        unsafe extern "C" fn(
            path: *const u8,
            path_len: usize,
            pos: usize,
            buf: *mut u8,
            buf_len: usize,
        ) -> i32,
    >,
    /// Optional write at `pos`. Bytes written (>=0) or negative error.
    pub write: Option<
        unsafe extern "C" fn(
            path: *const u8,
            path_len: usize,
            pos: usize,
            buf: *const u8,
            buf_len: usize,
        ) -> i32,
    >,
    /// Optional create/truncate/mkdir/rmdir/unlink. 0 ok, negative fail.
    pub create: Option<unsafe extern "C" fn(path: *const u8, path_len: usize) -> i32>,
    pub truncate: Option<unsafe extern "C" fn(path: *const u8, path_len: usize) -> i32>,
    pub mkdir: Option<unsafe extern "C" fn(path: *const u8, path_len: usize) -> i32>,
    pub rmdir: Option<unsafe extern "C" fn(path: *const u8, path_len: usize) -> i32>,
    pub unlink: Option<unsafe extern "C" fn(path: *const u8, path_len: usize) -> i32>,
    pub rename: Option<
        unsafe extern "C" fn(old: *const u8, old_len: usize, new: *const u8, new_len: usize) -> i32,
    >,
    pub symlink: Option<
        unsafe extern "C" fn(
            target: *const u8,
            target_len: usize,
            linkpath: *const u8,
            linkpath_len: usize,
        ) -> i32,
    >,
    /// Optional readlink. Bytes written (>=0) or negative error.
    pub readlink: Option<
        unsafe extern "C" fn(path: *const u8, path_len: usize, buf: *mut u8, buf_len: usize) -> i32,
    >,
    /// Optional: last open fd on this path was closed (fork-safe close).
    /// Socket-like backends must tear their conv down here, not on any
    /// mid-life close: after fork() several fds share one conv, and the
    /// parent's close must not kill the child's connection.
    pub release: Option<unsafe extern "C" fn(path: *const u8, path_len: usize) -> i32>,
    // --- ABI 16: device memory ---
    /// Optional: the physical address of the page at byte `offset` (page
    /// aligned) of `path`, for `mmap` of device memory (`/dev/fb/data`), or 0
    /// when that page cannot be mapped. The page stays the module's: the
    /// kernel maps it shared into every process that asks, never copies it
    /// on fork and never frees it.
    pub mmap: Option<unsafe extern "C" fn(path: *const u8, path_len: usize, offset: usize) -> u64>,
    // --- ABI 17: readiness ---
    /// Optional: the `poll(2)` bits ([`MYOS_POLLIN`], [`MYOS_POLLOUT`],
    /// [`MYOS_POLLERR`], [`MYOS_POLLHUP`]) that hold now for `path`, whatever
    /// the caller asked for; without it a file is always readable and
    /// writable. Must not block. When readiness changes other than through a
    /// write or the last close of one of its files (both wake pollers), the
    /// backend calls `KernelApi::wake_any`.
    pub poll: Option<unsafe extern "C" fn(path: *const u8, path_len: usize) -> u32>,
    // --- ABI 20: exclusive files ---
    /// Optional: an `open(2)` of `path` (not a `dup` or a `fork`): 0 lets it
    /// through, negative refuses it (a file one program holds at a time).
    /// `release` follows when the last fd of the file closes.
    pub open: Option<unsafe extern "C" fn(path: *const u8, path_len: usize) -> i32>,
    // --- ABI 25: file times ---
    /// Optional: set `path`'s access and modification times, in seconds
    /// since the epoch ([`MYOS_TIME_OMIT`] keeps one): 0, or negative.
    /// Without it the mount's times cannot be set.
    pub set_times: Option<unsafe extern "C" fn(path: *const u8, path_len: usize, atime: u64, mtime: u64) -> i32>,
    // --- ABI 26: unmount ---
    /// Optional: `umount(2)` detached this mount (no file on it is open):
    /// write back what is cached and forget the filesystem. The hooks are
    /// not called for it again.
    pub unmount: Option<unsafe extern "C" fn()>,
    // --- ABI 28: files held after an unlink ---
    /// Optional: remove the name `path` (not a directory) but keep its
    /// file, which something still holds (an open fd, a mapping): its
    /// inode number (> 0), or negative. The kernel calls `forget_ino` when
    /// the last holder lets go. Without it, a held file a name no longer
    /// leads to is gone for its holders.
    pub unlink_keep: Option<unsafe extern "C" fn(path: *const u8, path_len: usize) -> i64>,
    /// [`ModuleVfsOps::read`] of the file with inode `ino` (see `file_id`).
    pub read_ino: Option<unsafe extern "C" fn(ino: u64, pos: usize, buf: *mut u8, buf_len: usize) -> i32>,
    /// [`ModuleVfsOps::write`] of the file with inode `ino`.
    pub write_ino: Option<unsafe extern "C" fn(ino: u64, pos: usize, buf: *const u8, buf_len: usize) -> i32>,
    /// [`ModuleVfsOps::stat`] of the file with inode `ino`.
    pub stat_ino: Option<unsafe extern "C" fn(ino: u64, out: *mut VfsStatInfo) -> i32>,
    /// Nothing holds the file `unlink_keep` kept any more: free it.
    pub forget_ino: Option<unsafe extern "C" fn(ino: u64) -> i32>,
    // --- ABI 29: file sizes ---
    /// Optional: make the file `path` `size` bytes long, cut or grown with
    /// zeros (`ftruncate`): 0, or negative. Without it the mount's files
    /// cannot be resized (only emptied, by `truncate`).
    pub set_size: Option<unsafe extern "C" fn(path: *const u8, path_len: usize, size: u64) -> i32>,
    /// [`ModuleVfsOps::set_size`] of the file with inode `ino`.
    pub set_size_ino: Option<unsafe extern "C" fn(ino: u64, size: u64) -> i32>,
    // --- ABI 30: files by inode ---
    /// Optional: the inode number (> 0) of the file or directory at `path`,
    /// or negative. With it, an open file is used by its inode from then on
    /// (`read_ino`, `write_ino`, `stat_ino`, `set_size_ino`,
    /// `set_times_ino`), whatever is renamed meanwhile: a module that
    /// gives inodes has all of them, and `unlink_keep` and `forget_ino`.
    pub file_id: Option<unsafe extern "C" fn(path: *const u8, path_len: usize) -> i64>,
    /// [`ModuleVfsOps::set_times`] of the file with inode `ino`.
    pub set_times_ino: Option<unsafe extern "C" fn(ino: u64, atime: u64, mtime: u64) -> i32>,
}

/// [`ModuleVfsOps::read`]: nothing to read yet. A read through an fd waits
/// until `poll` reports [`MYOS_POLLIN`] (or a signal), then reads again.
pub const MYOS_READ_WAIT: i32 = -11;

/// `poll(2)` bits (Linux values), for [`ModuleVfsOps::poll`].
pub const MYOS_POLLIN: u32 = 0x1;
pub const MYOS_POLLOUT: u32 = 0x4;
pub const MYOS_POLLERR: u32 = 0x8;
pub const MYOS_POLLHUP: u32 = 0x10;
/// [`ModuleVfsOps::poll`]: readiness changes without a wake (a device the
/// kernel polls, like the keyboard): pollers re-check the file every 10 ms.
pub const MYOS_POLL_RECHECK: u32 = 0x8000_0000;

/// Module-provided block device (`KernelApi::blk_register`). Sector size is
/// 512 bytes; `buf` lengths are whole sectors. `ctx` is the value given at
/// registration (the driver's device index). 0 ok, negative on error.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ModuleBlkOps {
    pub read: unsafe extern "C" fn(ctx: usize, lba: u64, buf: *mut u8, len: usize) -> i32,
    pub write: unsafe extern "C" fn(ctx: usize, lba: u64, buf: *const u8, len: usize) -> i32,
    /// Size in 512-byte sectors (0 = unknown).
    pub capacity_sectors: unsafe extern "C" fn(ctx: usize) -> u64,
}

/// The boot framebuffer as the bootloader left it (`KernelApi::framebuffer_info`):
/// `addr` is its mapped virtual address, `pitch` the bytes per row.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FramebufferInfo {
    pub addr: u64,
    pub width: u64,
    pub height: u64,
    pub pitch: u64,
    pub bpp: u16,
    pub r_shift: u8,
    pub g_shift: u8,
    pub b_shift: u8,
    pub r_size: u8,
    pub g_size: u8,
    pub b_size: u8,
}

/// Text kinds for `ModuleConsoleOps::write_kind`: plain mirrored output, the
/// boot banner (accent colour) and dim informational text.
pub const CONSOLE_TEXT: u32 = 0;
pub const CONSOLE_BANNER: u32 = 1;
pub const CONSOLE_INFO: u32 = 2;
/// Status-line kinds for `ModuleConsoleOps::status_line` (`[ TAG ] label`):
/// ok, fail, info / progress, warn.
pub const CONSOLE_STATUS_OK: u32 = 0;
pub const CONSOLE_STATUS_FAIL: u32 = 1;
pub const CONSOLE_STATUS_INFO: u32 = 2;
pub const CONSOLE_STATUS_WARN: u32 = 3;

/// The module-provided console (`KernelApi::console_register`): the
/// framebuffer text screen and the local keyboards. Serial stays in the
/// kernel, which calls these after writing a byte there. `blink` runs from
/// the timer interrupt and must never spin on a lock.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ModuleConsoleOps {
    /// Paint `len` bytes of text (ANSI escapes understood) of `kind`
    /// ([`CONSOLE_TEXT`], [`CONSOLE_BANNER`], [`CONSOLE_INFO`]).
    pub write_kind: unsafe extern "C" fn(buf: *const u8, len: usize, kind: u32),
    /// Paint one `[ TAG ] label` line with the tag coloured by `kind`.
    pub status_line: unsafe extern "C" fn(
        tag: *const u8,
        tag_len: usize,
        kind: u32,
        label: *const u8,
        label_len: usize,
    ),
    /// Character-cell size (rows, cols); negative when there is no screen.
    pub winsize: unsafe extern "C" fn(rows: *mut u16, cols: *mut u16) -> i32,
    /// Toggle the block cursor (timer tick). Must not block.
    pub blink: unsafe extern "C" fn(),
    /// A local keyboard was found.
    pub keyboard_present: unsafe extern "C" fn() -> i32,
    /// Next keyboard byte (keymap-translated), or negative when none is pending.
    pub keyboard_poll: unsafe extern "C" fn() -> i32,
    /// Install a keymap from its text form (the console ctl's `keymap PATH`,
    /// `docs/keymap.md`). 0 ok, negative on error.
    pub keymap_load: unsafe extern "C" fn(text: *const u8, len: usize) -> i32,
    /// 1 when a keymap is loaded.
    pub keymap_loaded: unsafe extern "C" fn() -> i32,
}

// ---- ABI 13: personalities (the Linux layer is a module) --------------------

/// Task slots (pids / tids are slot indexes), the user page size and the
/// native exec limits, for modules that keep per-task state or build execs.
pub const MYOS_MAX_TASKS: usize = 64;
pub const MYOS_PAGE: usize = 4096;
pub const MYOS_MAX_ARGC: usize = 1024;
pub const MYOS_MAX_ENVC: usize = 1024;
pub const MYOS_MAX_EXEC_STRINGS: usize = 128 * 1024;

/// Native syscall results: anything from [`MYOS_SYSERR_LOWEST`] up is a
/// failure sentinel (`usize::MAX` the generic one).
pub const MYOS_SYSERR: usize = usize::MAX;
pub const MYOS_SYSERR_EIO: usize = usize::MAX - 1;
pub const MYOS_SYSERR_ENXIO: usize = usize::MAX - 2;
pub const MYOS_SYSERR_EINTR: usize = usize::MAX - 3;
pub const MYOS_SYSERR_EEXIST: usize = usize::MAX - 4;
pub const MYOS_SYSERR_ESPIPE: usize = usize::MAX - 5;
pub const MYOS_SYSERR_ELOOP: usize = usize::MAX - 6;
pub const MYOS_SYSERR_ENOTDIR: usize = usize::MAX - 7;
pub const MYOS_SYSERR_EAGAIN: usize = usize::MAX - 8;
pub const MYOS_SYSERR_LOWEST: usize = MYOS_SYSERR_EAGAIN;

/// Native signal dispositions (`signal_get_action`): default, ignore; any
/// other value is a caught handler's address. Native signal numbers are
/// newlib / BSD (`SIGKILL` 9, `SIGSEGV` 11).
pub const MYOS_HANDLER_DFL: usize = 0;
pub const MYOS_HANDLER_IGN: usize = 1;
pub const MYOS_SIGSEGV: u32 = 11;
/// `sa_flags` bit: on `SIGCHLD`, children leave no zombie (kernel `signal::SA_NOCLDWAIT`).
pub const MYOS_SA_NOCLDWAIT: u32 = 0x20;

/// Size of the FP/SIMD register image `fpu_save` writes (x86_64 FXSAVE,
/// aarch64 `fpsimd_context` head + v0-v31, riscv64 f0-f31 + fcsr). The
/// buffer must be 16-byte aligned.
#[cfg(target_arch = "x86_64")]
pub const MYOS_FP_BYTES: usize = 512;
#[cfg(target_arch = "aarch64")]
pub const MYOS_FP_BYTES: usize = 528;
#[cfg(target_arch = "riscv64")]
pub const MYOS_FP_BYTES: usize = 264;

/// A kernel-memory string (not NUL-terminated).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StrRef {
    pub ptr: *const u8,
    pub len: usize,
}

/// `vfs_stat` result.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PathStat {
    pub mode: u32,
    pub nlink: u32,
    pub size: u64,
    pub ino: u64,
    pub dev: u64,
    /// Last modification, in seconds since the epoch (0: not kept).
    pub mtime: u64,
    /// Last access, in seconds since the epoch (0: not kept).
    pub atime: u64,
}

/// `path_resolve` modes: the task's own view (cwd applied, chroot-relative),
/// the real VFS path (cwd, chroot and symlinks applied), the real path with a
/// symlink in the last component not followed.
pub const MYOS_PATH_VIRTUAL: u32 = 0;
pub const MYOS_PATH_REAL: u32 = 1;
pub const MYOS_PATH_REAL_NOFOLLOW: u32 = 2;

/// `fd_kind` results.
pub const MYOS_FD_TTY: i32 = 0;
pub const MYOS_FD_PIPE: i32 = 1;
pub const MYOS_FD_FILE: i32 = 2;

/// `wait_addr` results.
pub const MYOS_WAIT_WOKEN: i32 = 0;
pub const MYOS_WAIT_CHANGED: i32 = 1;
pub const MYOS_WAIT_TIMEOUT: i32 = 2;
pub const MYOS_WAIT_INTERRUPTED: i32 = 3;
pub const MYOS_WAIT_FAULT: i32 = 4;

/// A caught signal about to run its handler in a task of a foreign
/// personality (`PersonalityOps::deliver`): native signal number, the
/// handler and its `sigaction` flags, the sigreturn trampoline (the
/// registered `sa_restorer`, or the kernel's), the native blocked mask the
/// handler's return restores, and the interrupted PC and syscall result.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SignalDelivery {
    pub sig: u32,
    pub flags: u32,
    pub handler: usize,
    pub tramp: usize,
    pub restore_mask: u32,
    pub pc: usize,
    pub ret: usize,
    /// Arch word for the frame: x86_64 the user code segment selector (CS
    /// at handler entry); 0 elsewhere.
    pub arch: u64,
}

/// `PersonalityOps::flags`: the personality's tasks run with the FPU on
/// (riscv64 `sstatus.FS`; native programs are soft-float there).
pub const PERSONALITY_FPU_ON: u32 = 1;

/// A foreign syscall personality (`KernelApi::personality_register`; one per
/// boot). A task acquires it through `SYS_LINUX_NEXT_EXEC` + exec or
/// [`KernelApi::personality_exec`]; the kernel tracks which tasks have it and
/// calls these hooks for them. `on_fork` / `on_thread` run under the task
/// lock with interrupts off.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PersonalityOps {
    pub flags: u32,
    /// A syscall from a task with the personality: `regs` is the saved
    /// user-register block (layout per arch, see `user::SyscallRegs`).
    pub syscall: unsafe extern "C" fn(nr: usize, a0: usize, a1: usize, a2: usize, regs: *mut u64) -> usize,
    /// A successful exec replaced `slot`'s image (with or without the personality).
    pub on_exec: unsafe extern "C" fn(slot: usize),
    pub on_fork: unsafe extern "C" fn(parent: usize, child: usize),
    pub on_thread: unsafe extern "C" fn(creator: usize, slot: usize),
    pub on_spawn: unsafe extern "C" fn(slot: usize),
    /// Build the signal frame and redirect `regs` to the handler; writes the
    /// result-register value to `out`. Negative if the frame does not fit
    /// (the kernel then kills the task with SIGSEGV).
    pub deliver: unsafe extern "C" fn(regs: *mut u64, d: *const SignalDelivery, out: *mut usize) -> i32,
}

/// Bind `dev_id` to a filesystem and fill `ops`. Return 0 on success.
pub type FsBind = unsafe extern "C" fn(dev_id: u32, ops: *mut ModuleVfsOps) -> i32;

/// Module-provided character device (`KernelApi::dev_register`): the
/// directory `/dev/<name>/` with `data` (`S_IFCHR | 0666`) and, when the
/// module gives it one, the control file `ctl` (text, like a terminal's,
/// `docs/tty.md`). `read`/`write` are `data`: bytes (>=0) or a negative
/// error; `read` may return 0 when nothing is pending, `poll` tells.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ModuleChrOps {
    pub read: unsafe extern "C" fn(buf: *mut u8, buf_len: usize) -> i32,
    pub write: unsafe extern "C" fn(buf: *const u8, buf_len: usize) -> i32,
    /// Optional: the `poll(2)` bits ([`MYOS_POLLIN`], [`MYOS_POLLOUT`], ...)
    /// of `data` now. `None`: always readable and writable. The module wakes
    /// the pollers itself (`KernelApi::wake_any`) when readiness changes
    /// other than through its own `read`/`write`: an interrupt, a request
    /// queued from another module.
    pub poll: Option<unsafe extern "C" fn() -> u32>,
    /// Optional: the text of `ctl` into `buf` (cut at `cap`): its full
    /// length, or negative. `None` (with `ctl_write`): no `ctl` file.
    pub ctl_read: Option<unsafe extern "C" fn(buf: *mut u8, cap: usize) -> i32>,
    /// Optional: a write of `len` bytes of text to `ctl`: bytes taken, or
    /// negative when refused.
    pub ctl_write: Option<unsafe extern "C" fn(text: *const u8, len: usize) -> i32>,
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
    pub blk_read: unsafe extern "C" fn(dev: u32, lba: u64, buf: *mut u8, len: usize) -> i32,
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
    pub blk_write: unsafe extern "C" fn(dev: u32, lba: u64, buf: *const u8, len: usize) -> i32,
    pub blk_count: unsafe extern "C" fn() -> u32,
    /// Register a filesystem type. `bind` is called from `mount(2)`.
    pub fs_register: unsafe extern "C" fn(name: *const u8, name_len: usize, bind: FsBind) -> i32,
    /// Byte-granular read at `offset`. Returns bytes copied (>=0) or negative on error.
    pub blk_read_at: unsafe extern "C" fn(dev: u32, offset: u64, buf: *mut u8, len: usize) -> i32,
    /// Byte-granular write at `offset`. Returns bytes written (>=0) or negative on error.
    pub blk_write_at:
        unsafe extern "C" fn(dev: u32, offset: u64, buf: *const u8, len: usize) -> i32,
    pub pci_cfg_read32: unsafe extern "C" fn(bus: u8, slot: u8, func: u8, off: u8) -> u32,
    pub pci_cfg_write32: unsafe extern "C" fn(bus: u8, slot: u8, func: u8, off: u8, val: u32),
    pub pci_enable: unsafe extern "C" fn(bus: u8, slot: u8, func: u8),
    pub pci_find: unsafe extern "C" fn(
        vendor: u16,
        device: u16,
        index: u32,
        bus: *mut u8,
        slot: *mut u8,
        func: *mut u8,
    ) -> i32,
    pub pci_bar_map: unsafe extern "C" fn(
        bus: u8,
        slot: u8,
        func: u8,
        bar: u8,
        va: *mut usize,
        size: *mut u64,
    ) -> i32,
    pub dma_alloc: unsafe extern "C" fn(n_pages: usize, phys: *mut u64) -> *mut u8,
    pub dev_register: unsafe extern "C" fn(
        name: *const u8,
        name_len: usize,
        ops: *const ModuleChrOps,
    ) -> i32,
    /// Copy `len` bytes from kernel `src` to userspace address `dst_user`.
    /// Valid only during a syscall on the current task. 0 ok, negative on fault.
    pub copy_to_user: unsafe extern "C" fn(dst_user: usize, src: *const u8, len: usize) -> i32,
    /// Register or replace a generated `/proc/<name>` text node.
    /// `name` may contain a single `/` for a subdirectory (e.g. `acpi/tables`).
    /// Data is copied into a leaked kernel buffer. 0 ok, negative on error.
    pub proc_register: unsafe extern "C" fn(
        name: *const u8,
        name_len: usize,
        data: *const u8,
        data_len: usize,
    ) -> i32,
    /// Limine RSDP virtual address, or 0 if unavailable (non-ACPI firmware).
    pub acpi_rsdp: unsafe extern "C" fn() -> usize,
    /// HHDM offset for phys→virt of ACPI tables when needed. 0 on arches
    /// that identity-map low memory already.
    pub hhdm_offset: unsafe extern "C" fn() -> u64,
    /// Attach or clear a write handler for an existing `/proc/<name>` node.
    /// Invoked from write(2). Return bytes consumed (>=0) or negative on error.
    /// Passing `None` clears the writer. 0 ok, negative on error.
    pub proc_set_writer: unsafe extern "C" fn(
        name: *const u8,
        name_len: usize,
        writer: Option<unsafe extern "C" fn(*const u8, usize) -> i32>,
    ) -> i32,
    // --- ABI 11: device interrupts and blocking waits ---
    /// Route the PCI function's interrupt to `handler(ctx)`. The kernel picks
    /// the mechanism: `*msix_entry` is the MSI-X table entry the device must
    /// use (virtio: `queue_msix_vector`), or [`MYOS_IRQ_INTX`] for a legacy
    /// INTx line. `name` labels `/proc/interrupts`. 0 ok, negative on error.
    pub pci_irq_enable: unsafe extern "C" fn(
        bus: u8,
        slot: u8,
        func: u8,
        name: *const u8,
        name_len: usize,
        handler: IrqHandler,
        ctx: *mut core::ffi::c_void,
        msix_entry: *mut u16,
    ) -> i32,
    /// Wake every task sleeping on "any event": `poll(2)`, after a device's
    /// readiness changed (its interrupt, a queued request). Safe from
    /// interrupt context.
    pub wake_any: unsafe extern "C" fn(),
    /// Blocking-wait protocol (see `kernel/src/task/sched.rs`): read the
    /// sequence, check the condition, then `block_until(key, seq, deadline)`;
    /// a wake in between makes `block_until` return at once.
    pub wait_seq: unsafe extern "C" fn() -> u64,
    /// Block the calling task until `wake(key)` (or any wake for
    /// [`MYOS_WAIT_ANY`]), a signal, or the monotonic deadline (0 = none).
    /// Task context only.
    pub block_until: unsafe extern "C" fn(key: usize, seq: u64, deadline_ns: u64),
    /// Monotonic nanoseconds (`kernel/src/time.rs`).
    pub monotonic_ns: unsafe extern "C" fn() -> u64,
    // --- ABI 12: block devices are modules ---
    /// Register a block device as `/dev/<name>` (`vda`, `nvme0n1`, …) and a
    /// `blk_*` device id. Returns the id (>= 0) or negative on error (table
    /// full, duplicate name). The kernel's `blk_read` / `blk_write` and the
    /// filesystem modules then reach it through `ops`.
    pub blk_register: unsafe extern "C" fn(
        name: *const u8,
        name_len: usize,
        ops: *const ModuleBlkOps,
        ctx: usize,
    ) -> i32,
    /// Nth PCI function (0-based) of `class` / `subclass` (e.g. NVMe is
    /// 0x01 / 0x08). 0 ok (BDF filled), negative when there is none.
    pub pci_find_class: unsafe extern "C" fn(
        class: u8,
        subclass: u8,
        index: u32,
        bus: *mut u8,
        slot: *mut u8,
        func: *mut u8,
    ) -> i32,
    // --- ABI 12: the console is a module ---
    /// The boot framebuffer, if the bootloader set one up. Negative when none.
    pub framebuffer_info: unsafe extern "C" fn(out: *mut FramebufferInfo) -> i32,
    /// Install the console (one per boot): the kernel replays the boot output
    /// it buffered so far, then mirrors every later line. 0 ok.
    pub console_register: unsafe extern "C" fn(ops: *const ModuleConsoleOps) -> i32,
    // --- ABI 13: personalities and the kernel services a syscall layer needs ---
    /// Install the foreign personality (one per boot). 0 ok.
    pub personality_register: unsafe extern "C" fn(ops: *const PersonalityOps) -> i32,
    /// Exec `path` with `argv` / `envp` (kernel strings) so the new image
    /// starts with the personality. Native result (does not return on success).
    pub personality_exec: unsafe extern "C" fn(
        path: StrRef,
        argv: *const StrRef,
        argc: usize,
        envp: *const StrRef,
        envc: usize,
    ) -> usize,
    /// Run native syscall `nr` for the current task (no personality
    /// dispatch, no signal handling): the native result.
    pub native_syscall: unsafe extern "C" fn(nr: usize, a0: usize, a1: usize, a2: usize, regs: *mut u64) -> usize,
    /// Copy `len` bytes from user address `src_user` into kernel `dst`. 0 ok,
    /// negative on fault.
    pub copy_from_user: unsafe extern "C" fn(src_user: usize, dst: *mut u8, len: usize) -> i32,
    /// `[ptr, ptr+len)` lies in the current task's user mappings.
    pub user_buffer_ok: unsafe extern "C" fn(ptr: usize, len: usize) -> i32,
    pub current_tid: unsafe extern "C" fn() -> usize,
    pub current_pid: unsafe extern "C" fn() -> usize,
    pub current_ppid: unsafe extern "C" fn() -> usize,
    /// `id` is a live user task.
    pub task_is_live_user: unsafe extern "C" fn(id: usize) -> i32,
    /// The current process is chrooted.
    pub task_has_root: unsafe extern "C" fn() -> i32,
    /// Resolve `path` for the current task (`MYOS_PATH_*`) into `out`: the
    /// length, or negative.
    pub path_resolve: unsafe extern "C" fn(path: StrRef, mode: u32, out: *mut u8, cap: usize) -> i32,
    pub vfs_stat: unsafe extern "C" fn(path: StrRef, out: *mut PathStat) -> i32,
    /// Directory listing (newline-separated names) into `buf`: the length.
    pub vfs_listdir: unsafe extern "C" fn(path: StrRef, buf: *mut u8, cap: usize) -> i32,
    pub vfs_mkdir: unsafe extern "C" fn(path: StrRef) -> i32,
    pub vfs_rmdir: unsafe extern "C" fn(path: StrRef) -> i32,
    pub vfs_unlink: unsafe extern "C" fn(path: StrRef) -> i32,
    pub vfs_rename: unsafe extern "C" fn(old: StrRef, new: StrRef) -> i32,
    pub vfs_symlink: unsafe extern "C" fn(target: StrRef, link: StrRef) -> i32,
    /// The link target into `buf`: the length, or negative.
    pub vfs_readlink: unsafe extern "C" fn(path: StrRef, buf: *mut u8, cap: usize) -> i32,
    /// open(2) of a kernel-string path (cwd-relative or absolute) with
    /// native flags: the fd, or a native failure sentinel.
    pub open_path: unsafe extern "C" fn(path: StrRef, flags: usize) -> usize,
    pub chdir_path: unsafe extern "C" fn(path: StrRef) -> usize,
    /// Read `len` bytes at `pos` of the file behind `fd` into kernel `buf`
    /// (the file position is left alone): bytes read, or negative if not a file.
    pub fd_pread: unsafe extern "C" fn(fd: usize, pos: usize, buf: *mut u8, len: usize) -> i32,
    /// What `fd` refers to (`MYOS_FD_*`; a file's size in `*size`), or negative.
    pub fd_kind: unsafe extern "C" fn(fd: usize, size: *mut usize) -> i32,
    /// Pipe readiness bits (1 readable, 2 writable, 4 hung up), or negative
    /// when the fd is not a pipe.
    pub fd_poll_bits: unsafe extern "C" fn(fd: usize) -> i32,
    pub fd_dup_min: unsafe extern "C" fn(fd: usize, min: usize) -> i32,
    pub fd_dup2: unsafe extern "C" fn(old: usize, new: usize) -> i32,
    pub fd_close: unsafe extern "C" fn(fd: usize) -> i32,
    /// write(2) from user memory: native result.
    pub fd_write: unsafe extern "C" fn(fd: usize, buf_user: usize, len: usize) -> usize,
    pub pipe_open: unsafe extern "C" fn(read_fd: *mut usize, write_fd: *mut usize) -> i32,
    /// The native mmap (user addresses; `fd` -1 for anonymous): native result.
    pub mmap: unsafe extern "C" fn(addr: usize, len: usize, prot: usize, flags: usize, fd: isize, off: usize) -> usize,
    pub signal_get_action: unsafe extern "C" fn(id: usize, sig: u32, handler: *mut usize, flags: *mut u32, mask: *mut u32),
    pub signal_set_action: unsafe extern "C" fn(id: usize, sig: u32, handler: usize, flags: u32, mask: u32, tramp: usize) -> i32,
    pub signal_blocked: unsafe extern "C" fn(id: usize) -> u32,
    pub signal_set_blocked: unsafe extern "C" fn(id: usize, mask: u32),
    pub signal_pending: unsafe extern "C" fn(id: usize) -> u32,
    /// Take one pending signal of `set` (native numbering), or negative.
    pub signal_take: unsafe extern "C" fn(id: usize, set: u32) -> i32,
    /// kill(2) with a native signal number (`pid` <= 0 as POSIX). 0 ok.
    pub signal_kill: unsafe extern "C" fn(pid: isize, sig: u32) -> i32,
    pub signal_sigsuspend: unsafe extern "C" fn(mask: u32) -> usize,
    /// A signal that terminates or is caught is pending (a wait should end).
    pub signal_interrupt_wait: unsafe extern "C" fn() -> i32,
    /// End the current process as if killed by native signal `sig`.
    pub signal_terminate: unsafe extern "C" fn(sig: u32) -> !,
    /// Save / load the user FP/SIMD registers (`MYOS_FP_BYTES`, 16-aligned).
    pub fpu_save: unsafe extern "C" fn(buf: *mut u8),
    pub fpu_restore: unsafe extern "C" fn(buf: *const u8),
    pub thread_pointer_get: unsafe extern "C" fn() -> u64,
    pub thread_pointer_set: unsafe extern "C" fn(v: u64),
    /// Start a thread resuming like the caller of the syscall in `regs`
    /// (result 0) on stack `sp`, with thread pointer `tls` when `set_tls`:
    /// its tid, or negative.
    pub thread_spawn_from: unsafe extern "C" fn(regs: *mut u64, sp: usize, set_tls: i32, tls: u64) -> i32,
    pub thread_exit: unsafe extern "C" fn(code: u8) -> !,
    /// Block while the user word at `addr` holds `expected` (`MYOS_WAIT_*`).
    pub wait_addr: unsafe extern "C" fn(addr: usize, expected: u32, deadline_ns: u64) -> i32,
    pub wake_addr: unsafe extern "C" fn(addr: usize, max: usize) -> usize,
    /// Sleep until the monotonic deadline (a signal may end it early).
    pub task_sleep_until: unsafe extern "C" fn(deadline_ns: u64),
    pub task_yield: unsafe extern "C" fn(),
    /// Wall-clock microseconds since the epoch (0 if no RTC).
    pub wall_time_us: unsafe extern "C" fn() -> u64,
    pub rng_fill: unsafe extern "C" fn(buf: *mut u8, len: usize),
    // --- ABI 14 ---
    /// The `index`-th device-tree node whose `compatible` list has
    /// `compatible` (e.g. `virtio,mmio`), in ascending address order, with
    /// its registers mapped: 0 and `*out` filled, or -1 (no such node, or no
    /// device tree on this arch).
    pub dt_mmio_find: unsafe extern "C" fn(compatible: StrRef, index: usize, out: *mut MmioDevice) -> i32,
    // --- ABI 15: VFS paths without an fd (the Linux layer's sockets) ---
    /// Read up to `cap` bytes at `pos` of the file at VFS `path` (a real
    /// path, as `vfs_stat`'s) into kernel `buf`: bytes read, or negative.
    pub vfs_read: unsafe extern "C" fn(path: StrRef, pos: usize, buf: *mut u8, cap: usize) -> i32,
    /// Write kernel `buf` at `pos` of the file at VFS `path`: bytes
    /// written, or negative.
    pub vfs_write: unsafe extern "C" fn(path: StrRef, pos: usize, buf: *const u8, len: usize) -> i32,
    // --- ABI 18: terminals as files (docs/tty.md) ---
    /// The `ctl` text of the terminal `fd` is open on, into `buf` (cut at
    /// `cap`): its length, or negative when `fd` is not a terminal.
    pub tty_ctl_read: unsafe extern "C" fn(fd: usize, buf: *mut u8, cap: usize) -> i32,
    /// Write `len` bytes of ctl text to the terminal `fd` is open on: 0, or
    /// negative when `fd` is not a terminal or the text is refused.
    pub tty_ctl_write: unsafe extern "C" fn(fd: usize, text: *const u8, len: usize) -> i32,
    /// What `fd` is open on, as `/proc/self/fd` names it (`/dev/pts/3/data`,
    /// `/dev/pts/3/master`, `pipe:[N]`), into `buf`: its length, or negative.
    pub fd_path: unsafe extern "C" fn(fd: usize, buf: *mut u8, cap: usize) -> i32,
    // --- ABI 21: hot-pluggable buses (docs/usb.md) ---
    /// Take the block device `dev` (a `blk_register` id) out of `/dev`: 0,
    /// or [`MYOS_EBUSY`] while a filesystem is mounted from it or an fd is
    /// open on it (the driver then keeps the device, failing its I/O).
    pub blk_unregister: unsafe extern "C" fn(dev: u32) -> i32,
    /// Publish `table` (a `#[repr(C)]` function table that lives as long as
    /// the module) under `name` for other modules: a bus its class drivers
    /// reach through `service_lookup` ([`USB_SERVICE`]). 0, or negative
    /// (name taken, table full). Counted as a registration.
    pub service_register: unsafe extern "C" fn(name: StrRef, table: *const core::ffi::c_void) -> i32,
    /// The table published under `name`, or null. A module that found one
    /// stays loaded (`rmmod` refuses it): the table's owner may call back
    /// into it.
    pub service_lookup: unsafe extern "C" fn(name: StrRef) -> *const core::ffi::c_void,
    /// A kernel thread running `entry(ctx)` in task context (it may block
    /// on `block_until`, sleep, and call every `KernelApi` function a
    /// syscall may): 0, or negative (no thread slot left: [`MYOS_MAX_MODULE_THREADS`]).
    /// The module stays loaded while it runs.
    pub thread_spawn: unsafe extern "C" fn(
        name: StrRef,
        entry: unsafe extern "C" fn(ctx: *mut core::ffi::c_void),
        ctx: *mut core::ffi::c_void,
    ) -> i32,
    /// Wake the tasks blocked on `key` (`block_until`), and the `poll`
    /// sleepers. Safe from interrupt context.
    pub wake: unsafe extern "C" fn(key: usize),
    // --- ABI 22 ---
    /// Fork, the child resuming like the caller of the syscall in `regs`
    /// (result 0) on stack `sp`: its pid, or negative.
    pub fork_from: unsafe extern "C" fn(regs: *mut u64, sp: usize) -> i32,
    // --- ABI 23 ---
    /// Drop the pages of `[addr, addr + len)` in the caller's mmap window:
    /// they read as new on the next touch (zero, or the file's contents).
    /// 0, or negative when the range is outside the window.
    pub mmap_discard: unsafe extern "C" fn(addr: usize, len: usize) -> i32,
    // --- ABI 25 ---
    /// Set the access and modification times (seconds since the epoch,
    /// [`MYOS_TIME_OMIT`] keeps one) of the file at VFS `path` (a real
    /// path, as `vfs_stat`'s): 0, or negative (no such file, or a mount
    /// that keeps no times).
    pub vfs_set_times: unsafe extern "C" fn(path: StrRef, atime: u64, mtime: u64) -> i32,
    // --- ABI 27 ---
    /// Give thread `tid`, new from `thread_spawn_from` and not run yet, a
    /// CPU of its own: until then it waits for its creator's syscall to
    /// end, so what it must find when it starts is set up first.
    pub thread_place: unsafe extern "C" fn(tid: i32),
    // --- ABI 30 ---
    /// The native `lockctl(fd, cmd, lock)` (`kernel/src/user/syscall.rs`)
    /// with `lock` in kernel memory: a record lock on `fd`'s file
    /// ([`MyosLockRange`]). 0, or a native failure value
    /// ([`MYOS_SYSERR_EAGAIN`], [`MYOS_SYSERR_EINTR`]).
    pub fd_lockctl: unsafe extern "C" fn(fd: usize, cmd: usize, lock: *mut MyosLockRange) -> usize,
    // --- ABI 31 ---
    /// Add `method` for `action` ([`MYOS_POWER_OFF`], [`MYOS_POWER_REBOOT`]),
    /// tried before the arch's own methods once the system is going down
    /// (`docs/power.md`): it returns only when it failed. `name` is for the
    /// log (`power off via NAME`). 0, or negative. Counted as a
    /// registration.
    pub power_register: unsafe extern "C" fn(action: u32, name: StrRef, method: unsafe extern "C" fn()) -> i32,
}

/// Where a module keeps the table `module_init` received: set once there,
/// read through [`ApiCell::get`] by every later call, from any CPU.
pub struct ApiCell(AtomicPtr<KernelApi>);

impl ApiCell {
    pub const fn new() -> Self {
        Self(AtomicPtr::new(core::ptr::null_mut()))
    }

    /// Keep `api` for [`ApiCell::get`].
    ///
    /// # Safety
    ///
    /// `api` is the table `module_init` received: the kernel keeps it for as
    /// long as the module is loaded.
    pub unsafe fn set(&self, api: *const KernelApi) {
        self.0.store(api.cast_mut(), Ordering::Release);
    }

    /// The table, or `None` before `module_init` kept it.
    pub fn try_get(&self) -> Option<&'static KernelApi> {
        // SAFETY: `set` was handed a table that outlives the module.
        unsafe { self.0.load(Ordering::Acquire).as_ref() }
    }

    /// The table; panics before `module_init` kept it.
    pub fn get(&self) -> &'static KernelApi {
        self.try_get().expect("KernelApi not set")
    }
}

impl Default for ApiCell {
    fn default() -> Self {
        Self::new()
    }
}

impl StrRef {
    /// `s` as the kernel takes it (it copies what it keeps).
    pub const fn new(s: &str) -> Self {
        Self::from_bytes(s.as_bytes())
    }

    pub const fn from_bytes(s: &[u8]) -> Self {
        Self {
            ptr: s.as_ptr(),
            len: s.len(),
        }
    }
}

/// Safe calls into the kernel: a method per table entry, of the same name,
/// taking slices and references where the entry takes pointer and length
/// pairs and out pointers (`api.blk_read(dev, lba, buf)` for
/// `unsafe { (api.blk_read)(dev, lba, buf.as_mut_ptr(), buf.len()) }`).
/// The kernel copies or checks everything these hand it (names, function
/// tables, user addresses), so a module needs no `unsafe` to call them.
/// The entries that stay `unsafe` have no method and are called through
/// their field: `dealloc` (a pointer from `alloc`), the saved registers of
/// a syscall (`native_syscall`, `fork_from`, `thread_spawn_from`,
/// `personality_exec`'s strings) and the FP/SIMD save area (`fpu_save`,
/// `fpu_restore`, 16-byte aligned).
impl KernelApi {
    pub fn write_str(&self, s: &str) {
        self.write_bytes(s.as_bytes());
    }

    /// `write_str` for bytes that need not be UTF-8.
    pub fn write_bytes(&self, s: &[u8]) {
        unsafe { (self.write_str)(s.as_ptr(), s.len()) }
    }

    pub fn alloc(&self, size: usize, align: usize) -> *mut u8 {
        unsafe { (self.alloc)(size, align) }
    }

    pub fn blk_read(&self, dev: u32, lba: u64, buf: &mut [u8]) -> i32 {
        unsafe { (self.blk_read)(dev, lba, buf.as_mut_ptr(), buf.len()) }
    }

    pub fn vfs_register(&self, name: &str, data: &[u8]) -> i32 {
        unsafe { (self.vfs_register)(name.as_ptr(), name.len(), data.as_ptr(), data.len()) }
    }

    pub fn vfs_register_static(&self, name: &str, data: &'static [u8]) -> i32 {
        unsafe { (self.vfs_register_static)(name.as_ptr(), name.len(), data.as_ptr(), data.len()) }
    }

    pub fn vfs_mount(&self, name: &str, prefix: &str, ops: &ModuleVfsOps) -> i32 {
        unsafe { (self.vfs_mount)(name.as_ptr(), name.len(), prefix.as_ptr(), prefix.len(), ops) }
    }

    pub fn blk_write(&self, dev: u32, lba: u64, buf: &[u8]) -> i32 {
        unsafe { (self.blk_write)(dev, lba, buf.as_ptr(), buf.len()) }
    }

    pub fn blk_count(&self) -> u32 {
        unsafe { (self.blk_count)() }
    }

    pub fn fs_register(&self, name: &str, bind: FsBind) -> i32 {
        unsafe { (self.fs_register)(name.as_ptr(), name.len(), bind) }
    }

    pub fn blk_read_at(&self, dev: u32, offset: u64, buf: &mut [u8]) -> i32 {
        unsafe { (self.blk_read_at)(dev, offset, buf.as_mut_ptr(), buf.len()) }
    }

    pub fn blk_write_at(&self, dev: u32, offset: u64, buf: &[u8]) -> i32 {
        unsafe { (self.blk_write_at)(dev, offset, buf.as_ptr(), buf.len()) }
    }

    pub fn pci_cfg_read32(&self, bus: u8, slot: u8, func: u8, off: u8) -> u32 {
        unsafe { (self.pci_cfg_read32)(bus, slot, func, off) }
    }

    pub fn pci_cfg_write32(&self, bus: u8, slot: u8, func: u8, off: u8, val: u32) {
        unsafe { (self.pci_cfg_write32)(bus, slot, func, off, val) }
    }

    pub fn pci_enable(&self, bus: u8, slot: u8, func: u8) {
        unsafe { (self.pci_enable)(bus, slot, func) }
    }

    pub fn pci_find(
        &self,
        vendor: u16,
        device: u16,
        index: u32,
        bus: &mut u8,
        slot: &mut u8,
        func: &mut u8,
    ) -> i32 {
        unsafe { (self.pci_find)(vendor, device, index, bus, slot, func) }
    }

    pub fn pci_bar_map(&self, bus: u8, slot: u8, func: u8, bar: u8, va: &mut usize, size: &mut u64) -> i32 {
        unsafe { (self.pci_bar_map)(bus, slot, func, bar, va, size) }
    }

    pub fn dma_alloc(&self, n_pages: usize, phys: &mut u64) -> *mut u8 {
        unsafe { (self.dma_alloc)(n_pages, phys) }
    }

    pub fn dev_register(&self, name: &str, ops: &ModuleChrOps) -> i32 {
        unsafe { (self.dev_register)(name.as_ptr(), name.len(), ops) }
    }

    pub fn copy_to_user(&self, dst_user: usize, src: &[u8]) -> i32 {
        unsafe { (self.copy_to_user)(dst_user, src.as_ptr(), src.len()) }
    }

    pub fn proc_register(&self, name: &str, data: &[u8]) -> i32 {
        unsafe { (self.proc_register)(name.as_ptr(), name.len(), data.as_ptr(), data.len()) }
    }

    pub fn acpi_rsdp(&self) -> usize {
        unsafe { (self.acpi_rsdp)() }
    }

    pub fn hhdm_offset(&self) -> u64 {
        unsafe { (self.hhdm_offset)() }
    }

    pub fn proc_set_writer(
        &self,
        name: &str,
        writer: Option<unsafe extern "C" fn(*const u8, usize) -> i32>,
    ) -> i32 {
        unsafe { (self.proc_set_writer)(name.as_ptr(), name.len(), writer) }
    }

    /// `ctx` is handed back to `handler`, which says what it points to.
    pub fn pci_irq_enable(
        &self,
        bus: u8,
        slot: u8,
        func: u8,
        name: &str,
        handler: IrqHandler,
        ctx: *mut core::ffi::c_void,
        msix_entry: &mut u16,
    ) -> i32 {
        unsafe { (self.pci_irq_enable)(bus, slot, func, name.as_ptr(), name.len(), handler, ctx, msix_entry) }
    }

    pub fn wake_any(&self) {
        unsafe { (self.wake_any)() }
    }

    pub fn wait_seq(&self) -> u64 {
        unsafe { (self.wait_seq)() }
    }

    pub fn block_until(&self, key: usize, seq: u64, deadline_ns: u64) {
        unsafe { (self.block_until)(key, seq, deadline_ns) }
    }

    pub fn monotonic_ns(&self) -> u64 {
        unsafe { (self.monotonic_ns)() }
    }

    pub fn blk_register(&self, name: &str, ops: &ModuleBlkOps, ctx: usize) -> i32 {
        unsafe { (self.blk_register)(name.as_ptr(), name.len(), ops, ctx) }
    }

    pub fn pci_find_class(
        &self,
        class: u8,
        subclass: u8,
        index: u32,
        bus: &mut u8,
        slot: &mut u8,
        func: &mut u8,
    ) -> i32 {
        unsafe { (self.pci_find_class)(class, subclass, index, bus, slot, func) }
    }

    pub fn framebuffer_info(&self, out: &mut FramebufferInfo) -> i32 {
        unsafe { (self.framebuffer_info)(out) }
    }

    pub fn console_register(&self, ops: &ModuleConsoleOps) -> i32 {
        unsafe { (self.console_register)(ops) }
    }

    pub fn personality_register(&self, ops: &PersonalityOps) -> i32 {
        unsafe { (self.personality_register)(ops) }
    }

    pub fn copy_from_user(&self, src_user: usize, dst: &mut [u8]) -> i32 {
        unsafe { (self.copy_from_user)(src_user, dst.as_mut_ptr(), dst.len()) }
    }

    pub fn user_buffer_ok(&self, ptr: usize, len: usize) -> bool {
        unsafe { (self.user_buffer_ok)(ptr, len) != 0 }
    }

    pub fn current_tid(&self) -> usize {
        unsafe { (self.current_tid)() }
    }

    pub fn current_pid(&self) -> usize {
        unsafe { (self.current_pid)() }
    }

    pub fn current_ppid(&self) -> usize {
        unsafe { (self.current_ppid)() }
    }

    pub fn task_is_live_user(&self, id: usize) -> bool {
        unsafe { (self.task_is_live_user)(id) != 0 }
    }

    pub fn task_has_root(&self) -> bool {
        unsafe { (self.task_has_root)() != 0 }
    }

    pub fn path_resolve(&self, path: &str, mode: u32, out: &mut [u8]) -> i32 {
        unsafe { (self.path_resolve)(StrRef::new(path), mode, out.as_mut_ptr(), out.len()) }
    }

    pub fn vfs_stat(&self, path: &str, out: &mut PathStat) -> i32 {
        unsafe { (self.vfs_stat)(StrRef::new(path), out) }
    }

    pub fn vfs_listdir(&self, path: &str, buf: &mut [u8]) -> i32 {
        unsafe { (self.vfs_listdir)(StrRef::new(path), buf.as_mut_ptr(), buf.len()) }
    }

    pub fn vfs_mkdir(&self, path: &str) -> i32 {
        unsafe { (self.vfs_mkdir)(StrRef::new(path)) }
    }

    pub fn vfs_rmdir(&self, path: &str) -> i32 {
        unsafe { (self.vfs_rmdir)(StrRef::new(path)) }
    }

    pub fn vfs_unlink(&self, path: &str) -> i32 {
        unsafe { (self.vfs_unlink)(StrRef::new(path)) }
    }

    pub fn vfs_rename(&self, old: &str, new: &str) -> i32 {
        unsafe { (self.vfs_rename)(StrRef::new(old), StrRef::new(new)) }
    }

    pub fn vfs_symlink(&self, target: &str, link: &str) -> i32 {
        unsafe { (self.vfs_symlink)(StrRef::new(target), StrRef::new(link)) }
    }

    pub fn vfs_readlink(&self, path: &str, buf: &mut [u8]) -> i32 {
        unsafe { (self.vfs_readlink)(StrRef::new(path), buf.as_mut_ptr(), buf.len()) }
    }

    pub fn open_path(&self, path: &str, flags: usize) -> usize {
        unsafe { (self.open_path)(StrRef::new(path), flags) }
    }

    pub fn chdir_path(&self, path: &str) -> usize {
        unsafe { (self.chdir_path)(StrRef::new(path)) }
    }

    pub fn fd_pread(&self, fd: usize, pos: usize, buf: &mut [u8]) -> i32 {
        unsafe { (self.fd_pread)(fd, pos, buf.as_mut_ptr(), buf.len()) }
    }

    pub fn fd_kind(&self, fd: usize, size: &mut usize) -> i32 {
        unsafe { (self.fd_kind)(fd, size) }
    }

    pub fn fd_poll_bits(&self, fd: usize) -> i32 {
        unsafe { (self.fd_poll_bits)(fd) }
    }

    pub fn fd_dup_min(&self, fd: usize, min: usize) -> i32 {
        unsafe { (self.fd_dup_min)(fd, min) }
    }

    pub fn fd_dup2(&self, old: usize, new: usize) -> i32 {
        unsafe { (self.fd_dup2)(old, new) }
    }

    pub fn fd_close(&self, fd: usize) -> i32 {
        unsafe { (self.fd_close)(fd) }
    }

    pub fn fd_write(&self, fd: usize, buf_user: usize, len: usize) -> usize {
        unsafe { (self.fd_write)(fd, buf_user, len) }
    }

    pub fn pipe_open(&self, read_fd: &mut usize, write_fd: &mut usize) -> i32 {
        unsafe { (self.pipe_open)(read_fd, write_fd) }
    }

    pub fn mmap(&self, addr: usize, len: usize, prot: usize, flags: usize, fd: isize, off: usize) -> usize {
        unsafe { (self.mmap)(addr, len, prot, flags, fd, off) }
    }

    pub fn signal_get_action(&self, id: usize, sig: u32, handler: &mut usize, flags: &mut u32, mask: &mut u32) {
        unsafe { (self.signal_get_action)(id, sig, handler, flags, mask) }
    }

    pub fn signal_set_action(&self, id: usize, sig: u32, handler: usize, flags: u32, mask: u32, tramp: usize) -> i32 {
        unsafe { (self.signal_set_action)(id, sig, handler, flags, mask, tramp) }
    }

    pub fn signal_blocked(&self, id: usize) -> u32 {
        unsafe { (self.signal_blocked)(id) }
    }

    pub fn signal_set_blocked(&self, id: usize, mask: u32) {
        unsafe { (self.signal_set_blocked)(id, mask) }
    }

    pub fn signal_pending(&self, id: usize) -> u32 {
        unsafe { (self.signal_pending)(id) }
    }

    pub fn signal_take(&self, id: usize, set: u32) -> i32 {
        unsafe { (self.signal_take)(id, set) }
    }

    pub fn signal_kill(&self, pid: isize, sig: u32) -> i32 {
        unsafe { (self.signal_kill)(pid, sig) }
    }

    pub fn signal_sigsuspend(&self, mask: u32) -> usize {
        unsafe { (self.signal_sigsuspend)(mask) }
    }

    pub fn signal_interrupt_wait(&self) -> bool {
        unsafe { (self.signal_interrupt_wait)() != 0 }
    }

    pub fn signal_terminate(&self, sig: u32) -> ! {
        unsafe { (self.signal_terminate)(sig) }
    }

    pub fn thread_pointer_get(&self) -> u64 {
        unsafe { (self.thread_pointer_get)() }
    }

    pub fn thread_pointer_set(&self, v: u64) {
        unsafe { (self.thread_pointer_set)(v) }
    }

    pub fn thread_exit(&self, code: u8) -> ! {
        unsafe { (self.thread_exit)(code) }
    }

    pub fn wait_addr(&self, addr: usize, expected: u32, deadline_ns: u64) -> i32 {
        unsafe { (self.wait_addr)(addr, expected, deadline_ns) }
    }

    pub fn wake_addr(&self, addr: usize, max: usize) -> usize {
        unsafe { (self.wake_addr)(addr, max) }
    }

    pub fn task_sleep_until(&self, deadline_ns: u64) {
        unsafe { (self.task_sleep_until)(deadline_ns) }
    }

    pub fn task_yield(&self) {
        unsafe { (self.task_yield)() }
    }

    pub fn wall_time_us(&self) -> u64 {
        unsafe { (self.wall_time_us)() }
    }

    pub fn rng_fill(&self, buf: &mut [u8]) {
        unsafe { (self.rng_fill)(buf.as_mut_ptr(), buf.len()) }
    }

    pub fn dt_mmio_find(&self, compatible: &str, index: usize, out: &mut MmioDevice) -> i32 {
        unsafe { (self.dt_mmio_find)(StrRef::new(compatible), index, out) }
    }

    pub fn vfs_read(&self, path: &str, pos: usize, buf: &mut [u8]) -> i32 {
        unsafe { (self.vfs_read)(StrRef::new(path), pos, buf.as_mut_ptr(), buf.len()) }
    }

    pub fn vfs_write(&self, path: &str, pos: usize, buf: &[u8]) -> i32 {
        unsafe { (self.vfs_write)(StrRef::new(path), pos, buf.as_ptr(), buf.len()) }
    }

    pub fn tty_ctl_read(&self, fd: usize, buf: &mut [u8]) -> i32 {
        unsafe { (self.tty_ctl_read)(fd, buf.as_mut_ptr(), buf.len()) }
    }

    pub fn tty_ctl_write(&self, fd: usize, text: &[u8]) -> i32 {
        unsafe { (self.tty_ctl_write)(fd, text.as_ptr(), text.len()) }
    }

    pub fn fd_path(&self, fd: usize, buf: &mut [u8]) -> i32 {
        unsafe { (self.fd_path)(fd, buf.as_mut_ptr(), buf.len()) }
    }

    pub fn blk_unregister(&self, dev: u32) -> i32 {
        unsafe { (self.blk_unregister)(dev) }
    }

    /// Other modules reach `table` as a `T` (`UsbHostOps` for [`USB_SERVICE`]).
    pub fn service_register<T: Sync>(&self, name: &str, table: &'static T) -> i32 {
        unsafe { (self.service_register)(StrRef::new(name), (table as *const T).cast()) }
    }

    /// The table published under `name`, or null; the caller knows its type.
    pub fn service_lookup(&self, name: &str) -> *const core::ffi::c_void {
        unsafe { (self.service_lookup)(StrRef::new(name)) }
    }

    /// `ctx` is handed to `entry`, which says what it points to.
    pub fn thread_spawn(
        &self,
        name: &str,
        entry: unsafe extern "C" fn(ctx: *mut core::ffi::c_void),
        ctx: *mut core::ffi::c_void,
    ) -> i32 {
        unsafe { (self.thread_spawn)(StrRef::new(name), entry, ctx) }
    }

    pub fn wake(&self, key: usize) {
        unsafe { (self.wake)(key) }
    }

    pub fn mmap_discard(&self, addr: usize, len: usize) -> i32 {
        unsafe { (self.mmap_discard)(addr, len) }
    }

    pub fn vfs_set_times(&self, path: &str, atime: u64, mtime: u64) -> i32 {
        unsafe { (self.vfs_set_times)(StrRef::new(path), atime, mtime) }
    }

    pub fn thread_place(&self, tid: i32) {
        unsafe { (self.thread_place)(tid) }
    }

    pub fn fd_lockctl(&self, fd: usize, cmd: usize, lock: &mut MyosLockRange) -> usize {
        unsafe { (self.fd_lockctl)(fd, cmd, lock) }
    }

    /// # Safety
    ///
    /// `method` can run at any time, from any CPU, until the machine goes
    /// down: it must stay valid as long as the module is loaded (the
    /// registration keeps it loaded).
    pub unsafe fn power_register(&self, action: u32, name: &str, method: unsafe extern "C" fn()) -> i32 {
        unsafe { (self.power_register)(action, StrRef::new(name), method) }
    }
}

/// The native `power(action)`'s actions, and [`KernelApi::power_register`]'s.
pub const MYOS_POWER_OFF: u32 = 0;
pub const MYOS_POWER_REBOOT: u32 = 1;
pub const MYOS_POWER_HALT: u32 = 2;

/// A record lock for `fd_lockctl` and the native `lockctl`: `kind` 0
/// shared (`F_RDLCK`), 1 exclusive (`F_WRLCK`), 2 none (`F_UNLCK`); the
/// bytes `start..start + len` (`len` 0: to the file's end); `pid` the owner
/// a get found (-1 for an open file description's).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MyosLockRange {
    pub kind: u32,
    pub _pad: u32,
    pub start: u64,
    pub len: u64,
    pub pid: i64,
}

/// `lockctl` commands, and the flag that makes the open file description
/// own the lock rather than the process (Linux's `F_OFD_*`).
pub const MYOS_LOCKCTL_GET: usize = 0;
pub const MYOS_LOCKCTL_SET: usize = 1;
pub const MYOS_LOCKCTL_WAIT: usize = 2;
pub const MYOS_LOCKCTL_OFD: usize = 0x10;

/// `blk_unregister`: the device is mounted or open.
pub const MYOS_EBUSY: i32 = -16;
/// Kernel threads modules may run (`thread_spawn`), in all.
pub const MYOS_MAX_MODULE_THREADS: usize = 8;

// ---- ABI 21: the USB bus (docs/usb.md) ------------------------------------
//
// A host controller module (`xhci`) publishes a [`UsbHostOps`] table as the
// service [`USB_SERVICE`]; class driver modules (`usb_hub`, `usb_storage`)
// look it up and register a [`UsbDriverOps`]. The host enumerates devices
// on its own thread (`thread_spawn`) and offers every interface to the
// drivers there, so `probe` and `disconnect` run in task context and may
// block in the host's transfers.

/// The name the host controller publishes its [`UsbHostOps`] under.
pub const USB_SERVICE: &str = "usb";
/// [`UsbHostOps::version`]. 2 added [`UsbHostOps::interface_label`].
pub const USB_HOST_VERSION: u32 = 2;

/// Device speeds ([`UsbDeviceInfo::speed`], `hub_attach`).
pub const USB_SPEED_LOW: u8 = 1;
pub const USB_SPEED_FULL: u8 = 2;
pub const USB_SPEED_HIGH: u8 = 3;
pub const USB_SPEED_SUPER: u8 = 4;

/// Transfer results below zero: the device is gone, the endpoint stalled
/// (`clear_halt` recovers it), the controller reported another error, the
/// transfer did not complete in time.
pub const USB_EGONE: i32 = -6;
pub const USB_ESTALL: i32 = -32;
pub const USB_EIO: i32 = -5;
pub const USB_ETIMEDOUT: i32 = -110;

/// Endpoints one interface may have ([`UsbInterfaceInfo::endpoints`]).
pub const USB_MAX_ENDPOINTS: usize = 15;
/// Bytes of an interface's label ([`UsbHostOps::interface_label`]).
pub const USB_LABEL_MAX: usize = 8;

/// One endpoint of an interface, as its descriptor says: the address
/// (direction in bit 7: IN), the attributes (transfer type in bits 1:0:
/// 0 control, 1 isochronous, 2 bulk, 3 interrupt), the max packet size and
/// the polling interval.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct UsbEndpoint {
    pub address: u8,
    pub attributes: u8,
    pub max_packet: u16,
    pub interval: u8,
}

/// An interface of the device's active configuration, as offered to
/// [`UsbDriverOps::probe`].
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct UsbInterfaceInfo {
    pub number: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub n_endpoints: u8,
    pub endpoints: [UsbEndpoint; USB_MAX_ENDPOINTS],
}

/// A device the host enumerated: `id` names it in every [`UsbHostOps`]
/// call; `parent` is the hub it hangs from (0: a root port) and `port` the
/// port number on it; `depth` the number of hubs above it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UsbDeviceInfo {
    pub id: u32,
    pub parent: u32,
    pub vendor: u16,
    pub product: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub speed: u8,
    pub port: u8,
    pub depth: u8,
    pub n_interfaces: u8,
    /// `bConfigurationValue` of the active configuration.
    pub config: u8,
}

/// The completion of a transfer queued with `interrupt_start`: `status` is
/// the bytes moved, or a `USB_E*` error ([`USB_EGONE`]: the device went
/// away; the transfer is not re-queued).
pub type UsbCompletion = unsafe extern "C" fn(ctx: *mut core::ffi::c_void, status: i32);

/// The USB host controller, for class drivers (the [`USB_SERVICE`] table).
/// Transfers block until they complete (task context only); the hub calls
/// run on the host's thread (from a hub driver's completion callback).
#[repr(C)]
pub struct UsbHostOps {
    pub version: u32,
    /// Register a class driver. The host offers it, on its thread, every
    /// interface no driver claimed yet and every one enumerated later.
    pub driver_register: unsafe extern "C" fn(ops: *const UsbDriverOps) -> i32,
    /// A control transfer on endpoint 0 (`request_type` bit 7: device to
    /// host): the bytes moved in the data stage, or a `USB_E*` error.
    pub control: unsafe extern "C" fn(
        dev: u32,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: *mut u8,
        len: u16,
    ) -> i32,
    /// A bulk transfer on `endpoint` (the address with its direction bit):
    /// bytes moved (a short IN transfer moves less than `len`), or an error.
    pub bulk: unsafe extern "C" fn(dev: u32, endpoint: u8, data: *mut u8, len: usize, timeout_ms: u32) -> i32,
    /// Queue one interrupt IN transfer on `endpoint`; `done(ctx, status)`
    /// runs on the host's thread when it completes. One outstanding
    /// transfer per endpoint. 0, or negative.
    pub interrupt_start: unsafe extern "C" fn(
        dev: u32,
        endpoint: u8,
        data: *mut u8,
        len: usize,
        done: UsbCompletion,
        ctx: *mut core::ffi::c_void,
    ) -> i32,
    /// Recover a stalled bulk or interrupt endpoint. 0 ok.
    pub clear_halt: unsafe extern "C" fn(dev: u32, endpoint: u8) -> i32,
    /// `dev` is a hub with `ports` downstream ports (a USB 2 hub: its
    /// transaction translator think time and whether it has one per port):
    /// the controller learns so before children are attached. 0 ok.
    pub hub_configure: unsafe extern "C" fn(dev: u32, ports: u8, tt_think: u8, multi_tt: u8) -> i32,
    /// A device at `speed` is on `port` of hub `dev`, reset and enabled:
    /// enumerate it as `dev`'s child and offer its interfaces. Its id, or
    /// negative.
    pub hub_attach: unsafe extern "C" fn(dev: u32, port: u8, speed: u8) -> i32,
    /// The device on `port` of hub `dev` is gone: its drivers are told, its
    /// children (a hub's) first, and its slot freed.
    pub hub_detach: unsafe extern "C" fn(dev: u32, port: u8),
    /// `dev`'s description into `*info`: 0, or [`USB_EGONE`].
    pub device_info: unsafe extern "C" fn(dev: u32, info: *mut UsbDeviceInfo) -> i32,
    /// What the driver that took interface `intf` of `dev` made of it (a
    /// disk's name), shown after the driver's name in `/proc/usb`: at most
    /// [`USB_LABEL_MAX`] bytes, longer is cut. 0, or [`USB_EGONE`].
    pub interface_label: unsafe extern "C" fn(dev: u32, intf: u8, label: *const u8, len: usize) -> i32,
}

/// Safe calls into the host, as [`KernelApi`]'s methods: the host checks
/// what it is handed. `interrupt_start` stays `unsafe` (its buffer is
/// written after it returns) and is called through its field.
impl UsbHostOps {
    pub fn driver_register(&self, ops: &'static UsbDriverOps) -> i32 {
        unsafe { (self.driver_register)(ops) }
    }

    /// A control transfer whose data stage is `data` (empty: none; at most
    /// `u16::MAX` bytes of it are used).
    pub fn control(&self, dev: u32, request_type: u8, request: u8, value: u16, index: u16, data: &mut [u8]) -> i32 {
        let len = data.len().min(u16::MAX as usize) as u16;
        unsafe { (self.control)(dev, request_type, request, value, index, data.as_mut_ptr(), len) }
    }

    pub fn bulk(&self, dev: u32, endpoint: u8, data: &mut [u8], timeout_ms: u32) -> i32 {
        unsafe { (self.bulk)(dev, endpoint, data.as_mut_ptr(), data.len(), timeout_ms) }
    }

    /// `bulk` on an OUT endpoint: the host only reads `data`.
    pub fn bulk_out(&self, dev: u32, endpoint: u8, data: &[u8], timeout_ms: u32) -> i32 {
        unsafe { (self.bulk)(dev, endpoint, data.as_ptr().cast_mut(), data.len(), timeout_ms) }
    }

    pub fn clear_halt(&self, dev: u32, endpoint: u8) -> i32 {
        unsafe { (self.clear_halt)(dev, endpoint) }
    }

    pub fn hub_configure(&self, dev: u32, ports: u8, tt_think: u8, multi_tt: u8) -> i32 {
        unsafe { (self.hub_configure)(dev, ports, tt_think, multi_tt) }
    }

    pub fn hub_attach(&self, dev: u32, port: u8, speed: u8) -> i32 {
        unsafe { (self.hub_attach)(dev, port, speed) }
    }

    pub fn hub_detach(&self, dev: u32, port: u8) {
        unsafe { (self.hub_detach)(dev, port) }
    }

    pub fn device_info(&self, dev: u32, info: &mut UsbDeviceInfo) -> i32 {
        unsafe { (self.device_info)(dev, info) }
    }

    pub fn interface_label(&self, dev: u32, intf: u8, label: &[u8]) -> i32 {
        unsafe { (self.interface_label)(dev, intf, label.as_ptr(), label.len()) }
    }
}

// Function tables with a name: shared between the modules' threads.
unsafe impl Sync for UsbHostOps {}
unsafe impl Sync for UsbDriverOps {}

/// A USB class driver (`UsbHostOps::driver_register`).
#[repr(C)]
pub struct UsbDriverOps {
    pub name: StrRef,
    /// An interface no driver claimed: 0 takes it (the driver owns its
    /// endpoints from now on), negative passes.
    pub probe: unsafe extern "C" fn(dev: *const UsbDeviceInfo, intf: *const UsbInterfaceInfo) -> i32,
    /// The device behind an interface this driver took is gone: forget it
    /// (its transfers fail with [`USB_EGONE`] from now on).
    pub disconnect: unsafe extern "C" fn(dev: u32, intf: u8),
}

/// A memory-mapped device from the device tree (`KernelApi::dt_mmio_find`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MmioDevice {
    /// Kernel virtual address of the register window.
    pub base: usize,
    pub size: usize,
    /// Interrupt number as `irq::dispatch` / `irq_register` see it; 0 when
    /// the node has none or the kernel cannot route it.
    pub irq: u32,
}

/// Emit `[ OK ] label\n` via `KernelApi::write_str` (same spacing as `console::status_ok`).
pub fn status_ok(api: &KernelApi, label: &str) {
    api.write_str("[ OK ] ");
    api.write_str(label);
    api.write_str("\n");
}

/// Emit `[ FAIL ] label\n` via `KernelApi::write_str`.
pub fn status_fail(api: &KernelApi, label: &str) {
    api.write_str("[ FAIL ] ");
    api.write_str(label);
    api.write_str("\n");
}

/// Emit `[ INFO ] label\n` via `KernelApi::write_str`.
pub fn status_info(api: &KernelApi, label: &str) {
    api.write_str("[ INFO ] ");
    api.write_str(label);
    api.write_str("\n");
}

/// Emit `[ WARN ] label\n` via `KernelApi::write_str`.
pub fn status_warn(api: &KernelApi, label: &str) {
    api.write_str("[ WARN ] ");
    api.write_str(label);
    api.write_str("\n");
}

/// `module_init` — required. Return 0 on success.
pub type ModuleInit = unsafe extern "C" fn(*const KernelApi) -> i32;

/// `module_exit` — optional cleanup, run by `rmmod` once nothing the module
/// registered is left (the kernel counts a module's registrations and
/// refuses to unload one that still provides something).
pub type ModuleExit = unsafe extern "C" fn();

/// `module_rescan` — optional: probe for devices that appeared since
/// `module_init` and register the new ones, leaving the known ones as they
/// are. The kernel calls it after a `rescan` written to `/proc/pci`.
pub type ModuleRescan = unsafe extern "C" fn();
