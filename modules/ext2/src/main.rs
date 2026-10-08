//! ext2 kernel module: registers fstype `"ext2"`; `mount(2)` binds a block
//! device formatted by `mkfs.ext2` (or by Linux's `mke2fs -t ext2`).
//!
//! The filesystem itself is the `ext2fs` crate (`ext2fs/`, host-tested
//! against e2fsprogs); this module puts it on a block device and serves the
//! VFS hooks with it. Up to four disks are mounted at once; the calls on
//! each are serialized by its lock.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use ext2fs::{Device, Fs, Kind};
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

/// A block device registered with the kernel (`blk_*` id).
struct Blk(u32);

impl Device for Blk {
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> bool {
        api().blk_read_at(self.0, offset, buf) == buf.len() as i32
    }
    fn write(&mut self, offset: u64, buf: &[u8]) -> bool {
        api().blk_write_at(self.0, offset, buf) == buf.len() as i32
    }
    fn now(&mut self) -> u32 {
        (api().wall_time_us() / 1_000_000) as u32
    }
}

/// Mounted filesystems, one per device. The VFS hooks carry no context, so
/// every slot has a hook set of its own (`ops::<S>()`).
const SLOTS: usize = 4;

struct Slot {
    held: AtomicBool,
    fs: UnsafeCell<Option<Fs<Blk>>>,
}

unsafe impl Sync for Slot {}

static MOUNTS: [Slot; SLOTS] = [const { Slot { held: AtomicBool::new(false), fs: UnsafeCell::new(None) } }; SLOTS];

/// Run `f` on the filesystem in slot `s` (`None` if there is none), with
/// the slot locked. A waiter yields rather than spins: the holder may have
/// been preempted on this very CPU (a user task runs on its home CPU only),
/// and a syscall waits with IRQs masked, so a spin would never see it let
/// go (issue #348).
fn with_slot<T>(s: usize, f: impl FnOnce(&mut Option<Fs<Blk>>) -> T) -> T {
    let slot = &MOUNTS[s];
    while slot.held.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        api().task_yield();
    }
    let r = f(unsafe { &mut *slot.fs.get() });
    slot.held.store(false, Ordering::Release);
    r
}

fn with_fs<const S: usize, T>(f: impl FnOnce(&mut Fs<Blk>) -> T) -> Option<T> {
    with_slot(S, |fs| fs.as_mut().map(f))
}

fn rc<T>(r: Option<ext2fs::Result<T>>) -> i32 {
    match r {
        Some(Ok(_)) => 0,
        _ => -1,
    }
}

/// A count of bytes as the hooks return it, or -1.
fn count(r: Option<ext2fs::Result<usize>>) -> i32 {
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

unsafe extern "C" fn ext2_lookup(_: *const u8, _: usize, _: *mut *const u8, _: *mut usize) -> i32 {
    -1 // no static bytes: everything is read through `read`
}

unsafe extern "C" fn ext2_stat<const S: usize>(path: *const u8, path_len: usize, out: *mut VfsStatInfo) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    let Some(Ok(st)) = with_fs::<S, _>(|fs| fs.stat(path)) else { return -1 };
    unsafe { *out = stat_info(&st) };
    0
}

fn stat_info(st: &ext2fs::Stat) -> VfsStatInfo {
    let kind: u32 = match st.kind {
        Kind::Dir => 0o040000,
        Kind::Symlink => 0o120000,
        _ => 0o100000,
    };
    VfsStatInfo {
        mode: kind | (st.mode as u32 & 0o7777),
        size: st.size.min(u32::MAX as u64) as u32,
        ino: st.ino,
        nlink: st.links as u32,
        mtime: u64::from(st.mtime),
        atime: u64::from(st.atime),
    }
}

unsafe extern "C" fn ext2_listdir<const S: usize>(
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

unsafe extern "C" fn ext2_read<const S: usize>(path: *const u8, path_len: usize, pos: usize, buf: *mut u8, buf_len: usize) -> i32 {
    let (Some(path), Some(out)) = (unsafe { text(path, path_len) }, unsafe { bytes_mut(buf, buf_len) }) else {
        return -1;
    };
    // A read past the end, or of something unreadable, reads nothing.
    count(with_fs::<S, _>(|fs| fs.read(path, pos as u64, out))).max(0)
}

unsafe extern "C" fn ext2_write<const S: usize>(path: *const u8, path_len: usize, pos: usize, buf: *const u8, buf_len: usize) -> i32 {
    let (Some(path), Some(src)) = (unsafe { text(path, path_len) }, unsafe { bytes(buf, buf_len) }) else {
        return -1;
    };
    count(with_fs::<S, _>(|fs| fs.write(path, pos as u64, src)))
}

unsafe extern "C" fn ext2_create<const S: usize>(path: *const u8, path_len: usize) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    rc(with_fs::<S, _>(|fs| fs.create(path)))
}

