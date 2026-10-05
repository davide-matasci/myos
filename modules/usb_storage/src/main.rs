//! USB mass storage class driver (`docs/usb.md`): a memory stick or a disk
//! enclosure speaking the bulk-only transport with SCSI commands (class 8,
//! subclass 6, protocol 0x50), as `/dev/sdX` through
//! `KernelApi::blk_register`. One request at a time per disk: a command
//! block (CBW) out, the data, a status block (CSW) in; a stall is cleared
//! with the host's `clear_halt`. The disk goes away with its device
//! (`blk_unregister`), unless a filesystem is mounted from it: then it stays
//! in `/dev`, failing its I/O, until the mount is gone.

#![no_std]
#![no_main]

use myos_abi::{
    ABI_VERSION, KernelApi, ModuleBlkOps, StrRef, USB_ESTALL, USB_HOST_VERSION, USB_SERVICE,
    UsbDeviceInfo, UsbDriverOps, UsbHostOps, UsbInterfaceInfo, status_fail, status_ok,
};

const MAX_DISKS: usize = 8;
const SECTOR: usize = 512;
const CLASS_MASS_STORAGE: u8 = 8;
const SUBCLASS_SCSI: u8 = 6;
const PROTOCOL_BULK_ONLY: u8 = 0x50;
/// Bulk transfers wait this long (a slow stick spinning up).
const TIMEOUT_MS: u32 = 10_000;

struct Disk {
    dev: u32,
    intf: u8,
    ep_in: u8,
    ep_out: u8,
    /// The `blk_register` id, negative while unregistered.
    blk: i32,
    capacity: u64,
    tag: u32,
    gone: bool,
    busy: core::sync::atomic::AtomicBool,
    name: [u8; 3],
}

static mut API: Option<&'static KernelApi> = None;
static mut HOST: Option<&'static UsbHostOps> = None;
static mut DISKS: [Option<Disk>; MAX_DISKS] = [const { None }; MAX_DISKS];

static DRIVER: UsbDriverOps = UsbDriverOps {
    name: StrRef { ptr: b"usb_storage".as_ptr(), len: 11 },
    probe,
    disconnect,
};

static OPS: ModuleBlkOps = ModuleBlkOps {
    read: blk_read,
    write: blk_write,
    capacity_sectors: blk_capacity,
};

fn api() -> &'static KernelApi {
    unsafe { (*core::ptr::addr_of!(API)).expect("usb_storage: API") }
}

fn host() -> &'static UsbHostOps {
    unsafe { (*core::ptr::addr_of!(HOST)).expect("usb_storage: host") }
}

fn disks() -> &'static mut [Option<Disk>; MAX_DISKS] {
    unsafe { &mut *core::ptr::addr_of_mut!(DISKS) }
}

fn sleep_ms(ms: u64) {
    let until = unsafe { (api().monotonic_ns)() } + ms * 1_000_000;
    unsafe { (api().task_sleep_until)(until) };
}

/// One request at a time per disk: wait for the holder.
fn acquire(d: &Disk) {
    while d.busy.swap(true, core::sync::atomic::Ordering::Acquire) {
        unsafe { (api().task_yield)() };
    }
}

fn release(d: &Disk) {
    d.busy.store(false, core::sync::atomic::Ordering::Release);
}

fn bulk(dev: u32, ep: u8, data: *mut u8, len: usize) -> i32 {
    unsafe { (host().bulk)(dev, ep, data, len, TIMEOUT_MS) }
}

/// One SCSI command: the CBW, `data` (`to_host` IN) and the CSW. The bytes
/// moved, or negative (a failed command, a transport error).
fn scsi(d: &mut Disk, cb: &[u8], data: *mut u8, len: usize, to_host: bool) -> i32 {
    d.tag = d.tag.wrapping_add(1);
    let mut cbw = [0u8; 31];
    cbw[..4].copy_from_slice(b"USBC");
    cbw[4..8].copy_from_slice(&d.tag.to_le_bytes());
    cbw[8..12].copy_from_slice(&(len as u32).to_le_bytes());
    cbw[12] = if to_host { 0x80 } else { 0 };
    cbw[13] = 0; // LUN
    cbw[14] = cb.len() as u8;
    cbw[15..15 + cb.len()].copy_from_slice(cb);
    let r = bulk(d.dev, d.ep_out, cbw.as_mut_ptr(), 31);
    if r < 0 {
        return r;
    }
    let mut moved = 0usize;
    if len > 0 {
        let r = bulk(d.dev, if to_host { d.ep_in } else { d.ep_out }, data, len);
        if r == USB_ESTALL {
            let _ = unsafe { (host().clear_halt)(d.dev, if to_host { d.ep_in } else { d.ep_out }) };
        } else if r < 0 {
            return r;
        } else {
            moved = r as usize;
        }
    }
    let mut csw = [0u8; 13];
    let mut r = bulk(d.dev, d.ep_in, csw.as_mut_ptr(), 13);
    if r == USB_ESTALL {
        let _ = unsafe { (host().clear_halt)(d.dev, d.ep_in) };
        r = bulk(d.dev, d.ep_in, csw.as_mut_ptr(), 13);
    }
    if r < 0 {
        return r;
    }
    if r != 13 || &csw[..4] != b"USBS" || csw[12] != 0 {
        if csw[12] == 2 {
            // Phase error: the transport needs a reset.
            reset_recovery(d);
        }
        return -1;
    }
    moved as i32
}

