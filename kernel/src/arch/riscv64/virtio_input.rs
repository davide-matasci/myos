//! Virtio-mmio input (device id 18) keyboard events for QEMU `virt` / UTM.
//!
//! UTM SE routes the iPad keyboard through `usb-kbd` by default; add
//! `virtio-keyboard-device` in the VM's QEMU settings (and remove `usb-kbd`
//! if both fight). Polling only, like the x86 PS/2 driver.

use core::sync::atomic::{AtomicBool, Ordering};

use spin::Mutex;

use crate::blk::virtq;
use crate::console;
use crate::kbd::{self, ByteFifo};

const MMIO_BASE: usize = 0x1000_1000;
const MMIO_STRIDE: usize = 0x1000;
const MMIO_SLOTS: usize = 8;

const MAGIC: u32 = 0x7472_6976;
const DEV_INPUT: u32 = 18;
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
const VIRTIO_F_VERSION_1: u32 = 1;

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;

const EVENT_SIZE: u32 = 8;
const NUM_BUFS: u16 = 8;

struct Dev {
    base: usize,
    num: u16,
    desc: *mut u8,
    avail: *mut u8,
    used: *mut u8,
    last_used: u16,
    event_phys: [u64; NUM_BUFS as usize],
    event_va: [*mut u8; NUM_BUFS as usize],
}

unsafe impl Send for Dev {}

static DEV: Mutex<Option<Dev>> = Mutex::new(None);
static READY: AtomicBool = AtomicBool::new(false);
static SHIFT: AtomicBool = AtomicBool::new(false);
static ALTGR: AtomicBool = AtomicBool::new(false);
static CTRL: AtomicBool = AtomicBool::new(false);
/// Bytes (single or multi-byte CSI sequences) waiting to drain from `poll_byte`.
static FIFO: Mutex<ByteFifo> = Mutex::new(ByteFifo::new());

/// Linux `KEY_*` codes for arrows (these differ from PS/2 set-1 makes).
const KEY_LEFT: u16 = 105;
const KEY_UP: u16 = 103;
const KEY_RIGHT: u16 = 106;
const KEY_DOWN: u16 = 108;

/// Map a virtio/Linux `KEY_*` code to the kernel's canonical set-1 space.
fn canonical(code: u16) -> Option<u8> {
    // Most Linux codes match PS/2 set-1 makes; arrows differ, so handle them
    // explicitly first.
    match code {
        KEY_UP => Some(kbd::KEY_UP),
        KEY_DOWN => Some(kbd::KEY_DOWN),
        KEY_LEFT => Some(kbd::KEY_LEFT),
        KEY_RIGHT => Some(kbd::KEY_RIGHT),
        _ => {
            if code > 127 {
                None
            } else {
                Some(code as u8)
            }
        }
    }
}

fn r32(base: usize, off: u32) -> u32 {
    unsafe { core::ptr::read_volatile((base + off as usize) as *const u32) }
}

fn w32(base: usize, off: u32, v: u32) {
    unsafe { core::ptr::write_volatile((base + off as usize) as *mut u32, v) }
}

fn dsb() {
    unsafe {
        core::arch::asm!("fence rw,rw", options(nostack, preserves_flags));
    }
}

fn dcache_civac(_va: *mut u8, _len: usize) {
    dsb();
}

fn write_phys(base: usize, lo: u32, hi: u32, phys: u64) {
    w32(base, lo, phys as u32);
    w32(base, hi, (phys >> 32) as u32);
}

fn notify_raw(base: usize, desc: *mut u8, avail: *mut u8) {
    dcache_civac(desc, 4096);
    dcache_civac(avail, 4096);
    core::sync::atomic::compiler_fence(Ordering::SeqCst);
    dsb();
    w32(base, REG_QUEUE_NOTIFY, 0);
    let isr = r32(base, REG_ISR);
    if isr != 0 {
        w32(base, REG_ISR_ACK, isr);
    }
}

unsafe fn avail_idx_ptr(avail: *mut u8) -> *mut u16 {
    avail.add(2) as *mut u16
}

unsafe fn post_buf(dev: &mut Dev, id: u16) {
    unsafe {
        virtq::write_desc(
            dev.desc,
            id,
            dev.event_phys[id as usize],
            EVENT_SIZE,
            virtq::DESC_F_WRITE,
            0,
        );
        let idx = core::ptr::read_volatile(avail_idx_ptr(dev.avail));
        let slot = (idx as usize) % (dev.num as usize);
        core::ptr::write_volatile(dev.avail.add(4 + slot * 2) as *mut u16, id);
        core::sync::atomic::compiler_fence(Ordering::Release);
        core::ptr::write_volatile(avail_idx_ptr(dev.avail), idx.wrapping_add(1));
    }
    notify_raw(dev.base, dev.desc, dev.avail);
}

