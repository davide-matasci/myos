//! Legacy I/O-BAR virtio-blk (transitional PCI device `0x1001`).
//!
//! QEMU is started with `disable-modern=on` so BAR0 is an I/O port bar
//! and the legacy queue-PFN interface applies. Polling only; no virtio IRQ.

use core::sync::atomic::{Ordering, compiler_fence};

use spin::Mutex;
use x86_64::instructions::port::Port;

use super::pci;
use crate::blk::MAX_DISKS;
use crate::blk::virtq;

const REG_DEV_FEAT: u16 = 0;
const REG_DRV_FEAT: u16 = 4;
const REG_QUEUE_PFN: u16 = 8;
const REG_QUEUE_NUM: u16 = 12;
const REG_QUEUE_SEL: u16 = 14;
const REG_QUEUE_NOTIFY: u16 = 16;
const REG_STATUS: u16 = 18;
const REG_ISR: u16 = 19;
const REG_CONFIG: u16 = 20;

const ACKNOWLEDGE: u8 = 1;
const DRIVER: u8 = 2;
const DRIVER_OK: u8 = 4;

const PAGE: usize = 4096;

struct Dev {
    iobase: u16,
    num: u16,
    desc: *mut u8,
    avail: *mut u8,
    used: *mut u8,
    last_used: u16,
    dma_phys: u64,
    dma_va: *mut u8,
    capacity: u64,
}

// The device is programmed once at init and then used from VFS / modules
// on the same CPU; the mutex is the Send/Sync boundary for the raw pointers.
unsafe impl Send for Dev {}

static DEVS: Mutex<[Option<Dev>; MAX_DISKS]> = Mutex::new([const { None }; MAX_DISKS]);

fn inb(port: u16) -> u8 {
    unsafe { Port::<u8>::new(port).read() }
}
fn inw(port: u16) -> u16 {
    unsafe { Port::<u16>::new(port).read() }
}
fn inl(port: u16) -> u32 {
    unsafe { Port::<u32>::new(port).read() }
}
fn outb(port: u16, v: u8) {
    unsafe { Port::<u8>::new(port).write(v) }
}
fn outw(port: u16, v: u16) {
    unsafe { Port::<u16>::new(port).write(v) }
}
fn outl(port: u16, v: u32) {
    unsafe { Port::<u32>::new(port).write(v) }
}

pub fn init() {
    let mut bdfs = [pci::Bdf {
        bus: 0,
        slot: 0,
        func: 0,
    }; MAX_DISKS];
    let n = pci::find_virtio_blk_legacy_io(&mut bdfs);
    let mut table = DEVS.lock();
    let mut slot = 0usize;
    for i in 0..n {
        let bdf = bdfs[i];
        pci::enable_bus_master(bdf);
        let Some(bar) = pci::bar0(bdf) else {
            continue;
        };
        if !bar.io {
            continue;
        }
        if let Some(dev) = setup(bar.addr as u16) {
            table[slot] = Some(dev);
            slot += 1;
            if slot == MAX_DISKS {
                break;
            }
        }
    }
}

fn setup(iobase: u16) -> Option<Dev> {
    outb(iobase + REG_STATUS, 0);
    outb(iobase + REG_STATUS, ACKNOWLEDGE);
    outb(iobase + REG_STATUS, ACKNOWLEDGE | DRIVER);
    // Accept no optional features; legacy does not use FEATURES_OK.
    let _host = { unsafe { Port::<u32>::new(iobase + REG_DEV_FEAT).read() } };
    outl(iobase + REG_DRV_FEAT, 0);

    outw(iobase + REG_QUEUE_SEL, 0);
    let num = inw(iobase + REG_QUEUE_NUM);
    if num == 0 || num > 256 {
        return None;
    }
    let bytes = virtq::vring_size(num as usize, PAGE);
    let pages = bytes.div_ceil(PAGE);
    let (vq_phys, vq_va) = virtq::alloc_pages(pages)?;
    let (dma_phys, dma_va) = virtq::alloc_pages(1)?;

    let used_off = virtq::used_offset(num as usize, PAGE);
    unsafe {
        virtq::set_avail_no_interrupt(vq_va.add(num as usize * virtq::DESC_SIZE));
    }

    compiler_fence(Ordering::SeqCst);
    outl(iobase + REG_QUEUE_PFN, (vq_phys / PAGE as u64) as u32);
    outb(iobase + REG_STATUS, ACKNOWLEDGE | DRIVER | DRIVER_OK);

    let lo = inl(iobase + REG_CONFIG);
    let hi = inl(iobase + REG_CONFIG + 4);
    let capacity = (u64::from(hi) << 32) | u64::from(lo);

    Some(Dev {
        iobase,
        num,
        desc: vq_va,
        avail: unsafe { vq_va.add(num as usize * virtq::DESC_SIZE) },
        used: unsafe { vq_va.add(used_off) },
        last_used: 0,
        dma_phys,
        dma_va,
        capacity,
    })
}

fn notify(iobase: u16) {
    outw(iobase + REG_QUEUE_NOTIFY, 0);
    let _ = inb(iobase + REG_ISR);
}

pub fn count() -> u32 {
    let table = DEVS.lock();
    table.iter().filter(|d| d.is_some()).count() as u32
}

pub fn capacity(dev: u32) -> Option<u64> {
    let table = DEVS.lock();
    table.get(dev as usize)?.as_ref().map(|d| d.capacity)
}

pub fn read(dev: u32, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
    let mut table = DEVS.lock();
    let slot = table.get_mut(dev as usize).ok_or(())?;
    let d = slot.as_mut().ok_or(())?;
    let iobase = d.iobase;
    unsafe {
        virtq::read_buf(
            d.num,
            d.desc,
            d.avail,
            d.used,
            &mut d.last_used,
            d.dma_phys,
            d.dma_va,
            lba,
            buf,
            || notify(iobase),
        )
    }
}

pub fn write(dev: u32, lba: u64, buf: &[u8]) -> Result<(), ()> {
    let mut table = DEVS.lock();
    let slot = table.get_mut(dev as usize).ok_or(())?;
    let d = slot.as_mut().ok_or(())?;
    let iobase = d.iobase;
    unsafe {
        virtq::write_buf(
            d.num,
            d.desc,
            d.avail,
            d.used,
            &mut d.last_used,
            d.dma_phys,
            d.dma_va,
            lba,
            buf,
            || notify(iobase),
        )
    }
}
