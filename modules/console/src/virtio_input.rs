//! Virtio-mmio input (device id 18) for QEMU `virt` / UTM: the keyboard and
//! the pointers (a mouse's relative motion, a tablet's absolute position)
//! from the `virtio,mmio` nodes of the device tree. A device with relative
//! or absolute axes (its `EV_BITS` in the config space) is a pointer, into
//! `/dev/console/mouse` ([`crate::mouse`]); the first other one is the
//! keyboard.
//!
//! UTM SE routes the iPad keyboard through `usb-kbd` by default, which
//! `usb_hid` drives ([`crate::keyboards`]). Interrupt-driven when the
//! node's interrupt can be routed
//! (the handler only acks it and wakes the console's readers, which take
//! the events), polled otherwise.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::kbd::{ByteFifo, LinuxKeys};
use myos_abi::Lock as Mutex;
use crate::status_ok;
use myos_abi::{DMA_FROM_DEVICE, DMA_TO_DEVICE, DmaDevice};
use virtq::dsb;

const MAGIC: u32 = 0x7472_6976; // "virt"
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
/// The device's config space: `select`, `subsel`, `size`, then the data.
const REG_CONFIG: u32 = 0x100;
const CFG_EV_BITS: u8 = 0x11;
const CFG_ABS_INFO: u8 = 0x12;

const ACKNOWLEDGE: u32 = 1;
const DRIVER: u32 = 2;
const DRIVER_OK: u32 = 4;
const FEATURES_OK: u32 = 8;
const VIRTIO_F_VERSION_1: u32 = 1; // bit 32, in features dword 1

fn r32(base: usize, off: u32) -> u32 {
    unsafe { core::ptr::read_volatile((base + off as usize) as *const u32) }
}
fn w32(base: usize, off: u32, v: u32) {
    unsafe { core::ptr::write_volatile((base + off as usize) as *mut u32, v) }
}
fn write_phys(base: usize, lo: u32, hi: u32, phys: u64) {
    w32(base, lo, phys as u32);
    w32(base, hi, (phys >> 32) as u32);
}

/// One DMA page from the kernel for device `dma`: `(bus address, va)`.
fn dma_page(dma: i32) -> Option<(u64, *mut u8)> {
    let mut bus = 0u64;
    let va = crate::api().dma_alloc(dma, 1, &mut bus);
    if va.is_null() { None } else { Some((bus, va)) }
}

/// `len` bytes at `va` over to device `dma` (`DMA_TO_DEVICE`) or back.
fn sync(dma: i32, va: *mut u8, len: usize, dir: u32) {
    crate::api().dma_sync(dma, va, len, dir);
}

const DEV_INPUT: u32 = 18;

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const EV_ABS: u16 = 3;
const REL_X: u16 = 0;
const REL_Y: u16 = 1;
const REL_WHEEL: u16 = 8;
const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
/// `BTN_LEFT` up to `BTN_EXTRA`: a pointer's buttons, as the pointer's codes.
const BTN_FIRST: u16 = 0x110;
const BTN_LAST: u16 = 0x114;
/// Pointers driven at once.
const MAX_POINTERS: usize = 2;

const EVENT_SIZE: u32 = 8;
/// Event buffers posted to a device: QEMU's queue holds 64 events, and
/// drops a key event (with the `SYN_REPORT` after it) when the guest has
/// none posted. Eight gave a burst of keys four events of slack, 40 ms at
/// QEMU's `sendkey` pace (a press and a release 10 ms apart): a reader
/// woken that late under TCG lost one (issue #650). One page, a buffer
/// per cache line, so that a sync of one never touches another's data.
const NUM_BUFS: u16 = 64;
/// A buffer's stride in the events page: a cache line.
const EVENT_STRIDE: usize = 64;
const _: () = assert!(NUM_BUFS as usize * EVENT_STRIDE <= 4096 && EVENT_SIZE as usize <= EVENT_STRIDE);

