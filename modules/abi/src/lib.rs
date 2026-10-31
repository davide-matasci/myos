//! Function-pointer table the kernel hands to a loadable module.
//!
//! This is the modular ABI. Modules do not link against kernel `.dynsym`;
//! they receive a [`KernelApi`] from `module_init` and call through it.
//! The helpers a module keeps its own state in are here too: the table in
//! an [`ApiCell`], the rest behind a [`Lock`] (or a [`SleepLock`] when it
//! is held across a wait).

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

use core::sync::atomic::{AtomicPtr, Ordering};

mod lock;
mod sleep_lock;

pub use lock::{Lock, LockGuard};
pub use sleep_lock::{SleepGuard, SleepLock};

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
/// 32 added [`KernelApi::current_uid`].
/// 33 added [`KernelApi::irq_enable`] and [`KernelApi::console_input`] (an
/// interrupt-driven keyboard), and `keyboard_irq` / `cursor_blinks` to
/// [`ModuleConsoleOps`] (CPU 0 without its tick).
/// 34 made [`VfsStatInfo`]'s `size` 64 bits (sizes past 4 GiB).
/// 35 added [`ModuleVfsOps::statfs`] and [`KernelApi::vfs_statfs`] (how
/// full a filesystem is, [`VfsStatFs`]).
/// 36 added [`KernelApi::blk_name`] (a block device's name).
/// 37 added [`KernelApi::wait_take`] (a wait set's ready members, for the
/// Linux layer's `epoll_wait`).
/// 38 added [`KernelApi::user_mapped`] (the Linux layer's `mremap` tells
/// a mapped range from a hole).
/// 39 raised [`MYOS_MAX_TASKS`] to 512 and [`MYOS_MAX_FDS`] to 256.
/// 40 added [`KernelApi::fd_hold`], `fd_install` and `fd_release` (fds
/// passed over a unix socket, `SCM_RIGHTS`).
/// 41 added [`KernelApi::cpu_time`] and [`KernelApi::membarrier`].
/// 42: [`KernelApi::blk_register`] takes the disk's description
/// ([`DiskInfo`], `/dev/<disk>/info`); [`MmioDevice`] gives the window's
/// physical address.
/// 43: the console has text screens ([`CONSOLE_SCREENS`], `docs/tty.md`):
/// [`ModuleConsoleOps::write_kind`] names the screen, `show` switches, and
/// `keyboard_poll` reports Alt+digit ([`CONSOLE_KEY_SCREEN`]).
/// 45: a filesystem's `write` and `write_ino` hooks and
/// [`KernelApi::vfs_write`] tell a full filesystem apart ([`VFS_ENOSPC`],
/// the caller's `ENOSPC`).
/// 46: [`ModuleVfsOps::mmap`] says how its page is mapped
/// ([`MYOS_MMAP_UNCACHED`]: a device's registers),
/// [`KernelApi::pci_irq_enable`] gives a function without MSI-X its MSI
/// ([`MYOS_IRQ_MSI`], x86_64), and a PCI function has one owner at a time
/// ([`KernelApi::pci_claim`], `pci_release`, `pci_owner`).
/// 47: DMA is per device ([`KernelApi::dma_attach`], [`DmaDevice`]):
/// [`KernelApi::dma_alloc`] gives memory the device reaches and the address
/// it uses for it, [`KernelApi::dma_sync`] keeps a device without coherent
/// DMA and the caches in step.
/// 48: a process may ask for a signal when its parent ends
/// ([`KernelApi::task_set_pdeath_signal`], `task_pdeath_signal`).
/// 49: the IOMMU confines a function a program drives
/// ([`KernelApi::iommu_attach`], `iommu_map`, `iommu_detach`).
/// 50: a file's [`ModuleVfsOps::open`] may name another file for the fd
/// (a `clone` opens the new object's `data`, `docs/events.md`).
/// 51: a module drives a secondary interrupt controller as a domain
/// ([`KernelApi::irq_domain_register`], `irq_dispatch`); [`MmioDevice`]
/// gives the node's phandle.
/// 52: `/dev/pci` on any board: a BAR's address as the CPUs see it
/// ([`KernelApi::pci_bar_phys`]), whether a device's DMA sees the caches
/// ([`KernelApi::dma_coherent`]), memory mapped non-cacheable for one that
/// does not ([`MYOS_MMAP_NOCACHE`]).
/// 53: [`UsbHostOps::transfer_start`] (was `interrupt_start`) queues a
/// bulk IN transfer too.
/// 54: x86_64 routes a PCI function's legacy INTx through the I/O APIC
/// as the firmware's ACPI `_PRT` says ([`KernelApi::pci_intx_route`], the
/// acpi module's AML interpreter); [`KernelApi::phys_map`] maps physical
/// memory for it.
/// 55: [`KernelApi::pci_irq_disable`] gives a PCI function's interrupt
/// back.
/// 56: [`KernelApi::irq_disable`] gives a board device's interrupt back;
/// [`KernelApi::irq_domain_register`] takes the domain's `mask` beside its
/// `unmask`.
/// 57: a fault raises its signal: [`SignalDelivery`] carries the
/// siginfo's `code` and `addr`.
/// 58: sound ([`SOUND_SERVICE`]: the `sound` core's [`SoundOps`], a card's
/// [`SoundCardOps`]); USB alternate settings and isochronous OUT transfers
/// ([`UsbHostOps::set_interface`], [`UsbHostOps::isoch_queue`], the
/// controllers' [`UsbHcdOps::endpoints_change`]).
/// 59: a PCI function's I/O BAR ([`KernelApi::pci_io_map`], [`PciIo`]).
/// 60: no `PersonalityOps::flags` (every riscv64 user task has the FPU
/// on, so a personality need not ask).
/// 61: x86's FP/SIMD image is XSAVE's (`MYOS_FP_BYTES` 832, ymm0-15's
/// upper halves included), and the save area of `fpu_save` /
/// `fpu_restore` is 64-byte aligned on every arch.
/// 63: [`ModuleConsoleOps::status_line_try`]: a status line from an
/// interrupt, or nothing while the screen is held.
pub const ABI_VERSION: u32 = 63;

/// [`ModuleVfsOps::mmap`]: map the page uncached, as device registers
/// (x86 PCD|PWT, aarch64 Device-nGnRE; riscv64 takes it from the
/// platform's attributes for the address).
pub const MYOS_MMAP_UNCACHED: u32 = 1;
/// [`ModuleVfsOps::mmap`]: map the page, ordinary memory, non-cacheable:
/// a DMA buffer of a device that does not see the caches, so the program
/// needs no cache maintenance (x86 PCD|PWT, aarch64 Normal non-cacheable;
/// riscv64 cached, its DMA being coherent).
pub const MYOS_MMAP_NOCACHE: u32 = 2;

/// A time argument of [`ModuleVfsOps::set_times`] / [`KernelApi::vfs_set_times`]
/// that keeps the current value.
pub const MYOS_TIME_OMIT: u64 = u64::MAX;

/// `KernelApi::block_until` key woken by every `wake`, including `wake_any`.
pub const MYOS_WAIT_ANY: usize = usize::MAX;

/// Device interrupt handler (`KernelApi::pci_irq_enable`, `irq_enable`):
/// runs in interrupt context on the BSP with interrupts masked; must not
/// block, allocate or take a lock that code it interrupts may hold.
pub type IrqHandler = unsafe extern "C" fn(ctx: *mut core::ffi::c_void);
/// `pci_irq_enable` out value: the device is on legacy INTx (read its ISR
/// register in the handler to deassert the line).
pub const MYOS_IRQ_INTX: u16 = 0xFFFF;
/// `pci_irq_enable` out value: the device raises its one MSI message
/// (x86_64, a function without MSI-X); nothing to deassert.
pub const MYOS_IRQ_MSI: u16 = 0xFFFE;

/// How full a filesystem is ([`ModuleVfsOps::statfs`], `statvfs(3)`):
/// sizes in blocks of `bsize` bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VfsStatFs {
    pub bsize: u64,
    pub blocks: u64,
    pub bfree: u64,
    /// Free blocks an ordinary user may take (`bfree` less what is
    /// reserved).
    pub bavail: u64,
    /// File nodes (inodes), in all and free; 0 for a filesystem without
    /// them.
    pub files: u64,
    pub ffree: u64,
    /// The longest name a directory entry takes.
    pub namemax: u64,
}

/// Stat blob exchanged with module VFS hooks (matches kernel layout).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VfsStatInfo {
    pub mode: u32,
    pub size: u64,
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
    /// Optional write at `pos`. Bytes written (>=0), or negative:
    /// [`VFS_ENOSPC`] when the filesystem has no room for them.
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
    /// on fork and never frees it. `*attrs` (0 on entry) takes
    /// [`MYOS_MMAP_UNCACHED`] for a page of registers (a PCI BAR); the
    /// others are mapped as memory.
    pub mmap: Option<unsafe extern "C" fn(path: *const u8, path_len: usize, offset: usize, attrs: *mut u32) -> u64>,
    // --- ABI 17: readiness ---
    /// Optional: the `poll(2)` bits ([`MYOS_POLLIN`], [`MYOS_POLLOUT`],
    /// [`MYOS_POLLERR`], [`MYOS_POLLHUP`]) that hold now for `path`, whatever
    /// the caller asked for; without it a file is always readable and
    /// writable. Must not block. When readiness changes other than through a
    /// write or the last close of one of its files (both wake pollers), the
    /// backend calls `KernelApi::wake_any`. Into `changes` goes the file's
    /// change count (ABI 44): a number that moves, by any amount, whenever
    /// the file's readiness may have changed (bytes in or out, its peer's
    /// state, a hangup), read before the bits or under the same lock. An
    /// edge-triggered wait set member on the file (`EPOLLET`,
    /// `docs/events.md`) is reported again only once it moved, so a backend
    /// that left it still would starve such a reader.
    pub poll: Option<unsafe extern "C" fn(path: *const u8, path_len: usize, changes: *mut u64) -> u32>,
    // --- ABI 20: exclusive files ---
    /// Optional: an `open(2)` of `path` (not a `dup` or a `fork`): 0 lets it
    /// through, negative refuses it (a file one program holds at a time).
    /// `release` follows when the last fd of the file closes. Positive
    /// (ABI 50): the fd is on another file, named by that many bytes the
    /// hook wrote to `out` (at most `out_cap`), relative to `path`'s
    /// directory: a `clone` that made a new conversation names its
    /// `N/data`. That file's own `open` follows; a second redirect fails
    /// the open.
    pub open: Option<unsafe extern "C" fn(path: *const u8, path_len: usize, out: *mut u8, out_cap: usize) -> i32>,
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
    // --- ABI 35: filesystem space ---
    /// Optional: how full the filesystem is (`statvfs`): 0, or negative.
    /// Without it the mount reports no blocks at all.
    pub statfs: Option<unsafe extern "C" fn(out: *mut VfsStatFs) -> i32>,
}

