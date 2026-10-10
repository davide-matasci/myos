//! FAT kernel module: registers fstype `"fat"`; `mount(2)` binds a block
//! device holding a FAT16 or FAT32 volume (from its first sector: a
//! partition of a disk is not a device of its own yet, issue #353).
//!
//! The filesystem is the `fatvol` crate (`fatvol/`, host-tested against
//! dosfstools and mtools: fstool's FAT driver, with growing by zeros,
//! rename and file times of its own); this module puts it on a block
//! device and serves the VFS hooks with it, like the ext2 module. Up to
//! four volumes are mounted at once; the calls on each are serialized by
//! its lock. Nothing is cached past a call but the allocation table, which
//! `release` (the last fd on a file closed) and `umount` write back.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use fatvol::{Fat, Kind, SectorDriver};
use myos_abi::{ApiCell, ABI_VERSION, KernelApi, ModuleVfsOps, VfsStatInfo, MYOS_TIME_OMIT};

/// The kernel's table, set once by `module_init` before anything runs.
static API: ApiCell = ApiCell::new();

fn api() -> &'static KernelApi {
    API.get()
}

/// The kernel heap, through the ABI.
struct KernelHeap;

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        api().alloc(layout.size(), layout.align())
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { (api().dealloc)(ptr, layout.size(), layout.align()) }
    }
}

#[global_allocator]
static HEAP: KernelHeap = KernelHeap;

/// A block device registered with the kernel (`blk_*` id), in 512-byte
/// sectors.
struct Blk(u32);

impl SectorDriver for Blk {
    type Error = ();
    fn sector_size(&self) -> u32 {
        512
    }
    /// The ABI has no device size; the volume's own (its boot sector) is
    /// what bounds the accesses, and the block layer refuses one past the
    /// end of the disk.
    fn sector_count(&self) -> u64 {
        u64::MAX / 512
    }
    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
        if api().blk_read_at(self.0, lba * 512, buf) == buf.len() as i32 { Ok(()) } else { Err(()) }
    }
    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), ()> {
        if api().blk_write_at(self.0, lba * 512, buf) == buf.len() as i32 { Ok(()) } else { Err(()) }
    }
}

/// Mounted volumes, one per device. The VFS hooks carry no context, so
/// every slot has a hook set of its own (`ops::<S>()`).
const SLOTS: usize = 4;

struct Slot {
    held: AtomicBool,
    fs: UnsafeCell<Option<Fat<Blk>>>,
}

unsafe impl Sync for Slot {}

static MOUNTS: [Slot; SLOTS] = [const { Slot { held: AtomicBool::new(false), fs: UnsafeCell::new(None) } }; SLOTS];

/// Run `f` on the volume in slot `s` (`None` if there is none), with the
/// slot locked. A waiter yields rather than spins: the holder may have been
/// preempted on this very CPU (see the ext2 module, issue #348).
fn with_slot<T>(s: usize, f: impl FnOnce(&mut Option<Fat<Blk>>) -> T) -> T {
    let slot = &MOUNTS[s];
    while slot.held.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        api().task_yield();
    }
    let r = f(unsafe { &mut *slot.fs.get() });
    slot.held.store(false, Ordering::Release);
    r
}

/// Run `f` on the volume of slot `S`, what it changes stamped with the time now.
fn with_fs<const S: usize, T>(f: impl FnOnce(&mut Fat<Blk>) -> T) -> Option<T> {
    with_slot(S, |fs| {
        fs.as_mut().map(|fs| {
            fs.set_now((api().wall_time_us() / 1_000_000) as u32);
            f(fs)
        })
    })
}

fn rc<T>(r: Option<fatvol::Result<T>>) -> i32 {
    match r {
        Some(Ok(_)) => 0,
        _ => -1,
    }
}

/// A count of bytes as the hooks return it, or -1.
fn count(r: Option<fatvol::Result<usize>>) -> i32 {
    match r {
        Some(Ok(n)) => n.min(i32::MAX as usize) as i32,
        _ => -1,
    }
}

unsafe fn text<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
    if ptr.is_null() {
        return (len == 0).then_some("");
    }
    core::str::from_utf8(unsafe { core::slice::from_raw_parts(ptr, len) }).ok()
}