struct Dev {
    base: usize,
    /// The device's DMA handle.
    dma: i32,
    num: u16,
    desc: *mut u8,
    avail: *mut u8,
    used: *mut u8,
    last_used: u16,
    event_phys: [u64; NUM_BUFS as usize],
    event_va: [*mut u8; NUM_BUFS as usize],
    /// A pointer's state; `None` for the keyboard.
    pointer: Option<Pointer>,
    /// The device interrupts on events (its handler acks them).
    irq: bool,
}

unsafe impl Send for Dev {}

/// A pointer's frame so far (evdev reports an axis per event, then
/// `SYN_REPORT`), and a tablet's ranges.
#[derive(Clone, Copy, Default)]
struct Pointer {
    /// `(min, max)` of `ABS_X` and `ABS_Y`; `None` for a mouse.
    abs: Option<[(i32, i32); 2]>,
    dx: i32,
    dy: i32,
    dz: i32,
    /// The position, as the device's own values, and whether it moved.
    x: i32,
    y: i32,
    moved: bool,
}

static DEV: Mutex<Option<Dev>> = Mutex::new(None);
static POINTERS: Mutex<[Option<Dev>; MAX_POINTERS]> = Mutex::new([const { None }; MAX_POINTERS]);
/// A pointer is polled (no interrupt): its readers pump it.
static POINTER_POLLED: AtomicBool = AtomicBool::new(false);
static READY: AtomicBool = AtomicBool::new(false);
/// The keyboard interrupts on events ([`interrupt`]); each `Dev` says its own.
static IRQ: AtomicBool = AtomicBool::new(false);
/// The keyboard's modifiers.
static KEYS: Mutex<LinuxKeys> = Mutex::new(LinuxKeys::new());
/// Bytes (single or multi-byte CSI sequences) waiting to drain from `poll_byte`.
static FIFO: Mutex<ByteFifo> = Mutex::new(ByteFifo::new());

fn notify_raw(dev: &Dev) {
    let (base, irq) = (dev.base, dev.irq);
    sync(dev.dma, dev.desc, 4096, DMA_TO_DEVICE);
    sync(dev.dma, dev.avail, 4096, DMA_TO_DEVICE);
    core::sync::atomic::compiler_fence(Ordering::SeqCst);
    w32(base, REG_QUEUE_NOTIFY, 0);
    // Polled: nobody else acks. With the interrupt, its handler does: an
    // ack here could take back an interrupt for an event this drain did
    // not see, and its wake with it.
    if !irq {
        let isr = r32(base, REG_ISR);
        if isr != 0 {
            w32(base, REG_ISR_ACK, isr);
        }
    }
}

/// The device's interrupt: ack it and wake the console's readers (they take
/// the events through `poll_byte` / `pump`). `ctx` is the register base.
unsafe extern "C" fn interrupt(ctx: *mut core::ffi::c_void) {
    let base = ctx as usize;
    let isr = r32(base, REG_ISR);
    if isr != 0 {
        w32(base, REG_ISR_ACK, isr);
    }
    crate::api().console_input();
}

unsafe fn avail_idx_ptr(avail: *mut u8) -> *mut u16 {
    unsafe { avail.add(2) as *mut u16 }
}

unsafe fn post_buf(dev: &mut Dev, id: u16) {
    sync(dev.dma, dev.event_va[id as usize], EVENT_SIZE as usize, DMA_TO_DEVICE);
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
    notify_raw(dev);
}

unsafe fn drain_events(dev: &mut Dev) {
    loop {
        sync(dev.dma, dev.used, 4096, DMA_FROM_DEVICE);
        let used_idx = unsafe { core::ptr::read_volatile(dev.used.add(2) as *const u16) };
        if used_idx == dev.last_used {
            break;
        }
        let slot = (dev.last_used as usize) % (dev.num as usize);
        let used_elem = unsafe { dev.used.add(4 + slot * 8) };
        // `struct virtq_used_elem { le32 id; le32 len; }`; `len` is always
        // one event here.
        let id = unsafe { core::ptr::read_volatile(used_elem as *const u32) } as u16;

        let va = dev.event_va[id as usize];
        sync(dev.dma, va, EVENT_SIZE as usize, DMA_FROM_DEVICE);
        let type_ = unsafe { core::ptr::read_volatile(va as *const u16) };
        let code = unsafe { core::ptr::read_volatile(va.add(2) as *const u16) };
        let value = unsafe { core::ptr::read_volatile(va.add(4) as *const u32) };
        match dev.pointer.as_mut() {
            Some(p) => pointer_event(p, type_, code, value as i32),
            None => handle_event(type_, code, value),
        }

        dev.last_used = dev.last_used.wrapping_add(1);
        unsafe { post_buf(dev, id) };
    }
}

