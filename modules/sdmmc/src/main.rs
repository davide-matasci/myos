//! SD cards: `/dev/mmcblkN` through [`myos_abi::KernelApi::blk_register`].
//!
//! Speaks only through the module ABI. One card per controller, brought up
//! by the card layer ([`card`]) behind the controller's driver: a standard
//! SDHCI one ([`sdhci`]: by PCI class `0x08` / `0x05`, or the device tree's
//! `brcm,bcm2711-emmc2`) or the Raspberry Pi 3's sdhost ([`sdhost`], from
//! the device tree). A memory card's blocks move by ADMA2 on an SDHCI
//! controller that has it (through the CPU otherwise, and on the sdhost:
//! issue #576), its waits sleep on the controller's interrupt where it is
//! routed, and it runs at high speed (CMD6, 50 MHz) when card and
//! controller can; the Pi's firmware says the controllers' clocks. Cap
//! [`MAX_SLOTS`] controllers.
//!
//! An SDIO card (the `sdio` crate's layer) is not a disk: it goes to
//! `/dev/sdio` ([`io`]) for a driver that is a program. The Raspberry Pi's
//! Wi-Fi chip is one, on the tree's `brcm,bcm2835-sdhci` controller,
//! powered through the firmware's GPIO expander first (`rpi_firmware`).

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

mod card;
mod io;
mod sdhci;
mod sdhost;

use core::fmt::Write;

use myos_abi::{
    ABI_VERSION, ApiCell, DiskInfo, DmaDevice, KernelApi, Lock, MmioDevice, ModuleBlkOps, RPI_CLOCK_CORE, RPI_CLOCK_EMMC,
    RPI_CLOCK_EMMC2, RPI_FIRMWARE_SERVICE, RPI_FIRMWARE_VERSION, RPI_GPIO_WL_ON, RpiFirmwareOps, status_ok,
};

use card::{BLOCK, Card, Host};
use sdhci::Sdhci;
use sdhost::Sdhost;

pub const MAX_SLOTS: usize = 4;
/// The most blocks one command moves (SDHCI counts 16 bits of them, the
/// sdhost 9; the ADMA2 bounce buffer holds as many).
pub const MAX_BLOCKS: usize = 128;

/// Where a controller is: its PCI function, or its registers' physical
/// address (a device tree node); a rescan leaves a known one alone.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Pci(u8, u8, u8),
    Mmio(u64),
}

pub enum Controller {
    Sdhci(Sdhci),
    Sdhost(Sdhost),
}

impl Controller {
    pub fn host(&mut self) -> &mut dyn Host {
        match self {
            Controller::Sdhci(h) => h,
            Controller::Sdhost(h) => h,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Controller::Sdhci(_) => "sdhci",
            Controller::Sdhost(_) => "sdhost",
        }
    }

    /// An SDHCI controller's registers (the one with a card interrupt).
    pub fn sdhci_regs(&self) -> Option<usize> {
        match self {
            Controller::Sdhci(h) => Some(h.regs()),
            Controller::Sdhost(_) => None,
        }
    }

    /// A memory card's controller set up for its transfers, as slot
    /// `slot`: DMA through `dma` where it has it, its interrupt from
    /// `irq` routed to its handler.
    fn route(&mut self, api: &KernelApi, slot: usize, dma: DmaDevice, irq: Irq) {
        let ctx = slot as *mut core::ffi::c_void;
        match self {
            Controller::Sdhci(h) => {
                h.use_dma(api.dma_attach(&dma));
                let routed = match irq {
                    Irq::Pci(bus, dev, func) => {
                        let mut msix = 0u16;
                        api.pci_irq_enable(bus, dev, func, "sdmmc", sdhci::irq_handler, ctx, &mut msix) == 0
                    }
                    Irq::Line(0) => false,
                    Irq::Line(line) => api.irq_enable(line, "sdmmc", sdhci::irq_handler, ctx) == 0,
                };
                if routed {
                    h.irq_routed(slot);
                }
            }
            Controller::Sdhost(h) => {
                if let Irq::Line(line @ 1..) = irq
                    && api.irq_enable(line, "sdmmc", sdhost::irq_handler, ctx) == 0
                {
                    h.irq_routed();
                }
            }
        }
    }

