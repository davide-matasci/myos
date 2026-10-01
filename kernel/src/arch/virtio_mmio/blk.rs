//! Modern virtio-mmio v2 block device on QEMU `virt`.
//!
//! Version 2 is required (QEMU's default). Every device-id-2 transport is
//! probed (cap `MAX_DISKS`) and exposed as `/dev/vd*`. Polling only.
//!
//! DMA buffers live in cacheable HHDM RAM. QEMU TCG on AArch64 still needs
//! D-cache clean/invalidate around device-visible reads/writes or
//! `used`/`status` stay stale and every read times out (`fat mod failed`).

use core::sync::atomic::{Ordering, compiler_fence};

use spin::Mutex;

use crate::blk::MAX_DISKS;
use crate::blk::virtq;
use super::*;

const DEV_BLK: u32 = 2;

struct Dev {
    base: usize,
    num: u16,
    desc: *mut u8,
    avail: *mut u8,
    used: *mut u8,
    last_used: u16,
    dma_phys: u64,
    dma_va: *mut u8,
    capacity: u64,
}

unsafe impl Send for Dev {}

static DEVS: Mutex<[Option<Dev>; MAX_DISKS]> = Mutex::new([const { None }; MAX_DISKS]);

pub fn init() {
    let mut table = DEVS.lock();
    let mut slot = 0usize;
    for i in 0..MMIO_SLOTS {
        if slot == MAX_DISKS {
            break;
        }
        let base = MMIO_BASE + i * MMIO_STRIDE;
        if r32(base, REG_MAGIC) != MAGIC {
            continue;
        }
        if r32(base, REG_DEVICE_ID) != DEV_BLK {
            continue;
        }
        let Some(dev) = setup(base) else {
            continue;
        };
        table[slot] = Some(dev);
        slot += 1;
    }
    if slot > 0 {
        crate::console::status_ok("virtio block");
    }
}

fn setup(base: usize) -> Option<Dev> {
    if r32(base, REG_VERSION) != VERSION_2 {
        return None;
    }

    w32(base, REG_STATUS, 0);
    dsb();
    w32(base, REG_STATUS, ACKNOWLEDGE);
    w32(base, REG_STATUS, ACKNOWLEDGE | DRIVER);

    w32(base, REG_DEV_FEAT_SEL, 1);
    let f1 = r32(base, REG_DEV_FEAT);
    w32(base, REG_DRV_FEAT_SEL, 0);
    w32(base, REG_DRV_FEAT, 0);
    w32(base, REG_DRV_FEAT_SEL, 1);
    w32(base, REG_DRV_FEAT, f1 & VIRTIO_F_VERSION_1);

    w32(base, REG_STATUS, ACKNOWLEDGE | DRIVER | FEATURES_OK);
    dsb();
    if r32(base, REG_STATUS) & FEATURES_OK == 0 {
        return None;
    }

    w32(base, REG_QUEUE_SEL, 0);
    let max = r32(base, REG_QUEUE_NUM_MAX);
    if max == 0 {
        return None;
    }
    let num = (max.min(128) as u16).max(1);
    w32(base, REG_QUEUE_NUM, u32::from(num));

    let (desc_phys, desc_va) = virtq::alloc_pages(1)?;
    let (avail_phys, avail_va) = virtq::alloc_pages(1)?;
    let (used_phys, used_va) = virtq::alloc_pages(1)?;
    let (dma_phys, dma_va) = virtq::alloc_pages(1)?;

    unsafe { virtq::set_avail_no_interrupt(avail_va) };

    w32(base, REG_QUEUE_READY, 0);
    write_phys(base, REG_DESC_LO, REG_DESC_HI, desc_phys);
    write_phys(base, REG_AVAIL_LO, REG_AVAIL_HI, avail_phys);
    write_phys(base, REG_USED_LO, REG_USED_HI, used_phys);
    dcache_civac(desc_va, 4096);
    dcache_civac(avail_va, 4096);
    dcache_civac(used_va, 4096);
    dcache_civac(dma_va, 4096);
    dsb();
    w32(base, REG_QUEUE_READY, 1);
    if r32(base, REG_QUEUE_READY) != 1 {
        return None;
    }

    w32(
        base,
        REG_STATUS,
        ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK,
    );
    dsb();

    let lo = r32(base, 0x100);
    let hi = r32(base, 0x104);
    let capacity = (u64::from(hi) << 32) | u64::from(lo);

    Some(Dev {
        base,
        num,
        desc: desc_va,
        avail: avail_va,
        used: used_va,
        last_used: 0,
        dma_phys,
        dma_va,
        capacity,
    })
}

fn notify_raw(base: usize, desc: *mut u8, avail: *mut u8, dma_va: *mut u8) {
    dcache_civac(desc, 4096);
    dcache_civac(avail, 4096);
    dcache_civac(dma_va, 512 + 16);
    compiler_fence(Ordering::SeqCst);
    dsb();
    w32(base, REG_QUEUE_NOTIFY, 0);
    let isr = r32(base, REG_ISR);
    if isr != 0 {
        w32(base, REG_ISR_ACK, isr);
    }
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
    let base = d.base;
    let desc = d.desc;
    let avail = d.avail;
    let used = d.used;
    let dma_va = d.dma_va;
    let result = unsafe {
        virtq::read_buf(
            d.num,
            desc,
            avail,
            used,
            &mut d.last_used,
            d.dma_phys,
            dma_va,
            lba,
            buf,
            || notify_raw(base, desc, avail, dma_va),
        )
    };
    dcache_civac(used, 4096);
    dcache_civac(dma_va, 512 + 16);
    result
}

pub fn write(dev: u32, lba: u64, buf: &[u8]) -> Result<(), ()> {
    let mut table = DEVS.lock();
    let slot = table.get_mut(dev as usize).ok_or(())?;
    let d = slot.as_mut().ok_or(())?;
    let base = d.base;
    let desc = d.desc;
    let avail = d.avail;
    let used = d.used;
    let dma_va = d.dma_va;
    let result = unsafe {
        virtq::write_buf(
            d.num,
            desc,
            avail,
            used,
            &mut d.last_used,
            d.dma_phys,
            dma_va,
            lba,
            buf,
            || notify_raw(base, desc, avail, dma_va),
        )
    };
    dcache_civac(used, 4096);
    dcache_civac(dma_va, 512 + 16);
    result
}