/// `v` from `min..=max` to the pointer's absolute range.
fn scale(v: i32, (min, max): (i32, i32)) -> i32 {
    if max <= min {
        return 0;
    }
    let v = i64::from(v.clamp(min, max)) - i64::from(min);
    (v * i64::from(myos_abi::POINTER_ABS_MAX) / (i64::from(max) - i64::from(min))) as i32
}

/// One event of a pointer: axes add up until `SYN_REPORT` hands the frame
/// to the pointer, buttons go at once.
fn pointer_event(p: &mut Pointer, type_: u16, code: u16, value: i32) {
    let out = crate::mouse::pointer();
    match (type_, code) {
        (EV_REL, REL_X) => p.dx = p.dx.saturating_add(value),
        (EV_REL, REL_Y) => p.dy = p.dy.saturating_add(value),
        (EV_REL, REL_WHEEL) => p.dz = p.dz.saturating_add(value),
        (EV_ABS, ABS_X) => {
            p.x = value;
            p.moved = true;
        }
        (EV_ABS, ABS_Y) => {
            p.y = value;
            p.moved = true;
        }
        (EV_KEY, BTN_FIRST..=BTN_LAST) => out.button(code, value != 0),
        (EV_SYN, _) => {
            out.motion(p.dx, p.dy);
            out.wheel(p.dz);
            if let (true, Some([rx, ry])) = (p.moved, p.abs) {
                out.absolute(scale(p.x, rx), scale(p.y, ry));
            }
            (p.dx, p.dy, p.dz, p.moved) = (0, 0, 0, false);
        }
        _ => {}
    }
}

fn handle_event(type_: u16, code: u16, value: u32) {
    if type_ == EV_KEY {
        KEYS.lock().event(code, value, &mut FIFO.lock());
    }
}

/// Bytes the config space has for `select` / `subsel` (0: none).
fn config_size(base: usize, select: u8, subsel: u8) -> u8 {
    let cfg = base + REG_CONFIG as usize;
    unsafe {
        core::ptr::write_volatile(cfg as *mut u8, select);
        core::ptr::write_volatile((cfg + 1) as *mut u8, subsel);
        core::ptr::read_volatile((cfg + 2) as *const u8)
    }
}

/// Word `i` of the config data last selected.
fn config_u32(base: usize, i: usize) -> u32 {
    let data = base + REG_CONFIG as usize + 8;
    u32::from_le(unsafe { core::ptr::read_volatile((data + 4 * i) as *const u32) })
}

/// A pointer device's state, from its config space: one with absolute
/// axes is a tablet (their ranges), one with relative ones a mouse; `None`
/// for a keyboard.
fn pointer_kind(base: usize) -> Option<Pointer> {
    if config_size(base, CFG_EV_BITS, EV_ABS as u8) != 0 {
        let mut range = [(0, 0); 2];
        for (axis, r) in range.iter_mut().enumerate() {
            if config_size(base, CFG_ABS_INFO, axis as u8) >= 8 {
                *r = (config_u32(base, 0) as i32, config_u32(base, 1) as i32);
            }
        }
        return Some(Pointer { abs: Some(range), ..Pointer::default() });
    }
    (config_size(base, CFG_EV_BITS, EV_REL as u8) != 0).then(Pointer::default)
}