/// Bulk-only mass storage reset, then both endpoints cleared.
fn reset_recovery(d: &Disk) {
    unsafe {
        (host().control)(d.dev, 0x21, 0xFF, 0, u16::from(d.intf), core::ptr::null_mut(), 0);
        (host().clear_halt)(d.dev, d.ep_in);
        (host().clear_halt)(d.dev, d.ep_out);
    }
}

fn request_sense(d: &mut Disk) {
    let mut sense = [0u8; 18];
    let cb = [0x03u8, 0, 0, 0, 18, 0];
    let _ = scsi(d, &cb, sense.as_mut_ptr(), 18, true);
}

/// Wait for the unit (a stick needs a moment after power): true when ready.
fn unit_ready(d: &mut Disk) -> bool {
    for _ in 0..10 {
        let cb = [0u8; 6];
        if scsi(d, &cb, core::ptr::null_mut(), 0, false) >= 0 {
            return true;
        }
        request_sense(d);
        sleep_ms(200);
    }
    false
}

/// READ CAPACITY(10): the sector count, if the sector is 512 bytes.
fn read_capacity(d: &mut Disk) -> Option<u64> {
    let mut cap = [0u8; 8];
    let cb = [0x25u8, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    for _ in 0..3 {
        if scsi(d, &cb, cap.as_mut_ptr(), 8, true) == 8 {
            let last = u32::from_be_bytes([cap[0], cap[1], cap[2], cap[3]]);
            let block = u32::from_be_bytes([cap[4], cap[5], cap[6], cap[7]]);
            if block as usize != SECTOR {
                return None;
            }
            return Some(u64::from(last) + 1);
        }
        request_sense(d);
    }
    None
}

/// READ(10) / WRITE(10) of `buf` (whole sectors) at `lba`.
fn rw(d: &mut Disk, lba: u64, buf: *mut u8, len: usize, write: bool) -> i32 {
    if lba > u64::from(u32::MAX) || len / SECTOR > 0xFFFF {
        return -1;
    }
    let sectors = (len / SECTOR) as u16;
    let lba = lba as u32;
    let mut cb = [0u8; 10];
    cb[0] = if write { 0x2A } else { 0x28 };
    cb[2..6].copy_from_slice(&lba.to_be_bytes());
    cb[7..9].copy_from_slice(&sectors.to_be_bytes());
    for attempt in 0..2 {
        let r = scsi(d, &cb, buf, len, !write);
        if r == len as i32 {
            return 0;
        }
        if d.gone {
            return -1;
        }
        if attempt == 0 {
            request_sense(d);
        }
    }
    -1
}

fn disk(ctx: usize) -> Option<&'static mut Disk> {
    disks().get_mut(ctx)?.as_mut()
}

unsafe extern "C" fn blk_read(ctx: usize, lba: u64, buf: *mut u8, len: usize) -> i32 {
    if buf.is_null() || len % SECTOR != 0 {
        return -1;
    }
    let Some(d) = disk(ctx) else {
        return -1;
    };
    if d.gone {
        return -1;
    }
    acquire(d);
    let r = rw(d, lba, buf, len, false);
    release(d);
    r
}

unsafe extern "C" fn blk_write(ctx: usize, lba: u64, buf: *const u8, len: usize) -> i32 {
    if buf.is_null() || len % SECTOR != 0 {
        return -1;
    }
    let Some(d) = disk(ctx) else {
        return -1;
    };
    if d.gone {
        return -1;
    }
    acquire(d);
    // `rw` only reads from `buf` when writing.
    let r = rw(d, lba, buf as *mut u8, len, true);
    release(d);
    r
}

