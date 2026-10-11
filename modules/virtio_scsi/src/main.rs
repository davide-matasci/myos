//! virtio-scsi: the disks behind a virtio-scsi host (what a cloud VM's
//! disks usually are), as `/dev/sd*` through
//! [`myos_abi::KernelApi::blk_register`].
//!
//! `virtio_drivers` has no SCSI device, so this is one on its virtqueue
//! and transport (`myos_virtio` finds the host on PCI or virtio-mmio):
//! the host's request queue carries SCSI commands, one at a time, and the
//! driver speaks the little SCSI a disk needs: REPORT LUNS and INQUIRY to
//! find the disks, READ CAPACITY (16) for their size, READ (16) and
//! WRITE (16) for the data. The control and event queues are not used:
//! a disk added to a running host shows up after a `rescan` of
//! `/proc/pci`. Polling only; no virtio IRQ.

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use myos_abi::{ABI_VERSION, ApiCell, KernelApi, Lock, ModuleBlkOps, status_ok};
use myos_virtio::virtio_drivers::device::common::Feature;
use myos_virtio::virtio_drivers::queue::VirtQueue;
use myos_virtio::virtio_drivers::transport::{DeviceType, Transport};
use myos_virtio::{Device, Id, MAX_SHARE, MyosHal};

const MAX_HOSTS: usize = 4;
const MAX_DISKS: usize = 16;
/// Targets asked for their LUNs (the host's `max_target` can be 255; a
/// VM has its disks on the first few).
const MAX_TARGETS: u16 = 16;
const MAX_LUNS: usize = 8;
const SECTOR: usize = 512;
/// Request queue entries: one request is in flight at a time.
const QUEUE: usize = 16;

/// Queue 0 is the control queue, 1 the event queue; the requests go to
/// the first request queue.
const REQUEST_QUEUE: u16 = 2;

/// `struct virtio_scsi_config`.
const CFG_MAX_SECTORS: usize = 8;
const CFG_MAX_TARGET: usize = 30;

/// `struct virtio_scsi_req_cmd` with the default 32-byte CDB: lun[8],
/// tag, task_attr, prio, crn, cdb[32].
const REQ_LEN: usize = 8 + 8 + 3 + 32;
const REQ_CDB: usize = 19;
/// `struct virtio_scsi_resp_cmd` with the default 96 bytes of sense:
/// sense_len, resid, status_qualifier, status, response, sense[96].
const RESP_LEN: usize = 4 + 4 + 2 + 1 + 1 + 96;
const RESP_STATUS: usize = 10;
const RESP_RESPONSE: usize = 11;
const RESPONSE_OK: u8 = 0;
const STATUS_GOOD: u8 = 0;
const STATUS_CHECK_CONDITION: u8 = 2;

struct Host {
    id: Id,
    transport: Device,
    queue: VirtQueue<MyosHal, QUEUE>,
    /// The most sectors of one request the host takes.
    max_sectors: u32,
    max_target: u16,
}

// SAFETY: the host's queue and registers are used only by whoever holds
// its slot in `HOSTS`.
unsafe impl Send for Host {}

/// A disk: where it is and how big. Copied out of its slot for an I/O,
/// which then takes its host's lock.
#[derive(Clone, Copy)]
struct Disk {
    host: usize,
    target: u16,
    lun: u16,
    capacity: u64,
}

/// The hosts and the disks, a lock per slot: a request holds its host's
/// from the descriptors going in to the used ring saying it is done
/// (one request queue per host, so its disks' I/O is serial), and a probe
/// fills free slots without touching the others. A disk slot is never
/// moved or emptied: its index is the block device's context.
static HOSTS: [Lock<Option<Host>>; MAX_HOSTS] = [const { Lock::new(None) }; MAX_HOSTS];
static DISKS: [Lock<Option<Disk>>; MAX_DISKS] = [const { Lock::new(None) }; MAX_DISKS];
static API: ApiCell = ApiCell::new();

static OPS: ModuleBlkOps = ModuleBlkOps {
    read: blk_read,
    write: blk_write,
    capacity_sectors: blk_capacity,
};

fn api() -> &'static KernelApi {
    API.get()
}

/// What a command came back with: the host's response code, and the SCSI
/// status when the host passed the command on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Outcome {
    Good,
    CheckCondition,
    /// Any other SCSI status, or the host's response other than OK
    /// (`BAD_TARGET` for a target that is not there), or a queue error.
    Failed,
}