    /// `clock HZ dma adma2|none irq on|off`, for `/dev/<disk>/info`.
    fn describe(&self, info: &mut DiskInfo) {
        let (clock, dma, irq) = match self {
            Controller::Sdhci(h) => {
                let (dma, irq) = h.modes();
                (h.clock(), dma, irq)
            }
            Controller::Sdhost(h) => (h.clock(), "none", h.irq_mode()),
        };
        let _ = write!(info, " clock {clock} dma {dma} irq {irq}");
    }
}

/// Where a controller's interrupt comes from: its PCI function's, or a
/// device tree line (0: none).
#[derive(Clone, Copy)]
pub enum Irq {
    Pci(u8, u8, u8),
    Line(u32),
}

struct Slot {
    place: Place,
    controller: Controller,
    card: Card,
}

/// The attached cards, a lock per slot: a transfer holds its card's from
/// the command to its end, and a probe fills a free slot without touching
/// the others. A slot is never moved or emptied: its index is the block
/// device's context.
static SLOTS: [Lock<Option<Slot>>; MAX_SLOTS] = [const { Lock::new(None) }; MAX_SLOTS];
static API: ApiCell = ApiCell::new();

static OPS: ModuleBlkOps = ModuleBlkOps {
    read: blk_read,
    write: blk_write,
    capacity_sectors: blk_capacity,
};

pub fn api() -> &'static KernelApi {
    API.get()
}

pub fn now_ns() -> u64 {
    api().monotonic_ns()
}

/// Sleep `ms` milliseconds (task context: a probe, a transfer).
pub fn sleep_ms(ms: u64) {
    api().task_sleep_until(now_ns() + ms * 1_000_000);
}

/// Spin until `done`, for at most `timeout_ns`: whether it was.
pub fn until(timeout_ns: u64, mut done: impl FnMut() -> bool) -> bool {
    let deadline = now_ns() + timeout_ns;
    loop {
        if done() {
            return true;
        }
        if now_ns() > deadline {
            return false;
        }
        core::hint::spin_loop();
    }
}

fn known(place: Place) -> bool {
    SLOTS.iter().any(|s| s.lock().as_ref().is_some_and(|s| s.place == place)) || io::known(place)
}

/// The Raspberry Pi's firmware (`rpi_firmware`), when its module is there.
fn firmware(api: &KernelApi) -> Option<&'static RpiFirmwareOps> {
    let table = api.service_lookup(RPI_FIRMWARE_SERVICE) as *const RpiFirmwareOps;
    if table.is_null() || unsafe { (*table).version } != RPI_FIRMWARE_VERSION {
        return None;
    }
    Some(unsafe { &*table })
}

/// Clock `id`'s rate (`RPI_CLOCK_*`) as the firmware says it.
fn firmware_clock(api: &KernelApi, id: u32) -> Option<u32> {
    firmware(api)?.clock_rate(id)
}

/// Power the Pi's Wi-Fi chip through the firmware's GPIO expander
/// (`WL_REG_ON`, held low then high, then the chip's own start-up), when
/// the firmware module is there and takes the line (QEMU's does not).
fn power_wifi(api: &KernelApi) {
    let Some(fw) = firmware(api) else {
        return;
    };
    if !fw.gpio_output(RPI_GPIO_WL_ON, false) {
        return;
    }
    sleep_ms(10);
    fw.set_gpio(RPI_GPIO_WL_ON, true);
    sleep_ms(100);
}