unsafe extern "C" fn blk_capacity(ctx: usize) -> u64 {
    disk(ctx).map_or(0, |d| d.capacity)
}

/// The lowest `sdX` letter no disk in the table uses.
fn free_name() -> Option<[u8; 3]> {
    for letter in b'a'..=b'z' {
        let name = [b's', b'd', letter];
        if !disks().iter().flatten().any(|d| d.name == name) {
            return Some(name);
        }
    }
    None
}

unsafe extern "C" fn probe(dev: *const UsbDeviceInfo, intf: *const UsbInterfaceInfo) -> i32 {
    let (dev, intf) = unsafe { (&*dev, &*intf) };
    if intf.class != CLASS_MASS_STORAGE || intf.subclass != SUBCLASS_SCSI || intf.protocol != PROTOCOL_BULK_ONLY {
        return -1;
    }
    let eps = &intf.endpoints[..usize::from(intf.n_endpoints)];
    let ep_in = eps.iter().find(|e| e.attributes & 3 == 2 && e.address & 0x80 != 0).map(|e| e.address);
    let ep_out = eps.iter().find(|e| e.attributes & 3 == 2 && e.address & 0x80 == 0).map(|e| e.address);
    let (Some(ep_in), Some(ep_out)) = (ep_in, ep_out) else {
        return -1;
    };
    let Some(slot) = disks().iter().position(|d| d.is_none()) else {
        return -1;
    };
    let Some(name) = free_name() else {
        return -1;
    };
    disks()[slot] = Some(Disk {
        dev: dev.id,
        intf: intf.number,
        ep_in,
        ep_out,
        blk: -1,
        capacity: 0,
        tag: 0,
        gone: false,
        busy: core::sync::atomic::AtomicBool::new(false),
        name,
    });
    let d = disks()[slot].as_mut().unwrap();
    // INQUIRY (what it is; the answer is not needed), ready, capacity.
    let mut inquiry = [0u8; 36];
    let cb = [0x12u8, 0, 0, 0, 36, 0];
    let _ = scsi(d, &cb, inquiry.as_mut_ptr(), 36, true);
    if !unit_ready(d) {
        status_fail(api(), "usb_storage: unit not ready");
        disks()[slot] = None;
        return -1;
    }
    let Some(capacity) = read_capacity(d) else {
        status_fail(api(), "usb_storage: no capacity, or a sector not 512 bytes");
        disks()[slot] = None;
        return -1;
    };
    d.capacity = capacity;
    let id = unsafe { (api().blk_register)(name.as_ptr(), 3, &OPS, slot) };
    if id < 0 {
        status_fail(api(), "usb_storage: blk_register");
        disks()[slot] = None;
        return -1;
    }
    d.blk = id;
    // `/proc/usb` names the disk on its interface's line.
    let _ = unsafe { (host().interface_label)(dev.id, intf.number, name.as_ptr(), name.len()) };
    0
}

unsafe extern "C" fn disconnect(dev: u32, intf: u8) {
    for slot in 0..MAX_DISKS {
        let Some(d) = disks()[slot].as_mut() else {
            continue;
        };
        if d.dev != dev || d.intf != intf {
            continue;
        }
        d.gone = true;
        // Gone from /dev, unless a filesystem is mounted from it or a
        // program holds it open: then the entry stays, failing its I/O.
        if d.blk >= 0 && unsafe { (api().blk_unregister)(d.blk as u32) } == 0 {
            disks()[slot] = None;
        } else {
            status_fail(api(), "usb_storage: disk unplugged while in use");
        }
    }
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
        *core::ptr::addr_of_mut!(API) = Some(api);
    }
    let name = StrRef { ptr: USB_SERVICE.as_ptr(), len: USB_SERVICE.len() };
    let table = unsafe { (api.service_lookup)(name) } as *const UsbHostOps;
    if table.is_null() {
        unsafe { (api.write_str)(b"usb_storage: no usb host\n".as_ptr(), 25) };
        return -3;
    }
    let host: &'static UsbHostOps = unsafe { &*table };
    if host.version != USB_HOST_VERSION {
        return -4;
    }
    unsafe {
        *core::ptr::addr_of_mut!(HOST) = Some(host);
    }
    if unsafe { (host.driver_register)(&DRIVER) } != 0 {
        return -5;
    }
    status_ok(api, "usb_storage");
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    let api = api();
    status_fail(api, "usb_storage: panic");
    loop {
        unsafe { (api.task_yield)() };
    }
}