fn setup(base: usize, phys: u64) -> Option<Dev> {
    let dma = crate::api().dma_attach(&DmaDevice::mmio(phys));
    if dma < 0 {
        return None;
    }
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
    // The device's queue, up to the buffers there are (QEMU's is 64).
    let num = NUM_BUFS.min(max.min(128) as u16);
    w32(base, REG_QUEUE_NUM, u32::from(num));

    let (desc_phys, desc_va) = dma_page(dma)?;
    let (avail_phys, avail_va) = dma_page(dma)?;
    let (used_phys, used_va) = dma_page(dma)?;

    let (events_phys, events_va) = dma_page(dma)?;
    let mut event_phys = [0u64; NUM_BUFS as usize];
    let mut event_va = [core::ptr::null_mut(); NUM_BUFS as usize];
    for i in 0..NUM_BUFS as usize {
        event_phys[i] = events_phys + (i * EVENT_STRIDE) as u64;
        event_va[i] = unsafe { events_va.add(i * EVENT_STRIDE) };
    }

    w32(base, REG_QUEUE_READY, 0);
    write_phys(base, REG_DESC_LO, REG_DESC_HI, desc_phys);
    write_phys(base, REG_AVAIL_LO, REG_AVAIL_HI, avail_phys);
    write_phys(base, REG_USED_LO, REG_USED_HI, used_phys);
    for page in [desc_va, avail_va, used_va] {
        sync(dma, page, 4096, DMA_TO_DEVICE);
    }
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
        dma,
        num,
        desc: desc_va,
        avail: avail_va,
        used: used_va,
        last_used: 0,
        event_phys,
        event_va,
        pointer: None,
        irq: false,
    };

    for id in 0..num {
        unsafe { post_buf(&mut dev, id) };
    }

    Some(dev)
}

pub fn init() {
    let mut i = 0;
    let mut pointers = 0;
    loop {
        let mut node = myos_abi::MmioDevice::default();
        if crate::api().dt_mmio_find("virtio,mmio", i, &mut node) != 0 {
            break;
        }
        i += 1;
        let base = node.base;
        if r32(base, REG_MAGIC) != MAGIC {
            continue;
        }
        if r32(base, REG_DEVICE_ID) != DEV_INPUT {
            continue;
        }
        let pointer = pointer_kind(base);
        if pointer.is_none() && READY.load(Ordering::SeqCst) || pointer.is_some() && pointers == MAX_POINTERS {
            continue;
        }
        let Some(mut dev) = setup(base, node.phys) else {
            continue;
        };
        let name = if pointer.is_some() { "virtio-pointer" } else { "virtio-input" };
        let irq = node.irq != 0
            && crate::api().irq_enable(node.irq, name, interrupt, base as *mut core::ffi::c_void) == 0;
        if !irq {
            // Polled: the device need not interrupt.
            unsafe { virtq::set_avail_no_interrupt(dev.avail) };
        }
        dev.irq = irq;
        if let Some(p) = pointer {
            dev.pointer = Some(p);
            POINTERS.lock()[pointers] = Some(dev);
            pointers += 1;
            if !irq {
                POINTER_POLLED.store(true, Ordering::SeqCst);
            }
            status_ok(if p.abs.is_some() { "tablet (virtio)" } else { "mouse (virtio)" });
            continue;
        }
        IRQ.store(irq, Ordering::SeqCst);
        *DEV.lock() = Some(dev);
        READY.store(true, Ordering::SeqCst);
        status_ok("keyboard");
    }
}

pub fn present() -> bool {
    READY.load(Ordering::SeqCst)
}

pub fn irq() -> bool {
    IRQ.load(Ordering::SeqCst)
}

/// A pointer without an interrupt: its readers pump it.
pub fn pointer_polled() -> bool {
    POINTER_POLLED.load(Ordering::SeqCst)
}

pub fn poll_byte() -> Option<u8> {
    pump();
    FIFO.lock().pop()
}

/// Take the devices' pending events: the keyboard's (into the tty FIFO, or
/// the raw queue while `/dev/console/kbd` is held) and the pointers'.
pub fn pump() {
    if READY.load(Ordering::SeqCst) {
        if let Some(dev) = DEV.lock().as_mut() {
            unsafe { drain_events(dev) };
        }
    }
    for dev in POINTERS.lock().iter_mut().flatten() {
        unsafe { drain_events(dev) };
    }
}
