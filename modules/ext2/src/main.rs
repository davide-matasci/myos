//! ext2 kernel module: registers fstype `"ext2"`; `mount(2)` binds a block
//! device formatted by `mkfs.ext2` (or by Linux's `mke2fs -t ext2`).
//!
//! The filesystem itself is the `ext2fs` crate (`ext2fs/`, host-tested
//! against e2fsprogs); this module puts it on a block device and serves the
//! VFS hooks with it. One mount at a time; calls are serialized by a lock.

#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use ext2fs::{Device, Fs, Kind};
use myos_abi::{ABI_VERSION, KernelApi, ModuleVfsOps, VfsStatInfo};

static mut API: *const KernelApi = core::ptr::null();

fn api() -> &'static KernelApi {
    unsafe { &*API }
}

/// The kernel heap, through the ABI.
struct KernelHeap;

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { (api().alloc)(layout.size(), layout.align()) }
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
        unsafe { (api().blk_read_at)(self.0, offset, buf.as_mut_ptr(), buf.len()) == buf.len() as i32 }
    }
    fn write(&mut self, offset: u64, buf: &[u8]) -> bool {
        unsafe { (api().blk_write_at)(self.0, offset, buf.as_ptr(), buf.len()) == buf.len() as i32 }
    }
    fn now(&mut self) -> u32 {
        (unsafe { (api().wall_time_us)() } / 1_000_000) as u32
    }
}

/// The mounted filesystem, behind a spin lock.
struct Mounted {
    held: AtomicBool,
    fs: UnsafeCell<Option<Fs<Blk>>>,
}

unsafe impl Sync for Mounted {}

static MOUNTED: Mounted = Mounted { held: AtomicBool::new(false), fs: UnsafeCell::new(None) };

/// Run `f` on the mounted filesystem (`None` if there is none).
fn with_fs<T>(f: impl FnOnce(&mut Fs<Blk>) -> T) -> Option<T> {
    while MOUNTED.held.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    let r = unsafe { (*MOUNTED.fs.get()).as_mut().map(f) };
    MOUNTED.held.store(false, Ordering::Release);
    r
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

unsafe extern "C" fn ext2_stat(path: *const u8, path_len: usize, out: *mut VfsStatInfo) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    let Some(Ok(st)) = with_fs(|fs| fs.stat(path)) else { return -1 };
    let kind: u32 = match st.kind {
        Kind::Dir => 0o040000,
        Kind::Symlink => 0o120000,
        _ => 0o100000,
    };
    unsafe {
        *out = VfsStatInfo {
            mode: kind | (st.mode as u32 & 0o7777),
            size: st.size.min(u32::MAX as u64) as u32,
            ino: st.ino,
            nlink: st.links as u32,
        }
    };
    0
}

unsafe extern "C" fn ext2_listdir(
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
    let r = with_fs(|fs| {
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

unsafe extern "C" fn ext2_read(path: *const u8, path_len: usize, pos: usize, buf: *mut u8, buf_len: usize) -> i32 {
    let (Some(path), Some(out)) = (unsafe { text(path, path_len) }, unsafe { bytes_mut(buf, buf_len) }) else {
        return -1;
    };
    // A read past the end, or of something unreadable, reads nothing.
    count(with_fs(|fs| fs.read(path, pos as u64, out))).max(0)
}

unsafe extern "C" fn ext2_write(path: *const u8, path_len: usize, pos: usize, buf: *const u8, buf_len: usize) -> i32 {
    let (Some(path), Some(src)) = (unsafe { text(path, path_len) }, unsafe { bytes(buf, buf_len) }) else {
        return -1;
    };
    count(with_fs(|fs| fs.write(path, pos as u64, src)))
}

unsafe extern "C" fn ext2_create(path: *const u8, path_len: usize) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    rc(with_fs(|fs| fs.create(path)))
}

unsafe extern "C" fn ext2_truncate(path: *const u8, path_len: usize) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    rc(with_fs(|fs| fs.truncate(path)))
}

unsafe extern "C" fn ext2_mkdir(path: *const u8, path_len: usize) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    rc(with_fs(|fs| fs.mkdir(path)))
}

unsafe extern "C" fn ext2_rmdir(path: *const u8, path_len: usize) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    rc(with_fs(|fs| fs.rmdir(path)))
}

unsafe extern "C" fn ext2_unlink(path: *const u8, path_len: usize) -> i32 {
    let Some(path) = (unsafe { text(path, path_len) }) else { return -1 };
    rc(with_fs(|fs| fs.unlink(path)))
}

unsafe extern "C" fn ext2_rename(old: *const u8, old_len: usize, new: *const u8, new_len: usize) -> i32 {
    let (Some(old), Some(new)) = (unsafe { text(old, old_len) }, unsafe { text(new, new_len) }) else {
        return -1;
    };
    rc(with_fs(|fs| fs.rename(old, new)))
}

unsafe extern "C" fn ext2_symlink(target: *const u8, target_len: usize, link: *const u8, link_len: usize) -> i32 {
    let (Some(target), Some(link)) = (unsafe { text(target, target_len) }, unsafe { text(link, link_len) }) else {
        return -1;
    };
    rc(with_fs(|fs| fs.symlink(target, link)))
}

unsafe extern "C" fn ext2_readlink(path: *const u8, path_len: usize, buf: *mut u8, buf_len: usize) -> i32 {
    let (Some(path), Some(out)) = (unsafe { text(path, path_len) }, unsafe { bytes_mut(buf, buf_len) }) else {
        return -1;
    };
    count(with_fs(|fs| fs.readlink(path, out)))
}

/// `mount(2)` of a block device with fstype ext2: mount it (replacing the
/// previous mount) and hand the VFS the hooks.
unsafe extern "C" fn ext2_bind(dev_id: u32, ops: *mut ModuleVfsOps) -> i32 {
    if ops.is_null() {
        return -1;
    }
    let Ok(fs) = Fs::mount(Blk(dev_id)) else { return -1 };
    while MOUNTED.held.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    unsafe { *MOUNTED.fs.get() = Some(fs) };
    MOUNTED.held.store(false, Ordering::Release);
    unsafe {
        *ops = ModuleVfsOps {
            lookup: ext2_lookup,
            stat: ext2_stat,
            listdir: ext2_listdir,
            register: None,
            read: Some(ext2_read),
            write: Some(ext2_write),
            create: Some(ext2_create),
            truncate: Some(ext2_truncate),
            mkdir: Some(ext2_mkdir),
            rmdir: Some(ext2_rmdir),
            unlink: Some(ext2_unlink),
            rename: Some(ext2_rename),
            symlink: Some(ext2_symlink),
            readlink: Some(ext2_readlink),
            release: None,
        };
    }
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
    unsafe { API = api };
    unsafe { (api.fs_register)(b"ext2".as_ptr(), 4, ext2_bind) }
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
