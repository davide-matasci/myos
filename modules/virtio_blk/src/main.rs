//! virtio-blk: `/dev/vd*` through [`myos_abi::KernelApi::blk_register`].
//!
//! The device is `virtio_drivers`' block device over whichever transport
//! `myos_virtio` found it on: a PCI function (modern virtio-pci; a
//! transitional device with the modern layout too) or a `virtio,mmio`
//! node of the device tree (the QEMU `virt` boards). Polling only; no
//! virtio IRQ.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use core::fmt::Write;

use myos_abi::{ABI_VERSION, ApiCell, DiskInfo, KernelApi, Lock, ModuleBlkOps, status_ok};
use myos_virtio::virtio_drivers::device::blk::{SECTOR_SIZE, VirtIOBlk};
use myos_virtio::virtio_drivers::transport::DeviceType;
use myos_virtio::{Device, Id, MAX_SHARE, MyosHal};

const MAX_DISKS: usize = 8;

struct Dev {
    id: Id,
    blk: VirtIOBlk<MyosHal, Device>,
}

// SAFETY: the device's queue and registers are used only by whoever holds
// its slot in `DEVS`.
unsafe impl Send for Dev {}

/// The disks, a lock per slot: a request holds its disk's from the
/// descriptors going in to the used ring saying it is done (one queue per
/// disk, so its I/O is serial), and a probe fills a free slot without
/// touching the others. A slot is never moved or emptied: its index is
/// the block device's context.
static DEVS: [Lock<Option<Dev>>; MAX_DISKS] = [const { Lock::new(None) }; MAX_DISKS];
static API: ApiCell = ApiCell::new();

static OPS: ModuleBlkOps = ModuleBlkOps {
    read: blk_read,
    write: blk_write,
    capacity_sectors: blk_capacity,
};

fn api() -> &'static KernelApi {
    API.get()
}

/// Run `f` on the disk in slot `i` with its lock held; `None` for an
/// empty slot.
fn with_dev<R>(i: usize, f: impl FnOnce(&mut Dev) -> R) -> Option<R> {
    DEVS.get(i)?.lock().as_mut().map(f)
}

/// A request's worth of a transfer: the kernel's buffers are not DMA
/// memory, so they go through the Hal's bounce slots, one per request.
fn requests(lba: u64, len: usize) -> impl Iterator<Item = (u64, usize, usize)> {
    (0..len).step_by(MAX_SHARE).map(move |off| (lba + (off / SECTOR_SIZE) as u64, off, (len - off).min(MAX_SHARE)))
}

unsafe extern "C" fn blk_read(ctx: usize, lba: u64, buf: *mut u8, len: usize) -> i32 {
    if buf.is_null() || len % SECTOR_SIZE != 0 {
        return -1;
    }
    // SAFETY: the kernel hands `len` bytes at `buf` for the call.
    let buf = unsafe { core::slice::from_raw_parts_mut(buf, len) };
    with_dev(ctx, |d| {
        for (lba, off, n) in requests(lba, len) {
            if d.blk.read_blocks(lba as usize, &mut buf[off..off + n]).is_err() {
                return -1;
            }
        }
        0
    })
    .unwrap_or(-1)
}

unsafe extern "C" fn blk_write(ctx: usize, lba: u64, buf: *const u8, len: usize) -> i32 {
    if buf.is_null() || len % SECTOR_SIZE != 0 {
        return -1;
    }
    // SAFETY: as in `blk_read`.
    let buf = unsafe { core::slice::from_raw_parts(buf, len) };
    with_dev(ctx, |d| {
        for (lba, off, n) in requests(lba, len) {
            if d.blk.write_blocks(lba as usize, &buf[off..off + n]).is_err() {
                return -1;
            }
        }
        0
    })
    .unwrap_or(-1)
}

unsafe extern "C" fn blk_capacity(ctx: usize) -> u64 {
    with_dev(ctx, |d| d.blk.capacity()).unwrap_or(0)
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api_ptr: *const KernelApi) -> i32 {
    if api_ptr.is_null() {
        return -1;
    }
    let api: &'static KernelApi = unsafe { &*api_ptr };
    if api.abi_version != ABI_VERSION {
        return -2;
    }
    unsafe {
        API.set(api);
        myos_virtio::init(api);
    }
    if probe(api) > 0 {
        status_ok(api, "virtio block");
    }
    0
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

/// A `rescan` of `/proc/pci`: disks that appeared since init.
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_rescan() {
    probe(api());
}

/// Whether the device is already up: a rescan must not reset it.
fn known(id: Id) -> bool {
    DEVS.iter().any(|s| s.lock().as_ref().is_some_and(|d| d.id == id))
}

/// Every virtio-blk device: the new ones come up and register as
/// `/dev/vd<slot>`, the known ones are left alone. The number of disks
/// brought up by this call.
fn probe(api: &KernelApi) -> usize {
    let mut new = 0usize;
    myos_virtio::probe(DeviceType::Block, &mut known, &mut |id, transport| {
        let Some((n, mut free)) = DEVS.iter().enumerate().find_map(|(i, s)| {
            let s = s.lock();
            s.is_none().then_some((i, s))
        }) else {
            return;
        };
        let Ok(mut blk) = VirtIOBlk::<MyosHal, Device>::new(transport) else {
            api.write_str("virtio-blk: a device did not come up\n");
            return;
        };
        // `/dev/<disk>/info`: where it is, read-only or not, the id the host
        // gave it (QEMU's `serial=`).
        let mut info = DiskInfo::new();
        let _ = write!(info, "virtio_blk {id}{}", if blk.readonly() { " ro" } else { "" });
        let mut serial = [0u8; 20];
        if let Ok(len) = blk.device_id(&mut serial) {
            info.quoted("serial", &serial[..len]);
        }
        let name = [b'v', b'd', b'a' + n as u8];
        *free = Some(Dev { id, blk });
        if api.blk_register(core::str::from_utf8(&name).unwrap_or_default(), &OPS, n, &info) < 0 {
            *free = None;
            return;
        }
        new += 1;
    });
    new
}

// So `cargo build --bin virtio_blk` links. The kernel never jumps here.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