/// [`ModuleVfsOps::read`]: nothing to read yet. A read through an fd waits
/// until `poll` reports [`MYOS_POLLIN`] (or a signal), then reads again.
pub const MYOS_READ_WAIT: i32 = -11;

/// `poll(2)` bits (Linux values), for [`ModuleVfsOps::poll`].
pub const MYOS_POLLIN: u32 = 0x1;
pub const MYOS_POLLOUT: u32 = 0x4;
pub const MYOS_POLLERR: u32 = 0x8;
pub const MYOS_POLLHUP: u32 = 0x10;
/// [`ModuleVfsOps::poll`]: readiness changes without a wake (a polled
/// device, like a keyboard without an interrupt): pollers re-check the file
/// every 10 ms.
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

/// Bytes of a disk's description ([`DiskInfo`]); the kernel cuts a longer one.
pub const DISK_INFO_MAX: usize = 160;

/// What a driver tells about a disk it registers ([`KernelApi::blk_register`]),
/// shown after its name and size in `/dev/<disk>/info`: the driver, where the
/// disk is (`pci 00:04.0`, `usb 0x002`, ...) and what the device says it
/// is (`model "..."`, `serial "..."`). Built with `write!` and [`Self::quoted`];
/// what does not fit is cut.
pub struct DiskInfo {
    buf: [u8; DISK_INFO_MAX],
    len: usize,
}