/// Run `cdb` on `target`/`lun`, with `out` going to the device and `data`
/// coming back (either may be empty: no descriptor is posted for it).
fn command(host: &mut Host, target: u16, lun: u16, cdb: &[u8], out: &[u8], data: &mut [u8]) -> Outcome {
    let mut req = [0u8; REQ_LEN];
    // Single-level LUN addressing: 1, target, then the LUN as SAM's
    // flat space (0x40 | high bits, low byte).
    req[0] = 1;
    req[1] = target as u8;
    req[2] = 0x40 | ((lun >> 8) as u8 & 0x3f);
    req[3] = lun as u8;
    req[REQ_CDB..REQ_CDB + cdb.len()].copy_from_slice(cdb);
    let mut resp = [0u8; RESP_LEN];
    let (q, t) = (&mut host.queue, &mut host.transport);
    let done = match (out.is_empty(), data.is_empty()) {
        (true, true) => q.add_notify_wait_pop(&[&req], &mut [&mut resp], t),
        (false, true) => q.add_notify_wait_pop(&[&req, out], &mut [&mut resp], t),
        (true, false) => q.add_notify_wait_pop(&[&req], &mut [&mut resp, data], t),
        (false, false) => q.add_notify_wait_pop(&[&req, out], &mut [&mut resp, data], t),
    };
    if done.is_err() || resp[RESP_RESPONSE] != RESPONSE_OK {
        return Outcome::Failed;
    }
    match resp[RESP_STATUS] {
        STATUS_GOOD => Outcome::Good,
        STATUS_CHECK_CONDITION => Outcome::CheckCondition,
        _ => Outcome::Failed,
    }
}

/// READ (16) or WRITE (16) of `buf` at `lba`, in requests the host takes.
fn transfer(disk: Disk, lba: u64, buf: &mut [u8], write: bool) -> bool {
    let Some(mut slot) = HOSTS.get(disk.host).map(|h| h.lock()) else {
        return false;
    };
    let Some(host) = slot.as_mut() else {
        return false;
    };
    let step = MAX_SHARE.min(host.max_sectors as usize * SECTOR);
    let mut done = 0usize;
    while done < buf.len() {
        let n = (buf.len() - done).min(step);
        let at = lba + (done / SECTOR) as u64;
        let mut cdb = [0u8; 16];
        cdb[0] = if write { 0x8A } else { 0x88 };
        cdb[2..10].copy_from_slice(&at.to_be_bytes());
        cdb[10..14].copy_from_slice(&((n / SECTOR) as u32).to_be_bytes());
        let chunk = &mut buf[done..done + n];
        let outcome = if write { command(host, disk.target, disk.lun, &cdb, chunk, &mut []) } else { command(host, disk.target, disk.lun, &cdb, &[], chunk) };
        if outcome != Outcome::Good {
            return false;
        }
        done += n;
    }
    true
}

fn disk(ctx: usize) -> Option<Disk> {
    *DISKS.get(ctx)?.lock()
}

unsafe extern "C" fn blk_read(ctx: usize, lba: u64, buf: *mut u8, len: usize) -> i32 {
    let Some(d) = disk(ctx) else {
        return -1;
    };
    if buf.is_null() || len % SECTOR != 0 || lba + (len / SECTOR) as u64 > d.capacity {
        return -1;
    }
    // SAFETY: the kernel hands `len` bytes at `buf` for the call.
    let buf = unsafe { core::slice::from_raw_parts_mut(buf, len) };
    if transfer(d, lba, buf, false) { 0 } else { -1 }
}

unsafe extern "C" fn blk_write(ctx: usize, lba: u64, buf: *const u8, len: usize) -> i32 {
    let Some(d) = disk(ctx) else {
        return -1;
    };
    if buf.is_null() || len % SECTOR != 0 || lba + (len / SECTOR) as u64 > d.capacity {
        return -1;
    }
    // SAFETY: as in `blk_read`; a write only reads the buffer (`command`
    // posts it device-readable).
    let buf = unsafe { core::slice::from_raw_parts_mut(buf.cast_mut(), len) };
    if transfer(d, lba, buf, true) { 0 } else { -1 }
}

unsafe extern "C" fn blk_capacity(ctx: usize) -> u64 {
    disk(ctx).map_or(0, |d| d.capacity)
}

// ---- finding the disks ----------------------------------------------------

/// The LUNs of `target` (REPORT LUNS); none when the target is not there.
/// A target that will not list them has LUN 0.
fn luns(host: &mut Host, target: u16, out: &mut [u16; MAX_LUNS]) -> usize {
    let mut data = [0u8; 8 + 8 * MAX_LUNS];
    let mut cdb = [0u8; 12];
    cdb[0] = 0xA0;
    cdb[6..10].copy_from_slice(&(data.len() as u32).to_be_bytes());
    match command(host, target, 0, &cdb, &[], &mut data) {
        Outcome::Good => {}
        Outcome::CheckCondition => {
            out[0] = 0;
            return 1;
        }
        Outcome::Failed => return 0,
    }
    let listed = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize / 8;
    let mut n = 0;
    for i in 0..listed.min(MAX_LUNS) {
        let l = &data[8 + i * 8..16 + i * 8];
        // Peripheral or flat addressing: the LUN is in the first two
        // bytes; the other methods address LUNs behind a bridge.
        if l[0] & 0xC0 == 0x80 {
            continue;
        }
        out[n] = (u16::from(l[0] & 0x3f) << 8) | u16::from(l[1]);
        n += 1;
    }
    n
}