unsafe fn bytes_mut<'a>(ptr: *mut u8, len: usize) -> Option<&'a mut [u8]> {
    if ptr.is_null() {
        return (len == 0).then_some(&mut []);
    }
    Some(unsafe { core::slice::from_raw_parts_mut(ptr, len) })
}

unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if ptr.is_null() {
        return (len == 0).then_some(&[]);
    }
    Some(unsafe { core::slice::from_raw_parts(ptr, len) })
}

unsafe extern "C" fn fat_lookup(_: *const u8, _: usize, _: *mut *const u8, _: *mut usize) -> i32 {
    -1 // no static bytes: everything is read through `read`
}

unsafe extern "C" fn fat_stat<const S: usize>(path: *const u8, path_len: usize, out: *mut VfsStatInfo) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    let Some(Ok(st)) = with_fs::<S, _>(|fs| fs.stat(path)) else { return -1 };
    // FAT keeps no owner or permissions: everything may be read, written
    // and run, but what the read-only attribute protects.
    let (kind, nlink) = match st.kind {
        Kind::Dir => (0o040000, 2),
        Kind::File => (0o100000, 1),
    };
    let perm = if st.read_only { 0o555 } else { 0o755 };
    unsafe {
        *out = VfsStatInfo {
            mode: kind | perm,
            size: u64::from(st.size),
            ino: st.id,
            nlink,
            mtime: u64::from(st.mtime),
            atime: u64::from(st.atime),
        }
    };
    0
}

unsafe extern "C" fn fat_listdir<const S: usize>(
    path: *const u8,
    path_len: usize,
    buf: *mut u8,
    buf_len: usize,
    out_len: *mut usize,
) -> i32 {
    let (Some(path), Some(out)) = (unsafe { text(path, path_len) }, unsafe { bytes_mut(buf, buf_len) }) else {
        return -1;
    };
    let mut n = 0;
    let r = with_fs::<S, _>(|fs| {
        fs.list(path, |name| {
            if n + name.len() + 1 > out.len() {
                return false;
            }
            out[n..n + name.len()].copy_from_slice(name);
            out[n + name.len()] = b'\n';
            n += name.len() + 1;
            true
        })
    });
    unsafe { *out_len = n };
    rc(r)
}

unsafe extern "C" fn fat_read<const S: usize>(path: *const u8, path_len: usize, pos: usize, buf: *mut u8, buf_len: usize) -> i32 {
    let (Some(path), Some(out)) = (unsafe { text(path, path_len) }, unsafe { bytes_mut(buf, buf_len) }) else {
        return -1;
    };
    // A read past the end, or of something unreadable, reads nothing.
    count(with_fs::<S, _>(|fs| fs.read(path, pos as u64, out))).max(0)
}

unsafe extern "C" fn fat_write<const S: usize>(path: *const u8, path_len: usize, pos: usize, buf: *const u8, buf_len: usize) -> i32 {
    let (Some(path), Some(src)) = (unsafe { text(path, path_len) }, unsafe { bytes(buf, buf_len) }) else {
        return -1;
    };
    count(with_fs::<S, _>(|fs| fs.write(path, pos as u64, src)))
}

unsafe extern "C" fn fat_create<const S: usize>(path: *const u8, path_len: usize) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    rc(with_fs::<S, _>(|fs| fs.create(path)))
}

unsafe extern "C" fn fat_truncate<const S: usize>(path: *const u8, path_len: usize) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    rc(with_fs::<S, _>(|fs| fs.truncate(path)))
}

unsafe extern "C" fn fat_set_size<const S: usize>(path: *const u8, path_len: usize, size: u64) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    rc(with_fs::<S, _>(|fs| fs.set_size(path, size)))
}

unsafe extern "C" fn fat_mkdir<const S: usize>(path: *const u8, path_len: usize) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    rc(with_fs::<S, _>(|fs| fs.mkdir(path)))
}

unsafe extern "C" fn fat_rmdir<const S: usize>(path: *const u8, path_len: usize) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    rc(with_fs::<S, _>(|fs| fs.rmdir(path)))
}