/// Bring up the memory card behind `controller` into a free slot, its
/// DMA (`dma`) and interrupt (`irq`) set up, and register it as
/// `/dev/mmcblk<slot>`; the controller comes back when no card came up.
fn attach(api: &KernelApi, place: Place, mut controller: Controller, dma: DmaDevice, irq: Irq) -> Result<(), Controller> {
    let now_ms = || now_ns() / 1_000_000;
    let Some(card) = Card::init(controller.host(), &now_ms) else {
        return Err(controller);
    };
    let Some((n, mut free)) = SLOTS.iter().enumerate().find_map(|(i, s)| {
        let s = s.lock();
        s.is_none().then_some((i, s))
    }) else {
        return Err(controller);
    };
    // Before the disk is known: the function's interrupt is set up while
    // nothing else uses it.
    controller.route(api, n, dma, irq);
    let mut info = DiskInfo::new();
    let _ = match place {
        Place::Pci(bus, slot, func) => write!(info, "{} pci {bus:02x}:{slot:02x}.{func:x}", controller.host().name()),
        Place::Mmio(phys) => write!(info, "{} mmio {phys:#x}", controller.host().name()),
    };
    info.quoted("model", &card.product);
    let mut serial = [0u8; 8];
    for (i, b) in serial.iter_mut().enumerate() {
        *b = b"0123456789abcdef"[(card.serial >> (28 - 4 * i)) as usize & 0xf];
    }
    info.quoted("serial", &serial);
    let _ = write!(info, " speed {}", if card.high_speed { "high" } else { "default" });
    controller.describe(&mut info);
    *free = Some(Slot { place, controller, card });
    drop(free);
    // mmcblk{n} (n < 10: MAX_SLOTS is 4).
    let name = [b'm', b'm', b'c', b'b', b'l', b'k', b'0' + n as u8];
    api.blk_register(core::str::from_utf8(&name).unwrap_or_default(), &OPS, n, &info);
    Ok(())
}

const CLASS_SYSTEM: u8 = 0x08;
const SUBCLASS_SDHCI: u8 = 0x05;
/// The device tree's standard SDHCI controllers with a card slot (the
/// Pi 3's `brcm,bcm2835-sdhci` is its Wi-Fi's).
const DT_SDHCI: &[&str] = &["brcm,bcm2711-emmc2"];
const DT_SDHOST: &str = "brcm,bcm2835-sdhost";
/// The Pi 3's and 4's Arasan SDHCI, the Wi-Fi chip's (the tree's `mmcnr`).
const DT_WIFI_SDHCI: &str = "brcm,bcm2835-sdhci";

/// Every controller there is: the cards of the new ones come up, the
/// known ones are left alone. The number of cards attached by this call.
fn probe(api: &KernelApi) -> usize {
    let mut new = 0;
    for i in 0..MAX_SLOTS as u32 {
        let (mut bus, mut slot, mut func) = (0u8, 0u8, 0u8);
        if api.pci_find_class(CLASS_SYSTEM, SUBCLASS_SDHCI, i, &mut bus, &mut slot, &mut func) != 0 {
            break;
        }
        let place = Place::Pci(bus, slot, func);
        if known(place) {
            continue;
        }
        api.pci_enable(bus, slot, func);
        let (mut va, mut size) = (0usize, 0u64);
        if api.pci_bar_map(bus, slot, func, 0, &mut va, &mut size) != 0 {
            continue;
        }
        if let Some(h) = Sdhci::new(va, None, false) {
            let (dma, irq) = (DmaDevice::pci(bus, slot, func), Irq::Pci(bus, slot, func));
            new += usize::from(attach_any(api, place, Controller::Sdhci(h), dma, irq));
        }
    }
    let mut dt = |compatible: &str, kind: Dt| {
        for i in 0..MAX_SLOTS {
            let mut dev = MmioDevice::default();
            if api.dt_mmio_find(compatible, i, &mut dev) != 0 {
                break;
            }
            let place = Place::Mmio(dev.phys);
            if known(place) {
                continue;
            }
            if kind == Dt::Wifi {
                power_wifi(api);
            }
            let controller = match kind {
                Dt::Sdhost => Controller::Sdhost(Sdhost::new(dev.base, firmware_clock(api, RPI_CLOCK_CORE))),
                Dt::Sdhci | Dt::Wifi => {
                    let clock = firmware_clock(api, if kind == Dt::Wifi { RPI_CLOCK_EMMC } else { RPI_CLOCK_EMMC2 });
                    match Sdhci::new(dev.base, clock, kind == Dt::Wifi) {
                        Some(h) => Controller::Sdhci(h),
                        None => continue,
                    }
                }
            };
            new += usize::from(if kind == Dt::Wifi {
                io::attach(api, place, controller, dev.irq)
            } else {
                attach_any(api, place, controller, DmaDevice::mmio(dev.phys), Irq::Line(dev.irq))
            });
        }
    };
    for compatible in DT_SDHCI {
        dt(compatible, Dt::Sdhci);
    }
    dt(DT_SDHOST, Dt::Sdhost);
    dt(DT_WIFI_SDHCI, Dt::Wifi);
    new
}