unsafe extern "C" fn ext2_truncate<const S: usize>(path: *const u8, path_len: usize) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    rc(with_fs::<S, _>(|fs| fs.truncate(path)))
}

unsafe extern "C" fn ext2_set_size<const S: usize>(path: *const u8, path_len: usize, size: u64) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    rc(with_fs::<S, _>(|fs| fs.set_size(path, size)))
}

unsafe extern "C" fn ext2_mkdir<const S: usize>(path: *const u8, path_len: usize) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    rc(with_fs::<S, _>(|fs| fs.mkdir(path)))
}

unsafe extern "C" fn ext2_rmdir<const S: usize>(path: *const u8, path_len: usize) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    rc(with_fs::<S, _>(|fs| fs.rmdir(path)))
}

unsafe extern "C" fn ext2_unlink<const S: usize>(path: *const u8, path_len: usize) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    rc(with_fs::<S, _>(|fs| fs.unlink(path)))
}

unsafe extern "C" fn ext2_rename<const S: usize>(old: *const u8, old_len: usize, new: *const u8, new_len: usize) -> i32 {
    let (Some(old), Some(new)) = (unsafe { text(old, old_len) }, unsafe { text(new, new_len) }) else {
        return -1;
    };
    rc(with_fs::<S, _>(|fs| fs.rename(old, new)))
}

unsafe extern "C" fn ext2_symlink<const S: usize>(target: *const u8, target_len: usize, link: *const u8, link_len: usize) -> i32 {
    let (Some(target), Some(link)) = (unsafe { text(target, target_len) }, unsafe { text(link, link_len) }) else {
        return -1;
    };
    rc(with_fs::<S, _>(|fs| fs.symlink(target, link)))
}

unsafe extern "C" fn ext2_readlink<const S: usize>(path: *const u8, path_len: usize, buf: *mut u8, buf_len: usize) -> i32 {
    let (Some(path), Some(out)) = (unsafe { text(path, path_len) }, unsafe { bytes_mut(buf, buf_len) }) else {
        return -1;
    };
    count(with_fs::<S, _>(|fs| fs.readlink(path, out)))
}

/// An inode keeps 32-bit seconds; `MYOS_TIME_OMIT` keeps the time.
fn inode_time(t: u64) -> Option<u32> {
    (t != MYOS_TIME_OMIT).then(|| t.min(u64::from(u32::MAX)) as u32)
}

unsafe extern "C" fn ext2_set_times<const S: usize>(path: *const u8, path_len: usize, atime: u64, mtime: u64) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    rc(with_fs::<S, _>(|fs| fs.set_times(path, inode_time(atime), inode_time(mtime))))
}

unsafe extern "C" fn ext2_unlink_keep<const S: usize>(path: *const u8, path_len: usize) -> i64 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    match with_fs::<S, _>(|fs| fs.unlink_keep(path)) {
        Some(Ok(ino)) => i64::from(ino),
        _ => -1,
    }
}

/// An inode number from the kernel (one `unlink_keep` gave).
fn ino32(ino: u64) -> Option<u32> {
    u32::try_from(ino).ok()
}

unsafe extern "C" fn ext2_read_ino<const S: usize>(ino: u64, pos: usize, buf: *mut u8, buf_len: usize) -> i32 {
    let (Some(ino), Some(out)) = (ino32(ino), unsafe { bytes_mut(buf, buf_len) }) else {
        return -1;
    };
    count(with_fs::<S, _>(|fs| fs.read_ino(ino, pos as u64, out))).max(0)
}

unsafe extern "C" fn ext2_write_ino<const S: usize>(ino: u64, pos: usize, buf: *const u8, buf_len: usize) -> i32 {
    let (Some(ino), Some(src)) = (ino32(ino), unsafe { bytes(buf, buf_len) }) else {
        return -1;
    };
    count(with_fs::<S, _>(|fs| fs.write_ino(ino, pos as u64, src)))
}