unsafe extern "C" fn fat_unlink<const S: usize>(path: *const u8, path_len: usize) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    rc(with_fs::<S, _>(|fs| fs.unlink(path)))
}

unsafe extern "C" fn fat_rename<const S: usize>(old: *const u8, old_len: usize, new: *const u8, new_len: usize) -> i32 {
    let (Some(old), Some(new)) = (unsafe { text(old, old_len) }, unsafe { text(new, new_len) }) else {
        return -1;
    };
    rc(with_fs::<S, _>(|fs| fs.rename(old, new)))
}

/// FAT keeps 32-bit-range dates; `MYOS_TIME_OMIT` keeps the time.
fn entry_time(t: u64) -> Option<u32> {
    (t != MYOS_TIME_OMIT).then(|| t.min(u64::from(u32::MAX)) as u32)
}

unsafe extern "C" fn fat_set_times<const S: usize>(path: *const u8, path_len: usize, atime: u64, mtime: u64) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    rc(with_fs::<S, _>(|fs| fs.set_times(path, entry_time(atime), entry_time(mtime))))
}

/// The last fd on a file closed: write what is cached to the disk.
unsafe extern "C" fn fat_release<const S: usize>(_path: *const u8, _path_len: usize) -> i32 {
    rc(with_fs::<S, _>(|fs| fs.sync()))
}

/// `umount(2)`: write back what is cached and free the slot.
unsafe extern "C" fn fat_unmount<const S: usize>() {
    if let Some(fs) = with_slot(S, |f| f.take()) {
        let _ = fs.unmount();
    }
}

/// The hooks of slot `S`.
fn ops<const S: usize>() -> ModuleVfsOps {
    ModuleVfsOps {
        lookup: fat_lookup,
        stat: fat_stat::<S>,
        listdir: fat_listdir::<S>,
        register: None,
        read: Some(fat_read::<S>),
        write: Some(fat_write::<S>),
        create: Some(fat_create::<S>),
        truncate: Some(fat_truncate::<S>),
        mkdir: Some(fat_mkdir::<S>),
        rmdir: Some(fat_rmdir::<S>),
        unlink: Some(fat_unlink::<S>),
        rename: Some(fat_rename::<S>),
        symlink: None,
        readlink: None,
        release: Some(fat_release::<S>),
        mmap: None,
        poll: None,
        open: None,
        set_times: Some(fat_set_times::<S>),
        unmount: Some(fat_unmount::<S>),
        unlink_keep: None,
        read_ino: None,
        write_ino: None,
        stat_ino: None,
        forget_ino: None,
        set_size: Some(fat_set_size::<S>),
        set_size_ino: None,
        file_id: None,
        set_times_ino: None,
    }
}

/// `mount(2)` of a block device with fstype fat: mount it in the slot it
/// had (a remount) or a free one, and hand the VFS that slot's hooks.
/// Mounting writes nothing: a disk that is not FAT (`/ok` tries each
/// `vd*`) is left as it was.
unsafe extern "C" fn fat_bind(dev_id: u32, ops_out: *mut ModuleVfsOps) -> i32 {
    if ops_out.is_null() {
        return -1;
    }
    let Ok(fs) = Fat::mount(Blk(dev_id)) else { return -1 };
    let mine = |s: usize| with_slot(s, |f| f.as_ref().and_then(|f| f.device()).is_some_and(|d| d.0 == dev_id));
    let empty = |s: usize| with_slot(s, |f| f.is_none());
    let Some(s) = (0..SLOTS).find(|&s| mine(s)).or_else(|| (0..SLOTS).find(|&s| empty(s))) else {
        return -1;
    };
    with_slot(s, |f| {
        if let Some(old) = f.take() {
            let _ = old.unmount();
        }
        *f = Some(fs);
    });
    let ops = match s {
        0 => ops::<0>(),
        1 => ops::<1>(),
        2 => ops::<2>(),
        _ => ops::<3>(),
    };
    unsafe { *ops_out = ops };
    0
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api: *const KernelApi) -> i32 {
    if api.is_null() {
        return -1;
    }
    let api = unsafe { &*api };
    if api.abi_version != ABI_VERSION {
        return -2;
    }
    unsafe { API.set(api) };
    api.fs_register("fat", fat_bind)
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