/// What a device tree node's controller is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dt {
    Sdhci,
    Sdhost,
    /// The Pi's Wi-Fi chip's: an SDIO card, never a disk.
    Wifi,
}

/// A slot's card: a memory card, else an SDIO card (its interrupt the
/// card's, a device tree line's).
fn attach_any(api: &KernelApi, place: Place, controller: Controller, dma: DmaDevice, irq: Irq) -> bool {
    match attach(api, place, controller, dma, irq) {
        Ok(()) => true,
        Err(controller) => io::attach(api, place, controller, if let Irq::Line(line) = irq { line } else { 0 }),
    }
}

/// Move `buf` (whole sectors) [`MAX_BLOCKS`] at a time.
fn rw(s: &mut Slot, lba: u64, buf: &mut [u8], write: bool) -> i32 {
    if lba + (buf.len() / BLOCK) as u64 > s.card.capacity {
        return -1;
    }
    let mut lba = lba;
    for chunk in buf.chunks_mut(MAX_BLOCKS * BLOCK) {
        if s.card.transfer(s.controller.host(), lba, chunk, write).is_err() {
            return -1;
        }
        lba += (chunk.len() / BLOCK) as u64;
    }
    0
}

fn with_slot<R>(i: usize, f: impl FnOnce(&mut Slot) -> R) -> Option<R> {
    SLOTS.get(i)?.lock().as_mut().map(f)
}

unsafe extern "C" fn blk_read(ctx: usize, lba: u64, buf: *mut u8, len: usize) -> i32 {
    if buf.is_null() || len % BLOCK != 0 {
        return -1;
    }
    let buf = unsafe { core::slice::from_raw_parts_mut(buf, len) };
    with_slot(ctx, |s| rw(s, lba, buf, false)).unwrap_or(-1)
}

unsafe extern "C" fn blk_write(ctx: usize, lba: u64, buf: *const u8, len: usize) -> i32 {
    if buf.is_null() || len % BLOCK != 0 {
        return -1;
    }
    // `rw` only reads from `buf` when writing.
    let buf = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len) };
    with_slot(ctx, |s| rw(s, lba, buf, true)).unwrap_or(-1)
}

unsafe extern "C" fn blk_capacity(ctx: usize) -> u64 {
    with_slot(ctx, |s| s.card.capacity).unwrap_or(0)
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
    unsafe { API.set(api) };
    io::mount(api);
    if probe(api) == 0 {
        api.write_str("sdmmc none\n");
    } else {
        status_ok(api, "sdmmc");
    }
    0
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

/// A `rescan` of `/proc/pci`: controllers that appeared since init.
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_rescan() {
    probe(api());
}

// So `cargo build --bin sdmmc` links. The kernel never jumps here.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