unsafe extern "C" fn ext2_stat_ino<const S: usize>(ino: u64, out: *mut VfsStatInfo) -> i32 {
    let Some(ino) = ino32(ino) else { return -1 };
    match with_fs::<S, _>(|fs| fs.stat_ino(ino)) {
        Some(Ok(st)) => {
            unsafe { *out = stat_info(&st) };
            0
        }
        _ => -1,
    }
}

unsafe extern "C" fn ext2_forget_ino<const S: usize>(ino: u64) -> i32 {
    let Some(ino) = ino32(ino) else { return -1 };
    rc(with_fs::<S, _>(|fs| fs.forget(ino)))
}

unsafe extern "C" fn ext2_file_id<const S: usize>(path: *const u8, path_len: usize) -> i64 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    match with_fs::<S, _>(|fs| fs.file_id(path)) {
        Some(Ok(ino)) => i64::from(ino),
        _ => -1,
    }
}

unsafe extern "C" fn ext2_set_times_ino<const S: usize>(ino: u64, atime: u64, mtime: u64) -> i32 {
    let Some(ino) = ino32(ino) else { return -1 };
    rc(with_fs::<S, _>(|fs| fs.set_times_ino(ino, inode_time(atime), inode_time(mtime))))
}

unsafe extern "C" fn ext2_set_size_ino<const S: usize>(ino: u64, size: u64) -> i32 {
    let Some(ino) = ino32(ino) else { return -1 };
    rc(with_fs::<S, _>(|fs| fs.set_size_ino(ino, size)))
}

/// The last fd on a file closed: write what is cached to the disk.
unsafe extern "C" fn ext2_release<const S: usize>(_path: *const u8, _path_len: usize) -> i32 {
    rc(with_fs::<S, _>(|fs| fs.sync()))
}

/// `umount(2)`: write back what is cached and free the slot.
unsafe extern "C" fn ext2_unmount<const S: usize>() {
    if let Some(fs) = with_slot(S, |f| f.take()) {
        let _ = fs.unmount();
    }
}

/// The hooks of slot `S`.
fn ops<const S: usize>() -> ModuleVfsOps {
    ModuleVfsOps {
        lookup: ext2_lookup,
        stat: ext2_stat::<S>,
        listdir: ext2_listdir::<S>,
        register: None,
        read: Some(ext2_read::<S>),
        write: Some(ext2_write::<S>),
        create: Some(ext2_create::<S>),
        truncate: Some(ext2_truncate::<S>),
        mkdir: Some(ext2_mkdir::<S>),
        rmdir: Some(ext2_rmdir::<S>),
        unlink: Some(ext2_unlink::<S>),
        rename: Some(ext2_rename::<S>),
        symlink: Some(ext2_symlink::<S>),
        readlink: Some(ext2_readlink::<S>),
        release: Some(ext2_release::<S>),
        mmap: None,
        poll: None,
        open: None,
        set_times: Some(ext2_set_times::<S>),
        unmount: Some(ext2_unmount::<S>),
        unlink_keep: Some(ext2_unlink_keep::<S>),
        read_ino: Some(ext2_read_ino::<S>),
        write_ino: Some(ext2_write_ino::<S>),
        stat_ino: Some(ext2_stat_ino::<S>),
        forget_ino: Some(ext2_forget_ino::<S>),
        set_size: Some(ext2_set_size::<S>),
        set_size_ino: Some(ext2_set_size_ino::<S>),
        file_id: Some(ext2_file_id::<S>),
        set_times_ino: Some(ext2_set_times_ino::<S>),
    }
}

/// `mount(2)` of a block device with fstype ext2: mount it in the slot it
/// had (a remount) or a free one, and hand the VFS that slot's hooks.
unsafe extern "C" fn ext2_bind(dev_id: u32, ops_out: *mut ModuleVfsOps) -> i32 {
    if ops_out.is_null() {
        return -1;
    }
    let Ok(fs) = Fs::mount(Blk(dev_id)) else { return -1 };
    let mine = |s: usize| with_slot(s, |f| f.as_ref().is_some_and(|f| f.device().0 == dev_id));
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
    api.fs_register("ext2", ext2_bind)
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