impl DiskInfo {
    pub const fn new() -> Self {
        DiskInfo { buf: [0; DISK_INFO_MAX], len: 0 }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    /// ` key "text"` for an identity string as devices report them (ASCII,
    /// padded with spaces or NULs: SCSI INQUIRY, NVMe identify, a virtio-blk
    /// id): trimmed, a `"`, `\` or anything not printable as `?`. Nothing
    /// when the text is blank.
    pub fn quoted(&mut self, key: &str, text: &[u8]) {
        let blank = |b: &u8| *b == b' ' || *b == 0;
        let Some(start) = text.iter().position(|b| !blank(b)) else {
            return;
        };
        let end = text.iter().rposition(|b| !blank(b)).map_or(start, |e| e + 1);
        self.push(b" ");
        self.push(key.as_bytes());
        self.push(b" \"");
        for &b in &text[start..end] {
            self.push(&[if (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\' { b } else { b'?' }]);
        }
        self.push(b"\"");
    }

    fn push(&mut self, bytes: &[u8]) {
        let n = bytes.len().min(DISK_INFO_MAX - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&bytes[..n]);
        self.len += n;
    }
}

impl Default for DiskInfo {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Write for DiskInfo {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.push(s.as_bytes());
        Ok(())
    }
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

/// Text screens: 0 is the console's, 1–9 the ttys' (`docs/tty.md`).
pub const CONSOLE_SCREENS: usize = 10;
/// `ModuleConsoleOps::keyboard_poll` value for Alt+digit: this plus the
/// screen the digit names (Alt+0 the console's).
pub const CONSOLE_KEY_SCREEN: i32 = 0x100;

/// The module-provided console (`KernelApi::console_register`): the
/// framebuffer's text screens and the local keyboards. Serial stays in the
/// kernel. `blink` and `status_line_try` run from the timer interrupt and
/// must never spin on a lock.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ModuleConsoleOps {
    /// Write `len` bytes of text (ANSI escapes understood) of `kind`
    /// ([`CONSOLE_TEXT`], [`CONSOLE_BANNER`], [`CONSOLE_INFO`]) to text
    /// screen `screen` (below [`CONSOLE_SCREENS`]); painted when it is the
    /// one shown.
    pub write_kind: unsafe extern "C" fn(screen: u32, buf: *const u8, len: usize, kind: u32),
    /// Write one `[ TAG ] label` line, the tag coloured by `kind`, to the
    /// console's screen (0).
    pub status_line: unsafe extern "C" fn(
        tag: *const u8,
        tag_len: usize,
        kind: u32,
        label: *const u8,
        label_len: usize,
    ),
    /// [`ModuleConsoleOps::status_line`] for an interrupt (the kernel's
    /// stuck-CPU report): 1 once written, 0 when the screen is held, with
    /// nothing written. Must not block.
    pub status_line_try: unsafe extern "C" fn(
        tag: *const u8,
        tag_len: usize,
        kind: u32,
        label: *const u8,
        label_len: usize,
    ) -> i32,
    /// Show text screen `screen`. 0 ok, negative when there is no screen.
    pub show: unsafe extern "C" fn(screen: u32) -> i32,
    /// Character-cell size (rows, cols); negative when there is no screen.
    pub winsize: unsafe extern "C" fn(rows: *mut u16, cols: *mut u16) -> i32,
    /// Toggle the block cursor (timer tick). Must not block.
    pub blink: unsafe extern "C" fn(),
    /// A local keyboard was found.
    pub keyboard_present: unsafe extern "C" fn() -> i32,
    /// Next keyboard byte (keymap-translated), [`CONSOLE_KEY_SCREEN`] plus a
    /// screen for Alt+digit, or negative when none is pending.
    pub keyboard_poll: unsafe extern "C" fn() -> i32,
    /// Install a keymap from its text form (the console ctl's `keymap PATH`,
    /// `docs/keymap.md`). 0 ok, negative on error.
    pub keymap_load: unsafe extern "C" fn(text: *const u8, len: usize) -> i32,
    /// 1 when a keymap is loaded.
    pub keymap_loaded: unsafe extern "C" fn() -> i32,
    /// 1 when the keyboard interrupts on input ([`KernelApi::console_input`]):
    /// its readers wait for that instead of re-polling it.
    pub keyboard_irq: unsafe extern "C" fn() -> i32,
    /// 1 while the screen shows the blinking text cursor (a screen, not
    /// taken over by `/dev/fb`): [`ModuleConsoleOps::blink`] is worth a
    /// timer wakeup.
    pub cursor_blinks: unsafe extern "C" fn() -> i32,
}

// ---- ABI 13: personalities (the Linux layer is a module) --------------------

/// Task slots (pids / tids are slot indexes), the user page size and the
/// native exec limits, for modules that keep per-task state or build execs.
pub const MYOS_MAX_TASKS: usize = 512;
/// Fds per process (`toolchain/newlib/libgloss/myos/myos_syscalls.h`'s
/// `MYOS_MAX_FDS` says the same).
pub const MYOS_MAX_FDS: usize = 256;
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
/// `setpgid` of a child that has exec'd.
pub const MYOS_SYSERR_EACCES: usize = usize::MAX - 9;
/// A write the filesystem has no room for.
pub const MYOS_SYSERR_ENOSPC: usize = usize::MAX - 10;
pub const MYOS_SYSERR_LOWEST: usize = MYOS_SYSERR_ENOSPC;

/// A filesystem's `write` / `write_ino` hook's and [`KernelApi::vfs_write`]'s
/// result when the filesystem has no room for the bytes (the caller's
/// `ENOSPC`); any other negative result is the generic failure.
pub const VFS_ENOSPC: i32 = -2;

/// Native signal dispositions (`signal_get_action`): default, ignore; any
/// other value is a caught handler's address. Native signal numbers are
/// newlib / BSD (`SIGKILL` 9, `SIGSEGV` 11).
pub const MYOS_HANDLER_DFL: usize = 0;
pub const MYOS_HANDLER_IGN: usize = 1;
pub const MYOS_SIGSEGV: u32 = 11;
/// `sa_flags` bit: on `SIGCHLD`, children leave no zombie (kernel `signal::SA_NOCLDWAIT`).
pub const MYOS_SA_NOCLDWAIT: u32 = 0x20;

/// Size of the FP/SIMD register image `fpu_save` writes (x86_64 XSAVE of
/// x87, SSE and AVX, of which a CPU without AVX fills the FXSAVE region,
/// the first 512 bytes; aarch64 `fpsimd_context` head + v0-v31, riscv64
/// f0-f31 + fcsr). The buffer must be 64-byte aligned.
#[cfg(target_arch = "x86_64")]
pub const MYOS_FP_BYTES: usize = 832;
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
/// handler's return restores, and the interrupted PC and syscall result
/// (for a fault: the faulting instruction, the result register's value).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SignalDelivery {
    pub sig: u32,
    pub flags: u32,
    /// The siginfo's `si_code` for a fault (POSIX's, at Linux's values:
    /// `SEGV_MAPERR`, `ILL_ILLOPC`, ..., or `SI_KERNEL`); 0 for a signal
    /// sent (`kill`, `SI_USER`).
    pub code: u32,
    /// Its `si_addr`: the faulting address or instruction; 0 for a signal
    /// sent.
    pub addr: usize,
    pub handler: usize,
    pub tramp: usize,
    pub restore_mask: u32,
    pub pc: usize,
    pub ret: usize,
    /// Arch word for the frame: x86_64 the user code segment selector (CS
    /// at handler entry); 0 elsewhere.
    pub arch: u64,
}

/// A foreign syscall personality (`KernelApi::personality_register`; one per
/// boot). A task acquires it through `SYS_LINUX_NEXT_EXEC` + exec or
/// [`KernelApi::personality_exec`]; the kernel tracks which tasks have it and
/// calls these hooks for them. `on_fork` / `on_thread` run under the task
/// lock with interrupts off.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PersonalityOps {
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
    /// of `data` now, and into `changes` its change count (ABI 44), as
    /// [`ModuleVfsOps::poll`]'s: moved by every frame or request in or out.
    /// `None`: always readable and writable. The module wakes the pollers
    /// itself (`KernelApi::wake_any`) when readiness changes other than
    /// through its own `read`/`write`: an interrupt, a request queued from
    /// another module.
    pub poll: Option<unsafe extern "C" fn(changes: *mut u64) -> u32>,
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
    /// The DMA handle of a device (`dev`): >= 0, or negative when the
    /// kernel's table of them is full. The same device gets the same handle.
    pub dma_attach: unsafe extern "C" fn(dev: *const DmaDevice) -> i32,
    /// `n_pages` zeroed, contiguous pages device `dma` can reach (its
    /// `dma-ranges`), never returned: their kernel address, and in `bus`
    /// the address the device uses for them; null when none is left.
    pub dma_alloc: unsafe extern "C" fn(dma: i32, n_pages: usize, bus: *mut u64) -> *mut u8,
    /// Hand `len` bytes at `va` of DMA memory over ([`DMA_TO_DEVICE`], also
    /// before the device writes them) or back ([`DMA_FROM_DEVICE`], before
    /// the CPU reads what it wrote): cache maintenance for a device without
    /// coherent DMA, a barrier for one with.
    pub dma_sync: unsafe extern "C" fn(dma: i32, va: *const u8, len: usize, dir: u32),
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
    /// use (virtio: `queue_msix_vector`), [`MYOS_IRQ_MSI`] for plain MSI or
    /// [`MYOS_IRQ_INTX`] for a legacy INTx line. `name` labels `/proc/interrupts`. 0 ok, negative on error.
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
    /// filesystem modules then reach it through `ops`. A name ending in
    /// `?` (`sd?`) gets the lowest free letter of that family from the
    /// kernel, whichever module registered the others; `blk_name` tells
    /// which. `info` describes the disk in `/dev/<disk>/info` ([`DiskInfo`]).
    pub blk_register: unsafe extern "C" fn(
        name: *const u8,
        name_len: usize,
        ops: *const ModuleBlkOps,
        ctx: usize,
        info: *const u8,
        info_len: usize,
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
    /// The signal (native number; 0: none) the current process gets when
    /// its parent ends, and setting it (Linux's `PR_SET_PDEATHSIG`): kept
    /// across exec, not inherited by a forked child.
    pub task_pdeath_signal: unsafe extern "C" fn() -> u32,
    pub task_set_pdeath_signal: unsafe extern "C" fn(sig: u32),
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
    /// Save / load the user FP/SIMD registers (`MYOS_FP_BYTES`, 64-byte aligned).
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
    /// written, or negative ([`VFS_ENOSPC`]: the filesystem is full).
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
    /// The uid of the user the current process runs for (docs/security.md),
    /// `u32::MAX` in a kernel task: what a module's objects belong to.
    pub current_uid: unsafe extern "C" fn() -> u32,
    // --- ABI 33 ---
    /// Route a board device's interrupt to `handler(ctx)` on CPU 0 and
    /// unmask it: on x86_64 a legacy ISA IRQ (0..15, through the I/O APIC),
    /// elsewhere the number [`MmioDevice::irq`] gives (a GIC SPI, a PLIC
    /// source). `name` labels `/proc/interrupts`. 0, or negative when it
    /// cannot be routed (no I/O APIC): the driver polls its device.
    pub irq_enable: unsafe extern "C" fn(irq: u32, name: StrRef, handler: IrqHandler, ctx: *mut core::ffi::c_void) -> i32,
    /// Keyboard input may be waiting (the keyboard's interrupt): wake the
    /// console's readers and pollers, which take it through
    /// [`ModuleConsoleOps::keyboard_poll`]. Safe from interrupt context.
    pub console_input: unsafe extern "C" fn(),
    // --- ABI 35 ---
    /// How full the filesystem holding `path` (a real path, as
    /// `vfs_stat`'s) is: 0, or negative (no such file).
    pub vfs_statfs: unsafe extern "C" fn(path: StrRef, out: *mut VfsStatFs) -> i32,
    // --- ABI 36 ---
    /// The name of the block device `dev` (`blk_register`'s id), into
    /// `out`: its length, or negative for no such disk. What a name
    /// ending in `?` became (`sd?` -> `sdb`).
    pub blk_name: unsafe extern "C" fn(dev: u32, out: *mut u8, cap: usize) -> i32,
    // --- ABI 37 ---
    /// The ready members of the wait set `fd` is open on (`/dev/wait`,
    /// docs/events.md), taken as a read takes them: up to `max` records of
    /// 16 bytes, `{u64 cookie, u32 events, u32 0}`, into `out`; how many,
    /// 0 when none is ready now, negative when `fd` is no wait set.
    pub wait_take: unsafe extern "C" fn(fd: usize, out: *mut u8, max: usize) -> i32,
    // --- ABI 38 ---
    /// 1 when every page of `[addr, addr + len)` is the calling process's
    /// (its image, stack, heap below the break or an mmap region), else 0.
    pub user_mapped: unsafe extern "C" fn(addr: usize, len: usize) -> i32,
    // --- ABI 40 ---
    /// Hold what the calling process's `fd` refers to, as a `dup` would,
    /// for another process to receive: a token (> 0), or negative for a
    /// free fd. A module that never hands it on lets go with `fd_release`.
    pub fd_hold: unsafe extern "C" fn(fd: usize) -> isize,
    /// Make held `token` the calling process's lowest free fd (closing at
    /// exec when `cloexec` is nonzero): the fd, or negative (no such token,
    /// or no free fd: still held then).
    pub fd_install: unsafe extern "C" fn(token: usize, cloexec: i32) -> i32,
    /// Let go of held `token`, as a close of the fd would.
    pub fd_release: unsafe extern "C" fn(token: usize),
    // --- ABI 41 ---
    /// Nanoseconds of CPU time of [`MYOS_CPU_PROCESS`] (the calling
    /// process's threads, ended ones included), [`MYOS_CPU_THREAD`] (the
    /// calling thread) or [`MYOS_CPU_CHILDREN`] (its reaped children).
    pub cpu_time: unsafe extern "C" fn(which: u32) -> u64,
    /// A memory barrier on every CPU that runs a task now, done before it
    /// returns (the CPUs are interrupted and answer): `membarrier`.
    pub membarrier: unsafe extern "C" fn(),
    // --- ABI 46 ---
    /// Claim the PCI function for `owner` (up to [`PCI_OWNER_MAX`] bytes):
    /// 0 when it is now `owner`'s (it was free or already so), -1 when
    /// another holds it. A module's `pci_enable`, `pci_bar_map` and
    /// `pci_irq_enable` from its `module_init` or `module_rescan` claim the
    /// function for it; a claim made through here lasts until
    /// [`KernelApi::pci_release`].
    pub pci_claim: unsafe extern "C" fn(bus: u8, slot: u8, func: u8, owner: *const u8, owner_len: usize) -> i32,
    /// Give back `owner`'s claim of the function (another's stays).
    pub pci_release: unsafe extern "C" fn(bus: u8, slot: u8, func: u8, owner: *const u8, owner_len: usize),
    /// The holder of the function's claim into `out`: its length, 0 when
    /// the function is free.
    pub pci_owner: unsafe extern "C" fn(bus: u8, slot: u8, func: u8, out: *mut u8, cap: usize) -> usize,
    // --- ABI 49 ---
    /// Confine the PCI function's DMA to a domain of its own, empty until
    /// `iommu_map`: the domain, or -1 when no IOMMU translates for the
    /// function (its DMA reaches all of memory, as a kernel driver's).
    pub iommu_attach: unsafe extern "C" fn(bus: u8, slot: u8, func: u8) -> i32,
    /// Let the domain's devices reach `[phys, phys + len)` (whole pages),
    /// at that address: 0, or -1 (no such domain, no memory).
    pub iommu_map: unsafe extern "C" fn(domain: i32, phys: u64, len: usize) -> i32,
    /// The function back in passthrough, its domain and mappings gone.
    pub iommu_detach: unsafe extern "C" fn(bus: u8, slot: u8, func: u8),
    // --- ABI 51: interrupt domains (docs/interrupts.md) ---
    /// Make the module's secondary interrupt controller, `count` lines
    /// behind one interrupt of its own, an interrupt domain: the node
    /// `phandle` ([`MmioDevice::phandle`], 0 for none; a specifier whose
    /// `interrupt-parent` it is decodes to a line, its first cell), each
    /// line `n` delivered by `unmask(ctx, n)` when [`KernelApi::irq_enable`]
    /// enables its interrupt number, and stopped by `mask(ctx, n)` when
    /// [`KernelApi::irq_disable`] takes its last handler away. The first of
    /// those numbers (line `n` is `first + n`), 0 on failure. Counted as a
    /// registration.
    pub irq_domain_register: unsafe extern "C" fn(
        phandle: u32,
        count: u32,
        unmask: IrqLineFn,
        mask: IrqLineFn,
        ctx: *mut core::ffi::c_void,
    ) -> u32,
    /// Run the handlers of interrupt `irq`, a line of the caller's domain:
    /// from the handler of the controller's own interrupt, for each line
    /// pending.
    pub irq_dispatch: unsafe extern "C" fn(irq: u32),
    // --- ABI 52: /dev/pci where the PCI window moves addresses or DMA is not coherent (issue #513) ---
    /// Memory BAR `bar` of the PCI function as the CPUs see it (the bus
    /// address, moved by the host bridge's window; assigned when the
    /// firmware left it unassigned) and its size: 0, or -1 for no such
    /// memory BAR.
    pub pci_bar_phys: unsafe extern "C" fn(bus: u8, slot: u8, func: u8, bar: u8, cpu: *mut u64, size: *mut u64) -> i32,
    /// Whether DMA handle `dma`'s device sees the caches (1), or needs
    /// [`KernelApi::dma_sync`] and non-cacheable mappings
    /// ([`MYOS_MMAP_NOCACHE`]) instead (0); -1 for no such handle.
    pub dma_coherent: unsafe extern "C" fn(dma: i32) -> i32,
    // --- ABI 54 ---
    /// A PCI interrupt route the firmware gives (ACPI `_PRT`, read by the
    /// acpi module): pin `pin` (0 INTA to 3 INTD) of device `device` on
    /// bus `bus` (a root bridge's, or a PCI-to-PCI bridge's with a `_PRT`
    /// of its own) raises GSI `gsi`, [`PCI_INTX_LEVEL`] and
    /// [`PCI_INTX_ACTIVE_LOW`] in `flags` saying how. A function with
    /// neither MSI-X nor MSI then gets its INTx from `pci_irq_enable`
    /// ([`MYOS_IRQ_INTX`]); one behind bridges without routes by its pin
    /// swizzled up to a bus with them. 0, or -1 when the table is full or the arch routes INTx
    /// from the device tree instead (aarch64, riscv64).
    pub pci_intx_route: unsafe extern "C" fn(bus: u8, device: u8, pin: u8, gsi: u32, flags: u32) -> i32,
    /// The kernel virtual address of `[phys, phys + len)`: where the kernel
    /// maps it already (RAM, the ACPI tables), else a new uncached device
    /// mapping. 0 when it cannot (x86_64 only: 0 elsewhere).
    pub phys_map: unsafe extern "C" fn(phys: u64, len: usize) -> usize,
    // --- ABI 55 ---
    /// Give back the interrupt [`KernelApi::pci_irq_enable`] routed to
    /// `handler(ctx)` for the function: the handler runs no more once this
    /// returns (on any CPU), the function's interrupt is off, and once its
    /// last handler is gone what it took is free for the next device (the
    /// vector and the IOMMU's remapping entry on x86_64). 0, or -1 when no
    /// such handler is registered. Task context.
    pub pci_irq_disable: unsafe extern "C" fn(bus: u8, slot: u8, func: u8, handler: IrqHandler, ctx: *mut core::ffi::c_void) -> i32,
    // --- ABI 56 ---
    /// Undo [`KernelApi::irq_enable`] of the same interrupt, handler and
    /// context (task context, not from the handler): the handler runs no
    /// more once this returns, and the line is masked when no handler is
    /// left on it (a domain's line through its `mask`). The module's
    /// registration count goes down when the enable counted. 0, or -1 when
    /// `irq` has no such handler.
    pub irq_disable: unsafe extern "C" fn(irq: u32, handler: IrqHandler, ctx: *mut core::ffi::c_void) -> i32,
    // --- ABI 59 ---
    /// I/O BAR `bar` of the PCI function, its I/O decoding on (assigned
    /// from the host bridge's I/O window when the firmware left it
    /// unassigned): where its registers are into `*base` (x86_64: the
    /// first port; elsewhere: the kernel address the bridge's I/O window
    /// shows them at) and its size in bytes into `*size`. Reach them
    /// through a [`PciIo`]. 0, or -1 for no such I/O BAR.
    pub pci_io_map: unsafe extern "C" fn(bus: u8, slot: u8, func: u8, bar: u8, base: *mut usize, size: *mut u32) -> i32,
    // --- ABI 62 ---
    /// Attach or clear a reader for an existing `/proc/<name>` node: each
    /// read(2) of it then calls `reader(buf, cap)`, which writes the node's
    /// text now into `buf` (cut at `cap`) and returns its full length, or
    /// negative on error, in place of the registered data (a value that
    /// changes, read from the hardware when asked: a battery's charge).
    /// Called in the reading task, without kernel locks held. 0 ok,
    /// negative when there is no such node.
    pub proc_set_reader: unsafe extern "C" fn(
        name: *const u8,
        name_len: usize,
        reader: Option<unsafe extern "C" fn(*mut u8, usize) -> i32>,
    ) -> i32,
}

/// A PCI function's I/O registers ([`KernelApi::pci_io_map`]): ports on
/// x86_64 (`in` / `out`), memory elsewhere, where the host bridge maps PCI
/// I/O space into its window. `off` is the register's offset in the BAR.
#[derive(Clone, Copy, Debug)]
pub struct PciIo {
    base: usize,
    size: u32,
}

impl PciIo {
    pub fn read8(&self, off: u32) -> u8 {
        debug_assert!(off < self.size);
        #[cfg(target_arch = "x86_64")]
        unsafe {
            let v: u8;
            core::arch::asm!("in al, dx", in("dx") (self.base as u32 + off) as u16, out("al") v, options(nomem, nostack, preserves_flags));
            v
        }
        #[cfg(not(target_arch = "x86_64"))]
        unsafe {
            core::ptr::read_volatile((self.base + off as usize) as *const u8)
        }
    }

    pub fn read16(&self, off: u32) -> u16 {
        debug_assert!(off + 1 < self.size);
        #[cfg(target_arch = "x86_64")]
        unsafe {
            let v: u16;
            core::arch::asm!("in ax, dx", in("dx") (self.base as u32 + off) as u16, out("ax") v, options(nomem, nostack, preserves_flags));
            v
        }
        #[cfg(not(target_arch = "x86_64"))]
        unsafe {
            u16::from_le(core::ptr::read_volatile((self.base + off as usize) as *const u16))
        }
    }

    pub fn read32(&self, off: u32) -> u32 {
        debug_assert!(off + 3 < self.size);
        #[cfg(target_arch = "x86_64")]
        unsafe {
            let v: u32;
            core::arch::asm!("in eax, dx", in("dx") (self.base as u32 + off) as u16, out("eax") v, options(nomem, nostack, preserves_flags));
            v
        }
        #[cfg(not(target_arch = "x86_64"))]
        unsafe {
            u32::from_le(core::ptr::read_volatile((self.base + off as usize) as *const u32))
        }
    }

    pub fn write8(&self, off: u32, v: u8) {
        debug_assert!(off < self.size);
        #[cfg(target_arch = "x86_64")]
        unsafe {
            core::arch::asm!("out dx, al", in("dx") (self.base as u32 + off) as u16, in("al") v, options(nomem, nostack, preserves_flags));
        }
        #[cfg(not(target_arch = "x86_64"))]
        unsafe {
            core::ptr::write_volatile((self.base + off as usize) as *mut u8, v)
        }
    }

    pub fn write16(&self, off: u32, v: u16) {
        debug_assert!(off + 1 < self.size);
        #[cfg(target_arch = "x86_64")]
        unsafe {
            core::arch::asm!("out dx, ax", in("dx") (self.base as u32 + off) as u16, in("ax") v, options(nomem, nostack, preserves_flags));
        }
        #[cfg(not(target_arch = "x86_64"))]
        unsafe {
            core::ptr::write_volatile((self.base + off as usize) as *mut u16, v.to_le())
        }
    }

    pub fn write32(&self, off: u32, v: u32) {
        debug_assert!(off + 3 < self.size);
        #[cfg(target_arch = "x86_64")]
        unsafe {
            core::arch::asm!("out dx, eax", in("dx") (self.base as u32 + off) as u16, in("eax") v, options(nomem, nostack, preserves_flags));
        }
        #[cfg(not(target_arch = "x86_64"))]
        unsafe {
            core::ptr::write_volatile((self.base + off as usize) as *mut u32, v.to_le())
        }
    }

    /// The BAR's size in bytes.
    pub fn size(&self) -> u32 {
        self.size
    }
}

/// An interrupt domain's `unmask` or `mask` of line `line`
/// ([`KernelApi::irq_domain_register`]).
pub type IrqLineFn = unsafe extern "C" fn(ctx: *mut core::ffi::c_void, line: u32);

/// [`KernelApi::pci_intx_route`]'s flags: a level-triggered line (else
/// edge), active low (else high). PCI's are both.
pub const PCI_INTX_LEVEL: u32 = 1 << 0;
pub const PCI_INTX_ACTIVE_LOW: u32 = 1 << 1;

/// The longest owner name of a PCI claim ([`KernelApi::pci_claim`]).
pub const PCI_OWNER_MAX: usize = 16;

/// [`KernelApi::cpu_time`]'s `which`.
pub const MYOS_CPU_PROCESS: u32 = 0;
pub const MYOS_CPU_THREAD: u32 = 1;
pub const MYOS_CPU_CHILDREN: u32 = 2;

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
/// `fpu_restore`, 64-byte aligned).
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

    pub fn dma_attach(&self, dev: &DmaDevice) -> i32 {
        unsafe { (self.dma_attach)(dev) }
    }

    pub fn dma_alloc(&self, dma: i32, n_pages: usize, bus: &mut u64) -> *mut u8 {
        unsafe { (self.dma_alloc)(dma, n_pages, bus) }
    }

    pub fn dma_sync(&self, dma: i32, va: *const u8, len: usize, dir: u32) {
        unsafe { (self.dma_sync)(dma, va, len, dir) }
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

    pub fn proc_set_reader(
        &self,
        name: &str,
        reader: Option<unsafe extern "C" fn(*mut u8, usize) -> i32>,
    ) -> i32 {
        unsafe { (self.proc_set_reader)(name.as_ptr(), name.len(), reader) }
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

    /// See [`KernelApi::pci_irq_disable`] (field): whether it was routed.
    pub fn pci_irq_disable(&self, bus: u8, slot: u8, func: u8, handler: IrqHandler, ctx: *mut core::ffi::c_void) -> bool {
        unsafe { (self.pci_irq_disable)(bus, slot, func, handler, ctx) == 0 }
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

    pub fn blk_register(&self, name: &str, ops: &ModuleBlkOps, ctx: usize, info: &DiskInfo) -> i32 {
        let info = info.as_bytes();
        unsafe { (self.blk_register)(name.as_ptr(), name.len(), ops, ctx, info.as_ptr(), info.len()) }
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

    pub fn current_uid(&self) -> u32 {
        unsafe { (self.current_uid)() }
    }

    pub fn task_is_live_user(&self, id: usize) -> bool {
        unsafe { (self.task_is_live_user)(id) != 0 }
    }

    pub fn task_has_root(&self) -> bool {
        unsafe { (self.task_has_root)() != 0 }
    }

    pub fn task_pdeath_signal(&self) -> u32 {
        unsafe { (self.task_pdeath_signal)() }
    }

    pub fn task_set_pdeath_signal(&self, sig: u32) {
        unsafe { (self.task_set_pdeath_signal)(sig) }
    }

    pub fn path_resolve(&self, path: &str, mode: u32, out: &mut [u8]) -> i32 {
        unsafe { (self.path_resolve)(StrRef::new(path), mode, out.as_mut_ptr(), out.len()) }
    }

    pub fn vfs_stat(&self, path: &str, out: &mut PathStat) -> i32 {
        unsafe { (self.vfs_stat)(StrRef::new(path), out) }
    }

    pub fn vfs_statfs(&self, path: &str, out: &mut VfsStatFs) -> i32 {
        unsafe { (self.vfs_statfs)(StrRef::new(path), out) }
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

    /// The disk's name into `out`: its length, or negative.
    pub fn blk_name(&self, dev: u32, out: &mut [u8]) -> i32 {
        unsafe { (self.blk_name)(dev, out.as_mut_ptr(), out.len()) }
    }

    /// [`KernelApi::wait_take`]: `out` holds `max` records of 16 bytes.
    pub fn wait_take(&self, fd: usize, out: &mut [u8], max: usize) -> i32 {
        debug_assert!(out.len() >= max * 16);
        unsafe { (self.wait_take)(fd, out.as_mut_ptr(), max.min(out.len() / 16)) }
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

    pub fn fd_hold(&self, fd: usize) -> Option<usize> {
        let t = unsafe { (self.fd_hold)(fd) };
        (t > 0).then_some(t as usize)
    }

    pub fn fd_install(&self, token: usize, cloexec: bool) -> Option<usize> {
        let fd = unsafe { (self.fd_install)(token, cloexec as i32) };
        (fd >= 0).then_some(fd as usize)
    }

    pub fn fd_release(&self, token: usize) {
        unsafe { (self.fd_release)(token) }
    }

    pub fn cpu_time(&self, which: u32) -> u64 {
        unsafe { (self.cpu_time)(which) }
    }

    pub fn membarrier(&self) {
        unsafe { (self.membarrier)() }
    }

    pub fn pci_claim(&self, bus: u8, slot: u8, func: u8, owner: &str) -> bool {
        unsafe { (self.pci_claim)(bus, slot, func, owner.as_ptr(), owner.len()) == 0 }
    }

    pub fn pci_release(&self, bus: u8, slot: u8, func: u8, owner: &str) {
        unsafe { (self.pci_release)(bus, slot, func, owner.as_ptr(), owner.len()) }
    }

    /// The holder of the function's claim, in `out`; empty when it is free.
    pub fn pci_owner<'a>(&self, bus: u8, slot: u8, func: u8, out: &'a mut [u8; PCI_OWNER_MAX]) -> &'a [u8] {
        let n = unsafe { (self.pci_owner)(bus, slot, func, out.as_mut_ptr(), out.len()) };
        &out[..n.min(PCI_OWNER_MAX)]
    }

    /// The function's own IOMMU domain, or `None` without an IOMMU for it.
    pub fn iommu_attach(&self, bus: u8, slot: u8, func: u8) -> Option<i32> {
        let d = unsafe { (self.iommu_attach)(bus, slot, func) };
        (d >= 0).then_some(d)
    }

    pub fn iommu_map(&self, domain: i32, phys: u64, len: usize) -> bool {
        unsafe { (self.iommu_map)(domain, phys, len) == 0 }
    }

    pub fn iommu_detach(&self, bus: u8, slot: u8, func: u8) {
        unsafe { (self.iommu_detach)(bus, slot, func) }
    }

    /// See [`KernelApi::irq_domain_register`] (field).
    ///
    /// # Safety
    ///
    /// `unmask` and `ctx` must stay valid for as long as the kernel runs
    /// (the registration keeps the module loaded).
    pub unsafe fn irq_domain_register(
        &self,
        phandle: u32,
        count: u32,
        unmask: IrqLineFn,
        mask: IrqLineFn,
        ctx: *mut core::ffi::c_void,
    ) -> u32 {
        unsafe { (self.irq_domain_register)(phandle, count, unmask, mask, ctx) }
    }

    /// See [`KernelApi::irq_disable`] (field).
    pub fn irq_disable(&self, irq: u32, handler: IrqHandler, ctx: *mut core::ffi::c_void) -> i32 {
        unsafe { (self.irq_disable)(irq, handler, ctx) }
    }

    /// I/O BAR `bar` of the PCI function, its I/O decoding on
    /// ([`KernelApi::pci_io_map`]).
    pub fn pci_io_map(&self, bus: u8, slot: u8, func: u8, bar: u8) -> Option<PciIo> {
        let (mut base, mut size) = (0usize, 0u32);
        (unsafe { (self.pci_io_map)(bus, slot, func, bar, &mut base, &mut size) } == 0).then_some(PciIo { base, size })
    }

    pub fn irq_dispatch(&self, irq: u32) {
        unsafe { (self.irq_dispatch)(irq) }
    }

    /// See [`KernelApi::pci_bar_phys`] (field).
    pub fn pci_bar_phys(&self, bus: u8, slot: u8, func: u8, bar: u8, cpu: &mut u64, size: &mut u64) -> i32 {
        unsafe { (self.pci_bar_phys)(bus, slot, func, bar, cpu, size) }
    }

    /// See [`KernelApi::pci_intx_route`] (field): whether the kernel took it.
    pub fn pci_intx_route(&self, bus: u8, device: u8, pin: u8, gsi: u32, flags: u32) -> bool {
        unsafe { (self.pci_intx_route)(bus, device, pin, gsi, flags) == 0 }
    }

    /// See [`KernelApi::phys_map`] (field).
    pub fn phys_map(&self, phys: u64, len: usize) -> Option<usize> {
        let va = unsafe { (self.phys_map)(phys, len) };
        (va != 0).then_some(va)
    }

    /// See [`KernelApi::dma_coherent`] (field): `None` for no such handle.
    pub fn dma_coherent(&self, dma: i32) -> Option<bool> {
        match unsafe { (self.dma_coherent)(dma) } {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }

    pub fn user_mapped(&self, addr: usize, len: usize) -> bool {
        unsafe { (self.user_mapped)(addr, len) != 0 }
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

    /// See [`KernelApi::irq_enable`] (field).
    pub fn irq_enable(&self, irq: u32, name: &str, handler: IrqHandler, ctx: *mut core::ffi::c_void) -> i32 {
        unsafe { (self.irq_enable)(irq, StrRef::new(name), handler, ctx) }
    }

    pub fn console_input(&self) {
        unsafe { (self.console_input)() }
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
// service [`USB_SERVICE`]; class driver modules (`usb_hub`, `usb_storage`,
// `usb_hid`) look it up and register a [`UsbDriverOps`]. The host
// enumerates devices on its own thread (`thread_spawn`) and offers every
// interface to the drivers there, so `probe` and `disconnect` run in task
// context and may block in the host's transfers.

/// The name the host controller publishes its [`UsbHostOps`] under.
pub const USB_SERVICE: &str = "usb";
/// [`UsbHostOps::version`]. 2 added [`UsbHostOps::interface_label`]; 3
/// made `interrupt_start` [`UsbHostOps::transfer_start`], bulk IN too; 4
/// added alternate settings ([`UsbHostOps::set_interface`]) and
/// isochronous OUT transfers ([`UsbHostOps::isoch_queue`]).
pub const USB_HOST_VERSION: u32 = 4;
/// Isochronous packets queued on an endpoint at most
/// ([`UsbHostOps::isoch_queue`]), and the bytes of one.
pub const USB_ISOCH_MAX: usize = 64;
pub const USB_ISOCH_PACKET_MAX: usize = 1024;
/// Bytes of a bulk IN transfer `transfer_start` queues at most (a page:
/// the endpoint's own bounce buffer).
pub const USB_ASYNC_BULK_MAX: usize = 4096;

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
/// [`UsbHostOps::isoch_queue`]: the endpoint has [`USB_ISOCH_MAX`] packets
/// in flight.
pub const USB_EBUSY: i32 = -16;

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

/// The completion of a transfer queued with `transfer_start`: `status` is
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
    /// Queue one IN transfer on an interrupt or bulk `endpoint` (a bulk one
    /// of [`USB_ASYNC_BULK_MAX`] bytes at most); `done(ctx, status)` runs
    /// on the host's thread when it completes. One outstanding transfer per
    /// endpoint; nothing else of the device's is held while it waits (a
    /// blocking `bulk` holds the device and the controller's bounce
    /// buffer). 0, or negative.
    pub transfer_start: unsafe extern "C" fn(
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
    /// Select alternate setting `alternate` of interface `interface` (an
    /// audio streaming interface's, whose endpoints come with its setting
    /// 1): the controller lets the endpoints of the setting before go and
    /// takes the new one's, isochronous ones too, then SET_INTERFACE. The
    /// interface starts at setting 0. 0, or negative.
    pub set_interface: unsafe extern "C" fn(dev: u32, interface: u8, alternate: u8) -> i32,
    /// `done(ctx, packets)` runs on the host's thread as packets queued on
    /// the isochronous OUT `endpoint` complete (sent, or past their
    /// interval: either way gone), `packets` of them, until `isoch_stop`;
    /// [`USB_EGONE`] once the device is gone. 0, or negative.
    pub isoch_start: unsafe extern "C" fn(dev: u32, endpoint: u8, done: UsbCompletion, ctx: *mut core::ffi::c_void) -> i32,
    /// Queue `n` packets on the isochronous OUT `endpoint`, one per
    /// service interval from the next free one: `lens[i]` bytes each (at
    /// most [`USB_ISOCH_PACKET_MAX`]), back to back at `data` (the host
    /// copies them). 0, or negative: [`USB_EBUSY`] past
    /// [`USB_ISOCH_MAX`] packets in flight.
    pub isoch_queue: unsafe extern "C" fn(dev: u32, endpoint: u8, data: *const u8, lens: *const u16, n: usize) -> i32,
    /// No more `done` calls for `endpoint` (what is in flight still plays).
    pub isoch_stop: unsafe extern "C" fn(dev: u32, endpoint: u8),
}

/// Safe calls into the host, as [`KernelApi`]'s methods: the host checks
/// what it is handed. `transfer_start` stays `unsafe` (its buffer is
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

    pub fn set_interface(&self, dev: u32, interface: u8, alternate: u8) -> i32 {
        unsafe { (self.set_interface)(dev, interface, alternate) }
    }

    /// `isoch_queue` of the packets `lens` says, back to back in `data`.
    pub fn isoch_queue(&self, dev: u32, endpoint: u8, data: &[u8], lens: &[u16]) -> i32 {
        if lens.iter().map(|&l| usize::from(l)).sum::<usize>() > data.len() {
            return USB_EIO;
        }
        unsafe { (self.isoch_queue)(dev, endpoint, data.as_ptr(), lens.as_ptr(), lens.len()) }
    }

    pub fn isoch_stop(&self, dev: u32, endpoint: u8) {
        unsafe { (self.isoch_stop)(dev, endpoint) }
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

// ---- The USB core and its host controller drivers (docs/usb.md) -----------
//
// The `usb` module owns the bus: the devices, their enumeration, the class
// drivers, the hubs' children, `/proc/usb` and the [`UsbHostOps`] service
// above. A host controller module (`xhci`, `dwc2`) registers each
// controller it drives with it ([`UsbCoreOps::controller_register`], the
// service [`USB_CORE_SERVICE`]) and does what only its hardware knows
// ([`UsbHcdOps`]): the root ports, the per-device state the controller
// keeps and the transfers. The core calls a controller's table from its
// own thread and from the class drivers' callers, one call per device at a
// time (it holds the device's lock across the call); the controller calls
// the core's from any context, its interrupt handler included.

/// The name the core publishes its [`UsbCoreOps`] under.
pub const USB_CORE_SERVICE: &str = "usbcore";
/// [`UsbCoreOps::version`].
pub const USB_CORE_VERSION: u32 = 1;
/// [`UsbHcdOps::version`]. 2 added alternate settings and isochronous OUT
/// transfers ([`UsbHcdOps::endpoints_change`], `isoch_queue`,
/// `isoch_reap`).
pub const USB_HCD_VERSION: u32 = 2;
/// [`UsbHcdOps::transfer_take`]: the transfer has not completed yet.
pub const USB_EPENDING: i32 = -115;

/// Where a new device hangs and how it is reached
/// ([`UsbHcdOps::device_new`]).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct UsbDevicePath {
    /// `USB_SPEED_*`.
    pub speed: u8,
    /// The root port (1-based) the device is under.
    pub root_port: u8,
    /// Hubs above it.
    pub depth: u8,
    /// Its port on its hub, or the root port for a device on one.
    pub port: u8,
    /// The address the core gives it (1 to 127; every device of a
    /// controller has its own).
    pub address: u8,
    /// A low- or full-speed device behind a high-speed hub: the port of
    /// that hub (its transaction translator's), 0 for no translator.
    pub tt_port: u8,
    /// The hubs' ports from the root, 4 bits per tier (xHCI's route
    /// string).
    pub route: u32,
    /// The controller's handle of its hub, 0 for a device on a root port.
    pub parent: u32,
    /// The controller's handle of the hub whose transaction translator it
    /// goes through, 0 for none.
    pub tt_hub: u32,
}

/// A root port's state ([`UsbHcdOps::port_status`]).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct UsbPortStatus {
    /// A device is attached.
    pub connected: u8,
    /// The port is enabled: the device answers at `speed`.
    pub enabled: u8,
    /// `USB_SPEED_*` of the attached device, once the port is enabled.
    pub speed: u8,
}

/// A host controller, for the core ([`UsbCoreOps::controller_register`]).
/// `ctrl` is the value the driver registered the controller with (its own
/// index); `dev` a handle [`UsbHcdOps::device_new`] returned. The calls that
/// wait (a reset, a transfer) run in task context; `device_abort` may be
/// called while another call on the device is in flight, the rest never
/// overlap on one device.
#[repr(C)]
pub struct UsbHcdOps {
    pub version: u32,
    /// The driver's name (`/proc/usb`).
    pub name: StrRef,
    /// Root ports of the controller.
    pub ports: unsafe extern "C" fn(ctrl: usize) -> u8,
    /// Root port `port` (0-based): its state into `*out`, its change bits
    /// acknowledged. 0 ok.
    pub port_status: unsafe extern "C" fn(ctrl: usize, port: u8, out: *mut UsbPortStatus) -> i32,
    /// Reset root port `port`, so its device answers at address 0 (a USB 3
    /// port whose link trained is left as it is): 0 once the port is
    /// enabled, negative otherwise.
    pub port_reset: unsafe extern "C" fn(ctrl: usize, port: u8) -> i32,
    /// Look at the controller's events now (the core's thread, every pass):
    /// nonzero when the controller has no interrupt and wants to be asked
    /// every few milliseconds.
    pub poll: unsafe extern "C" fn(ctrl: usize) -> i32,
    /// What `/proc/usb` says of the controller after its ports (the
    /// interrupt, event counts): into `buf`, at most `len` bytes; the
    /// length written.
    pub describe: unsafe extern "C" fn(ctrl: usize, buf: *mut u8, len: usize) -> usize,
    /// A device at `path`, its default control endpoint ready at the
    /// speed's default max packet size, nothing sent to it yet: the
    /// controller's handle for it (positive), or negative.
    pub device_new: unsafe extern "C" fn(ctrl: usize, path: *const UsbDevicePath) -> i32,
    /// Give the device its address (`path.address`), the controller's way
    /// (xHCI's Address Device, or a SET_ADDRESS the driver sends). 0 ok.
    pub device_address: unsafe extern "C" fn(ctrl: usize, dev: u32) -> i32,
    /// The device's control endpoint takes `max_packet` bytes a packet (its
    /// device descriptor said so). 0 ok.
    pub device_ep0: unsafe extern "C" fn(ctrl: usize, dev: u32, max_packet: u16) -> i32,
    /// The `n` endpoints of the configuration about to be set (isochronous
    /// ones are not among them). 0 ok.
    pub device_configure: unsafe extern "C" fn(ctrl: usize, dev: u32, eps: *const UsbEndpoint, n: usize) -> i32,
    /// The device is a hub ([`UsbHostOps::hub_configure`]'s arguments). 0 ok.
    pub device_hub: unsafe extern "C" fn(ctrl: usize, dev: u32, ports: u8, tt_think: u8, multi_tt: u8) -> i32,
    /// The device is going: every transfer in flight on it fails with
    /// [`USB_EGONE`] now, its waiters woken, and none starts from here on.
    /// Called with no lock held, while a transfer may be in flight.
    pub device_abort: unsafe extern "C" fn(ctrl: usize, dev: u32),
    /// Forget the device (after `device_abort`, nothing in flight): its
    /// controller state and memory.
    pub device_free: unsafe extern "C" fn(ctrl: usize, dev: u32),
    /// [`UsbHostOps::control`] on the device.
    pub control: unsafe extern "C" fn(
        ctrl: usize,
        dev: u32,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: *mut u8,
        len: u16,
    ) -> i32,
    /// [`UsbHostOps::bulk`] on the device.
    pub bulk: unsafe extern "C" fn(ctrl: usize, dev: u32, endpoint: u8, data: *mut u8, len: usize, timeout_ms: u32) -> i32,
    /// Queue one IN transfer on an interrupt or bulk `endpoint` (a bulk one
    /// of [`USB_ASYNC_BULK_MAX`] bytes at most) into `data` (one per
    /// endpoint at a time; the core keeps the buffer until `transfer_take`
    /// reports it done). When it completes, the driver calls
    /// [`UsbCoreOps::wake`]. 0, or negative.
    pub transfer_start: unsafe extern "C" fn(ctrl: usize, dev: u32, endpoint: u8, data: *mut u8, len: usize) -> i32,
    /// The queued transfer on `endpoint`: [`USB_EPENDING`] while it runs;
    /// else its result (the bytes, now in the buffer, or a `USB_E*` error)
    /// and the endpoint takes another.
    pub transfer_take: unsafe extern "C" fn(ctrl: usize, dev: u32, endpoint: u8) -> i32,
    /// The controller's side of recovering a stalled endpoint (the core
    /// sends the CLEAR_FEATURE). 0 ok.
    pub clear_halt: unsafe extern "C" fn(ctrl: usize, dev: u32, endpoint: u8) -> i32,
    /// An interface's alternate setting changed (`set_interface`): the
    /// endpoints `drop` names (addresses) go, the `n_add` at `add` come,
    /// set up as `device_configure` does, isochronous ones too. 0 ok.
    pub endpoints_change: unsafe extern "C" fn(
        ctrl: usize,
        dev: u32,
        add: *const UsbEndpoint,
        n_add: usize,
        drop: *const u8,
        n_drop: usize,
    ) -> i32,
    /// [`UsbHostOps::isoch_queue`] on the device: 0, negative, or
    /// [`USB_EIO`] where the controller does not drive isochronous
    /// transfers. A packet's completion calls [`UsbCoreOps::wake`].
    pub isoch_queue: unsafe extern "C" fn(ctrl: usize, dev: u32, endpoint: u8, data: *const u8, lens: *const u16, n: usize) -> i32,
    /// The packets of `endpoint` that completed since the last call, or
    /// negative.
    pub isoch_reap: unsafe extern "C" fn(ctrl: usize, dev: u32, endpoint: u8) -> i32,
}

/// The core, for the host controller drivers (the [`USB_CORE_SERVICE`]
/// table). `port_change` and `wake` are safe from an interrupt handler.
#[repr(C)]
pub struct UsbCoreOps {
    pub version: u32,
    /// A controller `ops` drives, known to it as `ctrl`: the core's index
    /// for it (what `port_change` and `wake` take), or negative. The core's
    /// thread scans its root ports from here on.
    pub controller_register: unsafe extern "C" fn(ops: *const UsbHcdOps, ctrl: usize) -> i32,
    /// Root port `port` (0-based) of the core's controller `index` changed.
    pub port_change: unsafe extern "C" fn(index: u32, port: u8),
    /// Something completed on the core's controller `index` (a queued
    /// interrupt transfer): its thread looks.
    pub wake: unsafe extern "C" fn(index: u32),
}

impl UsbCoreOps {
    pub fn controller_register(&self, ops: &'static UsbHcdOps, ctrl: usize) -> i32 {
        unsafe { (self.controller_register)(ops, ctrl) }
    }

    pub fn port_change(&self, index: u32, port: u8) {
        unsafe { (self.port_change)(index, port) }
    }

    pub fn wake(&self, index: u32) {
        unsafe { (self.wake)(index) }
    }
}

unsafe impl Sync for UsbHcdOps {}
unsafe impl Sync for UsbCoreOps {}

// ---- The Raspberry Pi's firmware (docs/raspberry-pi.md) ---------------------
//
// The `rpi_firmware` module talks to the VideoCore firmware through its
// mailbox property interface and publishes a [`RpiFirmwareOps`] table as
// the service [`RPI_FIRMWARE_SERVICE`]: a driver asks it to power a device
// domain on (the USB controller) or for a clock's rate.

/// The name the module publishes its [`RpiFirmwareOps`] under.
pub const RPI_FIRMWARE_SERVICE: &str = "rpi_firmware";
/// [`RpiFirmwareOps::version`].
pub const RPI_FIRMWARE_VERSION: u32 = 1;

/// Property tags ([`RpiFirmwareOps::property`]): the firmware's and board's
/// revisions, a device's power state (`RPI_POWER_*`: `state` bit 0 on, bit 1
/// wait for it), a clock's rate (`RPI_CLOCK_*`) in Hz.
pub const RPI_TAG_FIRMWARE_REVISION: u32 = 0x0000_0001;
pub const RPI_TAG_BOARD_REVISION: u32 = 0x0001_0002;
pub const RPI_TAG_GET_POWER_STATE: u32 = 0x0002_0001;
pub const RPI_TAG_SET_POWER_STATE: u32 = 0x0002_8001;
pub const RPI_TAG_GET_CLOCK_RATE: u32 = 0x0003_0002;
/// The firmware's GPIO expander lines (`RPI_GPIO_EXPANDER` and up): a
/// line's state, its state set, its configuration (direction, polarity,
/// termination, state) set.
pub const RPI_TAG_GET_GPIO_STATE: u32 = 0x0003_0041;
pub const RPI_TAG_SET_GPIO_STATE: u32 = 0x0003_8041;
pub const RPI_TAG_SET_GPIO_CONFIG: u32 = 0x0003_8043;
/// The power domains the firmware switches.
pub const RPI_POWER_SD_CARD: u32 = 0;
pub const RPI_POWER_UART0: u32 = 1;
pub const RPI_POWER_UART1: u32 = 2;
pub const RPI_POWER_USB_HCD: u32 = 3;
/// The GPIO expander the firmware drives (the Pi 3's and 4's, `expgpio`
/// in their trees): its lines are numbered from here. `WL_ON` powers the
/// Wi-Fi chip (`WL_REG_ON`), `BT_ON` the Bluetooth side.
pub const RPI_GPIO_EXPANDER: u32 = 128;
pub const RPI_GPIO_BT_ON: u32 = RPI_GPIO_EXPANDER;
pub const RPI_GPIO_WL_ON: u32 = RPI_GPIO_EXPANDER + 1;
/// The clocks it reports.
pub const RPI_CLOCK_EMMC: u32 = 1;
pub const RPI_CLOCK_UART: u32 = 2;
pub const RPI_CLOCK_ARM: u32 = 3;
pub const RPI_CLOCK_CORE: u32 = 4;
/// The BCM2711's EMMC2 controller's (the Pi 4's card slot).
pub const RPI_CLOCK_EMMC2: u32 = 12;

/// The firmware, for the drivers (the [`RPI_FIRMWARE_SERVICE`] table).
/// Task context only: a call waits for the firmware's answer.
#[repr(C)]
pub struct RpiFirmwareOps {
    pub version: u32,
    /// Run the property tags in `tags` (`words` of them: each a tag id, its
    /// value buffer's size in bytes, a request code of 0 and the buffer,
    /// as the mailbox property interface lays them out, without the
    /// message header and end tag) and write the firmware's answer over
    /// them (the response code of each, bit 31 set, with the length it
    /// filled, and the values). 0, or negative when the firmware refused
    /// the message, did not answer, or `tags` does not fit.
    pub property: unsafe extern "C" fn(tags: *mut u32, words: usize) -> i32,
}

impl RpiFirmwareOps {
    /// [`RpiFirmwareOps::property`] on `tags`.
    pub fn property(&self, tags: &mut [u32]) -> i32 {
        unsafe { (self.property)(tags.as_mut_ptr(), tags.len()) }
    }

    /// One tag with `values` of 4 bytes each (at most 9), in and out: the
    /// values the firmware answered, `None` when it did not or the tag
    /// failed.
    pub fn tag<const N: usize>(&self, tag: u32, values: [u32; N]) -> Option<[u32; N]> {
        let mut buf = [0u32; 12];
        if N > 9 {
            return None;
        }
        buf[0] = tag;
        buf[1] = (N * 4) as u32;
        buf[3..3 + N].copy_from_slice(&values);
        if self.property(&mut buf[..3 + N]) < 0 || buf[2] & 0x8000_0000 == 0 {
            return None;
        }
        let mut out = [0u32; N];
        out.copy_from_slice(&buf[3..3 + N]);
        Some(out)
    }

    /// Power domain `device` (`RPI_POWER_*`) on, waiting until it is:
    /// whether it is on now.
    pub fn power_on(&self, device: u32) -> bool {
        self.tag(RPI_TAG_SET_POWER_STATE, [device, 3]).is_some_and(|[_, state]| state & 1 != 0 && state & 2 == 0)
    }

    /// The rate of clock `clock` (`RPI_CLOCK_*`) in Hz, `None` when the
    /// firmware does not know it.
    pub fn clock_rate(&self, clock: u32) -> Option<u32> {
        self.tag(RPI_TAG_GET_CLOCK_RATE, [clock, 0]).map(|[_, rate]| rate).filter(|&r| r != 0)
    }

    /// Expander line `gpio` (`RPI_GPIO_*`) an output, driven `on` or off:
    /// whether the firmware took it.
    pub fn gpio_output(&self, gpio: u32, on: bool) -> bool {
        self.tag(RPI_TAG_SET_GPIO_CONFIG, [gpio, 1, 0, 0, 0, u32::from(on)]).is_some()
    }

    /// Expander line `gpio` driven `on` or off.
    pub fn set_gpio(&self, gpio: u32, on: bool) -> bool {
        self.tag(RPI_TAG_SET_GPIO_STATE, [gpio, u32::from(on)]).is_some()
    }

    /// Expander line `gpio`'s state, `None` when the firmware does not say.
    pub fn gpio(&self, gpio: u32) -> Option<bool> {
        self.tag(RPI_TAG_GET_GPIO_STATE, [gpio, 0]).map(|[_, state]| state & 1 != 0)
    }
}

unsafe impl Sync for RpiFirmwareOps {}

// ---- The pointer (docs/tty.md) ---------------------------------------------
//
// The console module owns `/dev/console/mouse` and publishes a
// [`PointerOps`] table as the service [`POINTER_SERVICE`]; a pointing
// device's driver (`usb_hid`) looks it up and hands it what the device
// reports. Every device feeds the same file. The calls may come from any
// context but an interrupt handler.

/// The name the console publishes its [`PointerOps`] under.
pub const POINTER_SERVICE: &str = "pointer";
/// [`PointerOps::version`].
pub const POINTER_VERSION: u32 = 1;
/// An absolute position runs from 0 to this on both axes, whatever the
/// device's own range ([`PointerOps::absolute`]).
pub const POINTER_ABS_MAX: i32 = 32767;
/// Button codes ([`PointerOps::button`]): Linux's `BTN_*`.
pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;
pub const BTN_SIDE: u16 = 0x113;
pub const BTN_EXTRA: u16 = 0x114;

/// The console's pointer queue, for pointing device drivers (the
/// [`POINTER_SERVICE`] table).
#[repr(C)]
pub struct PointerOps {
    pub version: u32,
    /// Relative motion: right and down are positive.
    pub motion: unsafe extern "C" fn(dx: i32, dy: i32),
    /// An absolute position, both axes scaled to `0..=`[`POINTER_ABS_MAX`]
    /// (a tablet; left/top is 0).
    pub absolute: unsafe extern "C" fn(x: i32, y: i32),
    /// The vertical wheel: positive turns away from the user (up).
    pub wheel: unsafe extern "C" fn(dz: i32),
    /// A button (`BTN_*`) was pressed or released.
    pub button: unsafe extern "C" fn(code: u16, pressed: bool),
}

unsafe impl Sync for PointerOps {}

impl PointerOps {
    pub fn motion(&self, dx: i32, dy: i32) {
        unsafe { (self.motion)(dx, dy) }
    }

    pub fn absolute(&self, x: i32, y: i32) {
        unsafe { (self.absolute)(x, y) }
    }

    pub fn wheel(&self, dz: i32) {
        unsafe { (self.wheel)(dz) }
    }

    pub fn button(&self, code: u16, pressed: bool) {
        unsafe { (self.button)(code, pressed) }
    }
}

// ---- The keyboards of other modules (docs/tty.md) --------------------------
//
// The console module publishes a [`KeyboardOps`] table as the service
// [`KEYBOARD_SERVICE`]; a keyboard's driver in another module (`usb_hid`)
// hands it the presses and releases, which go where the console's own
// keyboard's go: the tty of the screen shown, the screen switch, or
// `/dev/console/kbd`. The keys do not repeat by themselves: the console
// repeats a held one. The calls may come from any context but an interrupt
// handler.

/// The name the console publishes its [`KeyboardOps`] under.
pub const KEYBOARD_SERVICE: &str = "keyboard";
/// [`KeyboardOps::version`].
pub const KEYBOARD_VERSION: u32 = 1;
/// The lock LEDs ([`KeyboardOps::leds`]), as a HID keyboard's output report
/// has them.
pub const LED_NUM_LOCK: u8 = 1 << 0;
pub const LED_CAPS_LOCK: u8 = 1 << 1;
pub const LED_SCROLL_LOCK: u8 = 1 << 2;

/// The console's keyboard input, for keyboard drivers (the
/// [`KEYBOARD_SERVICE`] table).
#[repr(C)]
pub struct KeyboardOps {
    pub version: u32,
    /// A keyboard is there (before its first key).
    pub attach: unsafe extern "C" fn(),
    /// A keyboard is gone; its driver released its held keys first.
    pub detach: unsafe extern "C" fn(),
    /// A key (Linux `KEY_*` code) was pressed or released.
    pub key: unsafe extern "C" fn(code: u16, pressed: bool),
    /// The lock LEDs to light now (`LED_*`).
    pub leds: unsafe extern "C" fn() -> u8,
}

unsafe impl Sync for KeyboardOps {}

impl KeyboardOps {
    pub fn attach(&self) {
        unsafe { (self.attach)() }
    }

    pub fn detach(&self) {
        unsafe { (self.detach)() }
    }

    pub fn key(&self, code: u16, pressed: bool) {
        unsafe { (self.key)(code, pressed) }
    }

    pub fn leds(&self) -> u8 {
        unsafe { (self.leds)() }
    }
}

/// A device that does DMA ([`KernelApi::dma_attach`]): a PCI function, or a
/// memory-mapped device by the physical address of its registers.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct DmaDevice {
    /// [`DMA_DEVICE_PCI`] or [`DMA_DEVICE_MMIO`].
    pub kind: u32,
    pub bus: u8,
    pub slot: u8,
    pub func: u8,
    /// [`DMA_DEVICE_MMIO`]: [`MmioDevice::phys`].
    pub phys: u64,
    /// The device's bus addresses end below this (a 32-bit controller's
    /// 4 GiB: [`DmaDevice::below`]); 0 for none.
    pub limit: u64,
}

pub const DMA_DEVICE_PCI: u32 = 0;
pub const DMA_DEVICE_MMIO: u32 = 1;

impl DmaDevice {
    pub const fn pci(bus: u8, slot: u8, func: u8) -> Self {
        Self { kind: DMA_DEVICE_PCI, bus, slot, func, phys: 0, limit: 0 }
    }

    pub const fn mmio(phys: u64) -> Self {
        Self { kind: DMA_DEVICE_MMIO, bus: 0, slot: 0, func: 0, phys, limit: 0 }
    }

    /// The same device, whose DMA reaches bus addresses below `limit` only
    /// (`1 << 32` for a controller of 32-bit pointers).
    pub const fn below(self, limit: u64) -> Self {
        Self { limit, ..self }
    }
}

/// [`KernelApi::dma_sync`]: the CPU wrote, or the device is about to write
/// (no dirty cache line may land on its data later).
pub const DMA_TO_DEVICE: u32 = 1;
/// [`KernelApi::dma_sync`]: the device wrote, the CPU is about to read.
pub const DMA_FROM_DEVICE: u32 = 2;

/// A memory-mapped device from the device tree (`KernelApi::dt_mmio_find`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MmioDevice {
    /// Kernel virtual address of the register window.
    pub base: usize,
    /// Its physical address, as the device tree gives it.
    pub phys: u64,
    pub size: usize,
    /// Interrupt number as `irq::dispatch` / `irq_register` see it; 0 when
    /// the node has none or the kernel cannot route it.
    pub irq: u32,
    /// The node's phandle (what other nodes name it by), 0 when it has
    /// none.
    pub phandle: u32,
}

// ---- Sound (docs/sound.md) -------------------------------------------------
//
// The `sound` module owns `/dev/sound` and publishes a [`SoundOps`] table
// as the service [`SOUND_SERVICE`]. A sound card's driver (`virtio_sound`,
// `usb_audio`) registers the card with its streams, each a playback or a
// capture stream, and a [`SoundCardOps`] table the core starts and stops
// them through; each stream becomes a `/dev/sound/N` for the sound server.
// The PCM goes through the core's buffer: the driver takes what plays
// ([`SoundOps::playback_take`]) and hands over what it recorded
// ([`SoundOps::capture_put`]), from its own thread (not an interrupt
// handler). Samples are interleaved frames, little endian.

/// The name the sound core publishes its [`SoundOps`] under.
pub const SOUND_SERVICE: &str = "sound";
/// [`SoundOps::version`] and [`SoundCardOps::version`].
pub const SOUND_VERSION: u32 = 1;
/// [`SoundStreamInfo::direction`].
pub const SOUND_PLAYBACK: u8 = 0;
pub const SOUND_CAPTURE: u8 = 1;
/// Sample formats ([`SoundParams::format`]; bit `1 << format` in
/// [`SoundStreamInfo::formats`]): unsigned 8-bit, signed 16-bit.
pub const SOUND_FORMAT_U8: u8 = 0;
pub const SOUND_FORMAT_S16: u8 = 1;
/// The frame rates a stream may offer, bit `i` of
/// [`SoundStreamInfo::rates`] for `SOUND_RATES[i]` (virtio-sound's
/// `VIRTIO_SND_PCM_RATE_*` order).
pub const SOUND_RATES: [u32; 14] =
    [5512, 8000, 11025, 16000, 22050, 32000, 44100, 48000, 64000, 88200, 96000, 176400, 192000, 384000];

/// What a stream can do (a card's registration).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SoundStreamInfo {
    /// [`SOUND_PLAYBACK`] or [`SOUND_CAPTURE`].
    pub direction: u8,
    pub channels_min: u8,
    pub channels_max: u8,
    /// Bit `1 << SOUND_FORMAT_*` per format.
    pub formats: u8,
    /// Bit `i` per [`SOUND_RATES`]`[i]`.
    pub rates: u32,
}

/// How a stream runs ([`SoundCardOps::start`]).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SoundParams {
    pub rate: u32,
    pub channels: u8,
    /// `SOUND_FORMAT_*`.
    pub format: u8,
    pub _pad: u16,
    /// Bytes the driver moves at a time (whole frames), and the most the
    /// core buffers between the server and the driver (whole periods).
    pub period_bytes: u32,
    pub buffer_bytes: u32,
}

/// A card's driver, for the core: a stream started and stopped. `ctx` is
/// what the driver registered with the card; `stream` counts the card's
/// streams from 0. Called from task context, never with the core's lock
/// held.
#[repr(C)]
pub struct SoundCardOps {
    pub version: u32,
    /// Run `stream` as `params` says: from then on the driver takes its
    /// PCM ([`SoundOps::playback_take`]) or hands it over
    /// ([`SoundOps::capture_put`]) a period at a time. 0, or negative when
    /// the card cannot.
    pub start: unsafe extern "C" fn(ctx: usize, stream: u32, params: *const SoundParams) -> i32,
    /// Stop `stream`; nothing more is taken or handed over.
    pub stop: unsafe extern "C" fn(ctx: usize, stream: u32),
}

unsafe impl Sync for SoundCardOps {}

/// The sound core, for card drivers (the [`SOUND_SERVICE`] table).
#[repr(C)]
pub struct SoundOps {
    pub version: u32,
    /// A card named `name` (`virtio-sound 00:04.0`) with `n` streams
    /// (`streams[i]` what stream `i` can do), driven through `ops` with
    /// `ctx`: its number (>= 0), or negative when the core has no room.
    pub card_register: unsafe extern "C" fn(
        ops: *const SoundCardOps,
        ctx: usize,
        name: StrRef,
        streams: *const SoundStreamInfo,
        n: u32,
    ) -> i32,
    /// The card is gone (unplugged): its streams' files fail from now on,
    /// and the core calls it no more.
    pub card_remove: unsafe extern "C" fn(card: i32),
    /// Up to `len` bytes of playback stream `stream`'s PCM into `buf`: the
    /// bytes copied, fewer when the server has not written them yet (an
    /// underrun; the driver plays silence for the rest).
    pub playback_take: unsafe extern "C" fn(card: i32, stream: u32, buf: *mut u8, len: usize) -> usize,
    /// `len` bytes capture stream `stream` recorded; what the server has
    /// not read when the buffer is full is dropped, the oldest first.
    pub capture_put: unsafe extern "C" fn(card: i32, stream: u32, buf: *const u8, len: usize),
}

unsafe impl Sync for SoundOps {}

impl SoundOps {
    pub fn card_register(&self, ops: &'static SoundCardOps, ctx: usize, name: &str, streams: &[SoundStreamInfo]) -> i32 {
        unsafe { (self.card_register)(ops, ctx, StrRef::new(name), streams.as_ptr(), streams.len() as u32) }
    }

    pub fn card_remove(&self, card: i32) {
        unsafe { (self.card_remove)(card) }
    }

    pub fn playback_take(&self, card: i32, stream: u32, buf: &mut [u8]) -> usize {
        unsafe { (self.playback_take)(card, stream, buf.as_mut_ptr(), buf.len()) }
    }

    pub fn capture_put(&self, card: i32, stream: u32, buf: &[u8]) {
        unsafe { (self.capture_put)(card, stream, buf.as_ptr(), buf.len()) }
    }
}

/// Emit `[ OK ] label\n` via `KernelApi::write_str` (same spacing as `console::status_ok`).
pub fn status_ok(api: &KernelApi, label: &str) {
    status_line(api, "[ OK ] ", label);
}

/// Emit `[ FAIL ] label\n` via `KernelApi::write_str`.
pub fn status_fail(api: &KernelApi, label: &str) {
    status_line(api, "[ FAIL ] ", label);
}

/// Emit `[ INFO ] label\n` via `KernelApi::write_str`.
pub fn status_info(api: &KernelApi, label: &str) {
    status_line(api, "[ INFO ] ", label);
}

/// Emit `[ WARN ] label\n` via `KernelApi::write_str`.
pub fn status_warn(api: &KernelApi, label: &str) {
    status_line(api, "[ WARN ] ", label);
}

/// `tag`, `label` and the newline in one write, which the kernel keeps
/// whole on the console: in three, another CPU's output (a getty's
/// prompt) could land between them, and the boot test's harness, which
/// skips a status line wherever it starts, could not find the prompt
/// (issue #632). A label too long for the line goes in pieces.
fn status_line(api: &KernelApi, tag: &str, label: &str) {
    let mut line = [0u8; 160];
    match status_text(&mut line, tag, label) {
        Some(len) => api.write_bytes(&line[..len]),
        None => {
            api.write_str(tag);
            api.write_str(label);
            api.write_str("\n");
        }
    }
}

/// `tag`, `label` and a newline into `line`: their length, or `None` when
/// they do not fit.
fn status_text(line: &mut [u8], tag: &str, label: &str) -> Option<usize> {
    let len = tag.len() + label.len() + 1;
    let line = line.get_mut(..len)?;
    line[..tag.len()].copy_from_slice(tag.as_bytes());
    line[tag.len()..len - 1].copy_from_slice(label.as_bytes());
    line[len - 1] = b'\n';
    Some(len)
}

#[cfg(test)]
mod tests {
    use super::status_text;

    #[test]
    fn status_text_is_the_whole_line() {
        let mut line = [0u8; 160];
        let n = status_text(&mut line, "[ OK ] ", "linux").unwrap();
        assert_eq!(&line[..n], b"[ OK ] linux\n");
        let n = status_text(&mut line, "[ WARN ] ", "").unwrap();
        assert_eq!(&line[..n], b"[ WARN ] \n");
    }

    #[test]
    fn status_text_refuses_what_does_not_fit() {
        let mut line = [0u8; 13];
        assert_eq!(status_text(&mut line, "[ OK ] ", "linux"), Some(13));
        assert_eq!(status_text(&mut line, "[ OK ] ", "linux!"), None);
    }
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