unsafe fn drain_events(dev: &mut Dev) {
    loop {
        dcache_civac(dev.used, 4096);
        let used_idx = unsafe { core::ptr::read_volatile(dev.used.add(2) as *const u16) };
        if used_idx == dev.last_used {
            break;
        }
        let slot = (dev.last_used as usize) % (dev.num as usize);
        let used_elem = dev.used.add(4 + slot * 8);
        dcache_civac(used_elem, 8);
        let id = unsafe { core::ptr::read_volatile(used_elem as *const u16) };
        let _len = unsafe { core::ptr::read_volatile(used_elem.add(2) as *const u32) };

        let va = dev.event_va[id as usize];
        dcache_civac(va, EVENT_SIZE as usize);
        let type_ = unsafe { core::ptr::read_volatile(va as *const u16) };
        let code = unsafe { core::ptr::read_volatile(va.add(2) as *const u16) };
        let value = unsafe { core::ptr::read_volatile(va.add(4) as *const u32) };
        handle_event(type_, code, value);

        dev.last_used = dev.last_used.wrapping_add(1);
        unsafe { post_buf(dev, id) };
    }
}

fn handle_event(type_: u16, code: u16, value: u32) {
    if type_ == EV_SYN {
        return;
    }
    if type_ != EV_KEY {
        return;
    }
    // Linux KEY_* codes for modifiers.
    const KEY_LEFTSHIFT: u16 = 42;
    const KEY_RIGHTSHIFT: u16 = 54;
    const KEY_LEFTCTRL: u16 = 29;
    const KEY_RIGHTCTRL: u16 = 97;
    const KEY_RIGHTALT: u16 = 100; // AltGr
    if value == 0 {
        match code {
            KEY_LEFTSHIFT | KEY_RIGHTSHIFT => SHIFT.store(false, Ordering::SeqCst),
            KEY_RIGHTALT => ALTGR.store(false, Ordering::SeqCst),
            KEY_LEFTCTRL | KEY_RIGHTCTRL => CTRL.store(false, Ordering::SeqCst),
            _ => {}
        }
        return;
    }
    match code {
        KEY_LEFTSHIFT | KEY_RIGHTSHIFT => {
            SHIFT.store(true, Ordering::SeqCst);
        }
        KEY_RIGHTALT => {
            ALTGR.store(true, Ordering::SeqCst);
        }
        KEY_LEFTCTRL | KEY_RIGHTCTRL => {
            CTRL.store(true, Ordering::SeqCst);
        }
        _ => {
            let Some(kc) = canonical(code) else {
                return;
            };
            if let Some(kb) = kbd::translate(
                kc,
                SHIFT.load(Ordering::SeqCst),
                ALTGR.load(Ordering::SeqCst),
                CTRL.load(Ordering::SeqCst),
            ) {
                FIFO.lock().push_bytes(kb.as_slice());
            }
        }
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
    let num = (max.min(128) as u16).max(NUM_BUFS);
    w32(base, REG_QUEUE_NUM, u32::from(num));

    let (desc_phys, desc_va) = virtq::alloc_pages(1)?;
    let (avail_phys, avail_va) = virtq::alloc_pages(1)?;
    let (used_phys, used_va) = virtq::alloc_pages(1)?;

    let mut event_phys = [0u64; NUM_BUFS as usize];
    let mut event_va = [core::ptr::null_mut(); NUM_BUFS as usize];
    for i in 0..NUM_BUFS as usize {
        let (phys, va) = virtq::alloc_pages(1)?;
        event_phys[i] = phys;
        event_va[i] = va;
        unsafe { core::ptr::write_bytes(va, 0, EVENT_SIZE as usize) };
        dcache_civac(va, EVENT_SIZE as usize);
    }

    unsafe { virtq::set_avail_no_interrupt(avail_va) };

    w32(base, REG_QUEUE_READY, 0);
    write_phys(base, REG_DESC_LO, REG_DESC_HI, desc_phys);
    write_phys(base, REG_AVAIL_LO, REG_AVAIL_HI, avail_phys);
    write_phys(base, REG_USED_LO, REG_USED_HI, used_phys);
    dcache_civac(desc_va, 4096);
    dcache_civac(avail_va, 4096);
    dcache_civac(used_va, 4096);
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

    let mut dev = Dev {
        base,
        num,
        desc: desc_va,
        avail: avail_va,
        used: used_va,
        last_used: 0,
        event_phys,
        event_va,
    };

    for id in 0..NUM_BUFS {
        unsafe { post_buf(&mut dev, id) };
    }

    Some(dev)
}

pub fn init() {
    for i in 0..MMIO_SLOTS {
        let base = MMIO_BASE + i * MMIO_STRIDE;
        if r32(base, REG_MAGIC) != MAGIC {
            continue;
        }
        if r32(base, REG_DEVICE_ID) != DEV_INPUT {
            continue;
        }
        if let Some(dev) = setup(base) {
            *DEV.lock() = Some(dev);
            READY.store(true, Ordering::SeqCst);
            SHIFT.store(false, Ordering::SeqCst);
            ALTGR.store(false, Ordering::SeqCst);
            CTRL.store(false, Ordering::SeqCst);
            console::write_str("kbd ok\n");
            return;
        }
    }
}

pub fn present() -> bool {
    READY.load(Ordering::SeqCst)
}

pub fn poll_byte() -> Option<u8> {
    if !READY.load(Ordering::SeqCst) {
        return None;
    }
    {
        let mut guard = DEV.lock();
        if let Some(dev) = guard.as_mut() {
            unsafe { drain_events(dev) };
        }
    }
    FIFO.lock().pop()
}
