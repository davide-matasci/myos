//! Modern virtio-mmio v2 block device on QEMU `virt`.
//!
//! Transports sit at `0x0a000000`, stride `0x200`. Version 2 is required
//! (QEMU's default). The boot disk may already occupy a slot as virtio-blk;
//! we probe every device-id-2 transport and keep the one whose LBA 0 looks
//! like a FAT12/16 boot sector (the second QEMU disk). Polling only; no
//! virtio IRQ.
//!
//! DMA buffers live in cacheable HHDM RAM. QEMU TCG still needs D-cache
//! clean/invalidate around device-visible reads/writes or `used`/`status`
//! stay stale and every read times out (`fat mod failed`).

use core::fmt::Write;
use core::sync::atomic::{compiler_fence, Ordering};

use spin::Mutex;

use crate::blk::virtq;

const MMIO_BASE: usize = 0x0A00_0000;
const MMIO_STRIDE: usize = 0x200;
const MMIO_SLOTS: usize = 32;

const MAGIC: u32 = 0x7472_6976; // "virt"
const DEV_BLK: u32 = 2;
const VERSION_2: u32 = 2;

const REG_MAGIC: u32 = 0x000;
const REG_VERSION: u32 = 0x004;
const REG_DEVICE_ID: u32 = 0x008;
const REG_DEV_FEAT: u32 = 0x010;
const REG_DEV_FEAT_SEL: u32 = 0x014;
const REG_DRV_FEAT: u32 = 0x020;
const REG_DRV_FEAT_SEL: u32 = 0x024;
const REG_QUEUE_SEL: u32 = 0x030;
const REG_QUEUE_NUM_MAX: u32 = 0x034;
const REG_QUEUE_NUM: u32 = 0x038;
const REG_QUEUE_READY: u32 = 0x044;
const REG_QUEUE_NOTIFY: u32 = 0x050;
const REG_ISR: u32 = 0x060;
const REG_ISR_ACK: u32 = 0x064;
const REG_STATUS: u32 = 0x070;
const REG_DESC_LO: u32 = 0x080;
const REG_DESC_HI: u32 = 0x084;
const REG_AVAIL_LO: u32 = 0x090;
const REG_AVAIL_HI: u32 = 0x094;
const REG_USED_LO: u32 = 0x0A0;
const REG_USED_HI: u32 = 0x0A4;

const ACKNOWLEDGE: u32 = 1;
const DRIVER: u32 = 2;
const DRIVER_OK: u32 = 4;
const FEATURES_OK: u32 = 8;
const VIRTIO_F_VERSION_1: u32 = 1; // bit 32, in features dword 1

struct Dev {
    base: usize,
    num: u16,
    desc: *mut u8,
    avail: *mut u8,
    used: *mut u8,
    last_used: u16,
    dma_phys: u64,
    dma_va: *mut u8,
}

unsafe impl Send for Dev {}

static DEV: Mutex<Option<Dev>> = Mutex::new(None);

fn r32(base: usize, off: u32) -> u32 {
    unsafe { core::ptr::read_volatile((base + off as usize) as *const u32) }
}
fn w32(base: usize, off: u32, v: u32) {
    unsafe { core::ptr::write_volatile((base + off as usize) as *mut u32, v) }
}

fn dsb() {
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

/// Clean+invalidate D-cache lines covering `[va, va+len)` so the device and
/// CPU agree on DMA / virtqueue memory.
fn dcache_civac(va: *mut u8, len: usize) {
    if len == 0 {
        return;
    }
    unsafe {
        let mut addr = va as usize & !63;
        let end = va as usize + len;
        while addr < end {
            core::arch::asm!("dc civac, {x}", x = in(reg) addr, options(nostack));
            addr += 64;
        }
        core::arch::asm!("dsb sy", options(nostack));
    }
}

fn write_phys(base: usize, lo: u32, hi: u32, phys: u64) {
    w32(base, lo, phys as u32);
    w32(base, hi, (phys >> 32) as u32);
}

fn looks_like_fat_boot(sec: &[u8; 512]) -> bool {
    if sec[510] != 0x55 || sec[511] != 0xAA {
        return false;
    }
    let bps = u16::from_le_bytes([sec[11], sec[12]]) as usize;
    if bps != 512 {
        return false;
    }
    let spc = sec[13];
    if spc == 0 {
        return false;
    }
    let fats = sec[16];
    if fats == 0 {
        return false;
    }
    let fat_sz16 = u16::from_le_bytes([sec[22], sec[23]]);
    fat_sz16 != 0
}

pub fn init() {
    // Prefer higher slots (the data disk is usually after the boot disk).
    for i in (0..MMIO_SLOTS).rev() {
        let base = MMIO_BASE + i * MMIO_STRIDE;
        if r32(base, REG_MAGIC) != MAGIC {
            continue;
        }
        if r32(base, REG_DEVICE_ID) != DEV_BLK {
            continue;
        }
        let Some(mut dev) = setup(base) else {
            continue;
        };
        let mut sec = [0u8; 512];
        let base_copy = dev.base;
        let ok = unsafe {
            let num = dev.num;
            let desc = dev.desc;
            let avail = dev.avail;
            let used = dev.used;
            let dma_phys = dev.dma_phys;
            let dma_va = dev.dma_va;
            virtq::read_buf(
                num,
                desc,
                avail,
                used,
                &mut dev.last_used,
                dma_phys,
                dma_va,
                0,
                &mut sec,
                || notify_raw(base_copy, desc, avail, dma_va),
            )
            .is_ok()
                && looks_like_fat_boot(&sec)
        };
        if ok {
            // Sync last_used with the probe completion.
            dev.last_used = unsafe { core::ptr::read_volatile(dev.used.add(2) as *const u16) };
            *DEV.lock() = Some(dev);
            let mut serial = crate::arch::SerialPort::new();
            let _ = writeln!(serial, "virtio ok");
            return;
        }
    }
    let mut serial = crate::arch::SerialPort::new();
    let _ = writeln!(serial, "virtio none");
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

    Some(Dev {
        base,
        num,
        desc: desc_va,
        avail: avail_va,
        used: used_va,
        last_used: 0,
        dma_phys,
        dma_va,
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

pub fn read(lba: u64, buf: &mut [u8]) -> Result<(), ()> {
    let mut guard = DEV.lock();
    let dev = guard.as_mut().ok_or(())?;
    let base = dev.base;
    let desc = dev.desc;
    let avail = dev.avail;
    let used = dev.used;
    let dma_va = dev.dma_va;
    let result = unsafe {
        virtq::read_buf(
            dev.num,
            desc,
            avail,
            used,
            &mut dev.last_used,
            dev.dma_phys,
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