/// The disk at `target`/`lun`: its sectors, when it is a direct-access
/// device of 512-byte sectors that is ready.
fn disk_capacity(host: &mut Host, target: u16, lun: u16) -> Option<u64> {
    // TEST UNIT READY: takes the unit attention a fresh device reports
    // (its first command answers CHECK CONDITION, power on), which would
    // otherwise fail the commands below.
    let _ = command(host, target, lun, &[0u8; 6], &[], &mut []);
    let mut inquiry = [0u8; 36];
    let cdb = [0x12u8, 0, 0, 0, 36, 0];
    if command(host, target, lun, &cdb, &[], &mut inquiry) != Outcome::Good || inquiry[0] & 0xE0 != 0 || inquiry[0] & 0x1f != 0 {
        return None;
    }
    // READ CAPACITY (16): the last LBA and the sector size.
    let mut data = [0u8; 32];
    let mut cdb = [0u8; 16];
    cdb[0] = 0x9E;
    cdb[1] = 0x10;
    cdb[10..14].copy_from_slice(&(data.len() as u32).to_be_bytes());
    let mut tries = 0;
    loop {
        match command(host, target, lun, &cdb, &[], &mut data) {
            Outcome::Good => break,
            Outcome::CheckCondition if tries < 2 => tries += 1,
            _ => return None,
        }
    }
    let last = u64::from_be_bytes(data[0..8].try_into().ok()?);
    let sector = u32::from_be_bytes(data[8..12].try_into().ok()?);
    if sector as usize != SECTOR {
        api().write_str("virtio-scsi: a disk whose sectors are not 512 bytes, skipped\n");
        return None;
    }
    Some(last + 1)
}

/// Register the disk; false when the tables are full.
fn add_disk(api: &KernelApi, disk: Disk) -> bool {
    let Some((n, mut free)) = DISKS.iter().enumerate().find_map(|(i, s)| {
        let s = s.lock();
        s.is_none().then_some((i, s))
    }) else {
        return false;
    };
    *free = Some(disk);
    if api.blk_register("sd?", &OPS, n) < 0 {
        *free = None;
        return false;
    }
    true
}

/// The disks of host `h`, except the ones already registered (a rescan).
fn scan(api: &KernelApi, h: usize) -> usize {
    let mut slot = HOSTS[h].lock();
    let Some(host) = slot.as_mut() else {
        return 0;
    };
    let have = |target: u16, lun: u16| DISKS.iter().any(|s| s.lock().is_some_and(|d| d.host == h && d.target == target && d.lun == lun));
    let mut new = 0;
    for target in 0..=host.max_target.min(MAX_TARGETS - 1) {
        let mut list = [0u16; MAX_LUNS];
        let n = luns(host, target, &mut list);
        for &lun in &list[..n] {
            if have(target, lun) {
                continue;
            }
            let Some(capacity) = disk_capacity(host, target, lun) else {
                continue;
            };
            if add_disk(api, Disk { host: h, target, lun, capacity }) {
                new += 1;
            }
        }
    }
    new
}

/// Whether the host is already up: a rescan must not reset it.
fn known(id: Id) -> bool {
    HOSTS.iter().any(|s| s.lock().as_ref().is_some_and(|h| h.id == id))
}

/// Bring up a host found on a transport: features, the request queue.
fn open(api: &KernelApi, id: Id, mut transport: Device) -> Option<Host> {
    let features = transport.begin_init(Feature::VERSION_1);
    let max_sectors = transport.read_config_space::<u32>(CFG_MAX_SECTORS).ok()?.max(1);
    let max_target = transport.read_config_space::<u16>(CFG_MAX_TARGET).ok()?;
    let queue = match VirtQueue::<MyosHal, QUEUE>::new(&mut transport, REQUEST_QUEUE, false, features.contains(Feature::RING_EVENT_IDX)) {
        Ok(q) => q,
        Err(_) => {
            api.write_str("virtio-scsi: a host without a usable request queue\n");
            return None;
        }
    };
    transport.finish_init();
    Some(Host { id, transport, queue, max_sectors, max_target })
}

/// Every virtio-scsi host and its disks: the new ones come up and
/// register as `/dev/sd*`, the known ones are asked for new disks. The
/// number of disks brought up by this call.
fn probe(api: &KernelApi) -> usize {
    myos_virtio::probe(DeviceType::ScsiHost, &mut known, &mut |id, transport| {
        let Some(mut free) = HOSTS.iter().find_map(|s| {
            let s = s.lock();
            s.is_none().then_some(s)
        }) else {
            return;
        };
        *free = open(api, id, transport);
    });
    (0..MAX_HOSTS).map(|h| scan(api, h)).sum()
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
        status_ok(api, "virtio scsi");
    }
    0
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

/// A `rescan` of `/proc/pci`: hosts and disks that appeared since init.
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_rescan() {
    probe(api());
}

// So `cargo build --bin virtio_scsi` links. The kernel never jumps here.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
