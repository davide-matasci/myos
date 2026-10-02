//! Function-pointer table the kernel hands to a loadable module.
//!
//! This is the modular ABI. Modules do not link against kernel `.dynsym`;
//! they receive a [`KernelApi`] from `module_init` and call through it.

#![no_std]

/// Bump this when [`KernelApi`] layout or meaning changes.
pub const ABI_VERSION: u32 = 13;

/// myos-specific: copy 6-byte MAC to the userspace pointer in `arg`.
/// Keep in sync with `user/net` / `user/lib` duplicates.
pub const MYOS_IOCTL_NET_GETMAC: u64 = 0x4d01;
/// Block until the NIC has a received frame, another kernel event a poller
/// cares about (`wake_any`), or `arg` nanoseconds passed (0 = driver cap).
/// Fails (negative) when the device has no RX interrupt, so callers fall back
/// to timed polling. Keep in sync with `user/net`.
pub const MYOS_IOCTL_NET_WAIT_RX: u64 = 0x4d02;
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
}

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
    /// Install a keymap from its text form (`KDSKMAP`). 0 ok, negative on error.
    pub keymap_load: unsafe extern "C" fn(text: *const u8, len: usize) -> i32,
    /// 1 when a keymap is loaded (`KDGKMAP`).
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

/// Native syscall results: anything from [`MYOS_SYSERR_EINTR`] up is a
/// failure sentinel (`usize::MAX` the generic one).
pub const MYOS_SYSERR: usize = usize::MAX;
pub const MYOS_SYSERR_EIO: usize = usize::MAX - 1;
pub const MYOS_SYSERR_ENXIO: usize = usize::MAX - 2;
pub const MYOS_SYSERR_EINTR: usize = usize::MAX - 3;

/// Native signal dispositions (`signal_get_action`): default, ignore; any
/// other value is a caught handler's address. Native signal numbers are
/// newlib / BSD (`SIGKILL` 9, `SIGSEGV` 11).
pub const MYOS_HANDLER_DFL: usize = 0;
pub const MYOS_HANDLER_IGN: usize = 1;
pub const MYOS_SIGSEGV: u32 = 11;

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

/// Module-provided character device ops for `/dev` nodes.
/// Kernel forces `S_IFCHR | 0666`. `read`/`write` return bytes (>=0) or a
/// negative error. `read` may return 0 when no data is ready (poll).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ModuleChrOps {
    pub read: unsafe extern "C" fn(buf: *mut u8, buf_len: usize) -> i32,
    pub write: unsafe extern "C" fn(buf: *const u8, buf_len: usize) -> i32,
    /// Optional. `None` → ENOTTY. `request` is an ioctl code; `arg` is a
    /// userspace pointer/value. Modules must not deref user pointers directly —
    /// use [`KernelApi::copy_to_user`] instead.
    pub ioctl: Option<unsafe extern "C" fn(request: u64, arg: usize) -> i32>,
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
    /// Wake every task sleeping on "any event" (pollers, `NET_WAIT_RX`).
    /// Safe from interrupt context.
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
    pub fd_ioctl: unsafe extern "C" fn(fd: usize, request: usize, arg: usize) -> usize,
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
}

/// Emit `[ OK ] label\n` via `KernelApi::write_str` (same spacing as `console::status_ok`).
pub fn status_ok(api: &KernelApi, label: &str) {
    unsafe {
        (api.write_str)(b"[ OK ] ".as_ptr(), 7);
        (api.write_str)(label.as_bytes().as_ptr(), label.len());
        (api.write_str)(b"\n".as_ptr(), 1);
    }
}

/// Emit `[ FAIL ] label\n` via `KernelApi::write_str`.
pub fn status_fail(api: &KernelApi, label: &str) {
    unsafe {
        (api.write_str)(b"[ FAIL ] ".as_ptr(), 9);
        (api.write_str)(label.as_bytes().as_ptr(), label.len());
        (api.write_str)(b"\n".as_ptr(), 1);
    }
}

/// Emit `[ INFO ] label\n` via `KernelApi::write_str`.
pub fn status_info(api: &KernelApi, label: &str) {
    unsafe {
        (api.write_str)(b"[ INFO ] ".as_ptr(), 9);
        (api.write_str)(label.as_bytes().as_ptr(), label.len());
        (api.write_str)(b"\n".as_ptr(), 1);
    }
}

/// Emit `[ WARN ] label\n` via `KernelApi::write_str`.
pub fn status_warn(api: &KernelApi, label: &str) {
    unsafe {
        (api.write_str)(b"[ WARN ] ".as_ptr(), 9);
        (api.write_str)(label.as_bytes().as_ptr(), label.len());
        (api.write_str)(b"\n".as_ptr(), 1);
    }
}

/// `module_init` — required. Return 0 on success.
pub type ModuleInit = unsafe extern "C" fn(*const KernelApi) -> i32;

/// `module_exit` — optional cleanup.
pub type ModuleExit = unsafe extern "C" fn();
