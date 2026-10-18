//! What the virtio kernel modules share: [`virtio_drivers`] bound to the
//! module ABI. [`MyosHal`] is the crate's `Hal` (DMA pages from
//! `KernelApi::dma_alloc`, PCI BARs the kernel mapped, a bounce pool for
//! buffers that are not DMA memory), [`probe_pci`] and [`probe_mmio`] find
//! the devices of a kind and open their transports, and the interrupt
//! helpers do what the crate leaves to the driver: route a device's
//! interrupt, point a queue at an MSI-X entry, acknowledge an INTx line.
//!
//! Each module links its own copy of this crate: the tables below are per
//! module, which is also how the kernel hands out the devices (a PCI
//! function is only ever opened by the module of its device type).

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

use core::ptr::NonNull;
use core::sync::atomic::{Ordering, compiler_fence};

use myos_abi::{ApiCell, KernelApi, Lock, MmioDevice};
pub use virtio_drivers;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::pci::bus::{ConfigurationAccess, DeviceFunction, PciRoot};
use virtio_drivers::transport::pci::{PciTransport, VIRTIO_VENDOR_ID};
use virtio_drivers::transport::{DeviceType, SomeTransport, Transport};
use virtio_drivers::{BufferDirection, Hal, PAGE_SIZE, PhysAddr};

/// The transport of a device this crate opened.
pub type Device = SomeTransport<'static>;

static API: ApiCell = ApiCell::new();

/// Hand the crate the kernel API; before any other call.
///
/// # Safety
/// `api` is the table `module_init` received, valid for the module's life.
pub unsafe fn init(api: &'static KernelApi) {
    unsafe { API.set(api) };
    // The crate's warnings on the console (`virtio WARN: ...`); the first
    // module to come up installs the logger, the others' calls fail.
    let _ = log::set_logger(&Console);
    log::set_max_level(log::LevelFilter::Warn);
}

struct Console;

impl log::Log for Console {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Warn
    }
    fn log(&self, record: &log::Record) {
        use core::fmt::Write;
        struct Out;
        impl Write for Out {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                api().write_str(s);
                Ok(())
            }
        }
        if self.enabled(record.metadata()) {
            let _ = writeln!(Out, "virtio {}: {}", record.level(), record.args());
        }
    }
    fn flush(&self) {}
}

fn api() -> &'static KernelApi {
    API.get()
}

/// A device, as a rescan tells one already up from a new one: its PCI
/// function, or its virtio-mmio window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Id {
    Pci { bus: u8, slot: u8, func: u8 },
    /// `base` is the kernel virtual address of the window, `phys` its
    /// physical one; `irq` the interrupt the device tree gives it (0: none).
    Mmio { base: usize, phys: u64, irq: u32 },
}

/// Where the device is, as `/dev/<disk>/info` says it: `pci 00:04.0`,
/// `mmio 0xa003e00`.
impl core::fmt::Display for Id {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Id::Pci { bus, slot, func } => write!(f, "pci {bus:02x}:{slot:02x}.{func:x}"),
            Id::Mmio { phys, .. } => write!(f, "mmio {phys:#x}"),
        }
    }
}

// ---- memory ---------------------------------------------------------------

/// A run of memory the device may be given: physical and virtual address.
#[derive(Clone, Copy)]
struct Region {
    phys: u64,
    va: usize,
    len: usize,
}

impl Region {
    fn holds(&self, va: usize, len: usize) -> bool {
        va >= self.va && va + len <= self.va + self.len
    }
    fn holds_phys(&self, phys: u64, len: usize) -> bool {
        phys >= self.phys && phys + len as u64 <= self.phys + self.len as u64
    }
}

const DMA_REGIONS: usize = 64;
const MMIO_REGIONS: usize = 32;

/// Every run `dma_pages` handed out: a buffer in one needs no bounce.
static DMA: Lock<[Option<Region>; DMA_REGIONS]> = Lock::new([None; DMA_REGIONS]);
/// The PCI BARs mapped for the devices opened so far, for
/// [`Hal::mmio_phys_to_virt`].
static MMIO: Lock<[Option<Region>; MMIO_REGIONS]> = Lock::new([None; MMIO_REGIONS]);

fn remember(table: &Lock<[Option<Region>; DMA_REGIONS]>, r: Region) {
    let mut t = table.lock();
    if let Some(slot) = t.iter_mut().find(|s| s.is_none()) {
        *slot = Some(r);
    }
}

/// `n` zeroed, physically contiguous pages for a device: physical address
/// and pointer. Never freed: the kernel does not take DMA pages back, and
/// a device keeps its until the system goes down.
pub fn dma_pages(n: usize) -> Option<(u64, NonNull<u8>)> {
    let mut phys = 0u64;
    let va = api().dma_alloc(n, &mut phys);
    let va = NonNull::new(va)?;
    if phys == 0 {
        return None;
    }
    unsafe { core::ptr::write_bytes(va.as_ptr(), 0, n * PAGE_SIZE) };
    remember(&DMA, Region { phys, va: va.as_ptr() as usize, len: n * PAGE_SIZE });
    Some((phys, va))
}

/// The physical address of `[va, va+len)` when it lies in DMA pages.
fn dma_phys(va: usize, len: usize) -> Option<u64> {
    DMA.lock().iter().flatten().find(|r| r.holds(va, len)).map(|r| r.phys + (va - r.va) as u64)
}

/// Make `[va, va+len)` consistent between the CPU and the device. The
/// AArch64 `virt` board's DMA memory is cacheable: clean and invalidate
/// its lines around every device access; the other arches are coherent
/// and need no more than the compiler not reordering around it.
pub fn dma_sync(va: usize, len: usize) {
    compiler_fence(Ordering::SeqCst);
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let mut addr = va & !63;
        while addr < va + len {
            core::arch::asm!("dc civac, {x}", x = in(reg) addr, options(nostack));
            addr += 64;
        }
        core::arch::asm!("dsb sy", options(nostack));
    }
    #[cfg(not(target_arch = "aarch64"))]
    let _ = (va, len);
}

/// Buffers the crate shares that are not DMA memory (a request header on
/// the stack, the kernel's block cache pages) go through here: a small
/// slot for a header or a status byte, a large one for data. A request
/// is at most [`MAX_SHARE`] bytes; the drivers cut theirs to that.
mod bounce {
    use super::{BufferDirection, Lock, NonNull, PAGE_SIZE, PhysAddr, Region, dma_pages, dma_sync};

    const SMALL: usize = 512;
    const SMALLS: usize = 32;
    const LARGE: usize = super::MAX_SHARE;
    const LARGES: usize = 8;

    #[derive(Clone, Copy)]
    struct Slot {
        mem: Region,
        busy: bool,
    }

    static SMALL_POOL: Lock<[Option<Slot>; SMALLS]> = Lock::new([None; SMALLS]);
    static LARGE_POOL: Lock<[Option<Slot>; LARGES]> = Lock::new([None; LARGES]);

    /// The small slots come from one run of pages, carved on first use.
    fn fill_small(pool: &mut [Option<Slot>; SMALLS]) -> bool {
        let Some((phys, va)) = dma_pages((SMALLS * SMALL).div_ceil(PAGE_SIZE)) else {
            return false;
        };
        for (i, s) in pool.iter_mut().enumerate() {
            *s = Some(Slot {
                mem: Region { phys: phys + (i * SMALL) as u64, va: va.as_ptr() as usize + i * SMALL, len: SMALL },
                busy: false,
            });
        }
        true
    }

    /// A free slot of `pool`, made on demand (`make` gets its pages). None
    /// when every slot is busy and no more can be made.
    fn claim<const N: usize>(pool: &Lock<[Option<Slot>; N]>, make: impl Fn(&mut [Option<Slot>; N]) -> bool) -> Option<Region> {
        let mut p = pool.lock();
        if let Some(s) = p.iter_mut().flatten().find(|s| !s.busy) {
            s.busy = true;
            return Some(s.mem);
        }
        if p.iter().all(|s| s.is_some()) || !make(&mut p) {
            return None;
        }
        let s = p.iter_mut().flatten().find(|s| !s.busy)?;
        s.busy = true;
        Some(s.mem)
    }

    fn make_large(pool: &mut [Option<Slot>; LARGES]) -> bool {
        let Some(free) = pool.iter_mut().find(|s| s.is_none()) else {
            return false;
        };
        let Some((phys, va)) = dma_pages(LARGE / PAGE_SIZE) else {
            return false;
        };
        *free = Some(Slot { mem: Region { phys, va: va.as_ptr() as usize, len: LARGE }, busy: false });
        true
    }

    /// Copy `buffer` into a slot (unless the device only writes it) and
    /// return the slot's physical address. Waits for a slot when every one
    /// is in use: their holders are requests in flight on other CPUs, and
    /// the device finishes those.
    pub unsafe fn take(buffer: NonNull<[u8]>, dir: BufferDirection) -> PhysAddr {
        let len = buffer.len();
        if len > LARGE {
            super::api().write_str("virtio: a shared buffer is larger than MAX_SHARE\n");
            return 0;
        }
        let slot = loop {
            let got = if len <= SMALL {
                claim(&SMALL_POOL, |p| fill_small(p))
            } else {
                claim(&LARGE_POOL, make_large)
            };
            match got {
                Some(r) => break r,
                None if len <= SMALL && SMALL_POOL.lock().iter().all(|s| s.is_none()) => return 0,
                None if len > SMALL && LARGE_POOL.lock().iter().all(|s| s.is_none()) => return 0,
                None => core::hint::spin_loop(),
            }
        };
        if dir != BufferDirection::DeviceToDriver {
            unsafe { core::ptr::copy_nonoverlapping(buffer.as_ptr() as *const u8, slot.va as *mut u8, len) };
        }
        dma_sync(slot.va, len);
        slot.phys
    }

    fn find(paddr: PhysAddr) -> Option<(bool, Region)> {
        let small = SMALL_POOL.lock().iter().flatten().find(|s| s.mem.phys == paddr).map(|s| (true, s.mem));
        small.or_else(|| LARGE_POOL.lock().iter().flatten().find(|s| s.mem.phys == paddr).map(|s| (false, s.mem)))
    }

    /// Whether `paddr` is a slot of the pools (one `take` handed out).
    pub fn is_slot(paddr: PhysAddr) -> bool {
        find(paddr).is_some()
    }

    /// The device is done with the slot at `paddr`: copy what it wrote
    /// back into `buffer` and free the slot.
    pub unsafe fn give_back(paddr: PhysAddr, buffer: NonNull<[u8]>, dir: BufferDirection) {
        let Some((small, mem)) = find(paddr) else {
            return;
        };
        let len = buffer.len().min(mem.len);
        dma_sync(mem.va, len);
        if dir != BufferDirection::DriverToDevice {
            unsafe { core::ptr::copy_nonoverlapping(mem.va as *const u8, buffer.as_ptr() as *mut u8, len) };
        }
        let release = |slots: &mut [Option<Slot>]| {
            if let Some(s) = slots.iter_mut().flatten().find(|s| s.mem.phys == paddr) {
                s.busy = false;
            }
        };
        if small {
            release(&mut SMALL_POOL.lock()[..]);
        } else {
            release(&mut LARGE_POOL.lock()[..]);
        }
    }
}

/// The most a driver may hand the device in one buffer that is not DMA
/// memory (a bounce slot's size): the drivers cut a longer request.
pub const MAX_SHARE: usize = 64 * 1024;

/// The crate's `Hal` over the module ABI.
pub struct MyosHal;

unsafe impl Hal for MyosHal {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        match dma_pages(pages) {
            Some((phys, va)) => (phys, va),
            None => (0, NonNull::dangling()),
        }
    }

    /// Nothing gives DMA pages back (`dma_pages`): a device that fails to
    /// come up leaves its queue pages behind.
    unsafe fn dma_dealloc(_paddr: PhysAddr, _vaddr: NonNull<u8>, _pages: usize) -> i32 {
        0
    }

    /// A BAR of a function [`probe_pci`] opened, mapped by the kernel.
    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, size: usize) -> NonNull<u8> {
        let hit = MMIO.lock().iter().flatten().find(|r| r.holds_phys(paddr, size)).map(|r| r.va + (paddr - r.phys) as usize);
        match hit.and_then(|va| NonNull::new(va as *mut u8)) {
            Some(p) => p,
            None => {
                api().write_str("virtio: a BAR region the kernel did not map\n");
                NonNull::dangling()
            }
        }
    }

    unsafe fn share(buffer: NonNull<[u8]>, direction: BufferDirection) -> PhysAddr {
        let va = buffer.as_ptr() as *const u8 as usize;
        match dma_phys(va, buffer.len()) {
            Some(phys) => {
                dma_sync(va, buffer.len());
                phys
            }
            None => unsafe { bounce::take(buffer, direction) },
        }
    }

    unsafe fn unshare(paddr: PhysAddr, buffer: NonNull<[u8]>, direction: BufferDirection) {
        if bounce::is_slot(paddr) {
            unsafe { bounce::give_back(paddr, buffer, direction) };
        } else {
            dma_sync(buffer.as_ptr() as *const u8 as usize, buffer.len());
        }
    }
}

// ---- PCI ------------------------------------------------------------------

/// The crate's view of PCI configuration space: the kernel's accessors.
pub struct KernelCam;

impl ConfigurationAccess for KernelCam {
    fn read_word(&self, df: DeviceFunction, off: u8) -> u32 {
        api().pci_cfg_read32(df.bus, df.device, df.function, off)
    }
    fn write_word(&mut self, df: DeviceFunction, off: u8, data: u32) {
        api().pci_cfg_write32(df.bus, df.device, df.function, off, data)
    }
    unsafe fn unsafe_clone(&self) -> Self {
        KernelCam
    }
}

fn root() -> PciRoot<KernelCam> {
    PciRoot::new(KernelCam)
}

fn df_of(bus: u8, slot: u8, func: u8) -> DeviceFunction {
    DeviceFunction { bus, device: slot, function: func }
}

/// The PCI device ids a virtio device of `kind` comes as: modern, and the
/// transitional one of the kinds that have it.
fn pci_ids(kind: DeviceType) -> [Option<u16>; 2] {
    let transitional = match kind {
        DeviceType::Network => Some(0x1000),
        DeviceType::Block => Some(0x1001),
        DeviceType::Console => Some(0x1003),
        DeviceType::ScsiHost => Some(0x1004),
        DeviceType::EntropySource => Some(0x1005),
        _ => None,
    };
    [Some(0x1040 + kind as u16), transitional]
}

/// Map the function's memory BARs (the virtio structures live in them)
/// for [`Hal::mmio_phys_to_virt`].
fn map_bars(root: &mut PciRoot<KernelCam>, df: DeviceFunction) {
    let mut bar = 0u8;
    while bar < 6 {
        let Ok(Some(info)) = root.bar_info(df, bar) else {
            bar += 1;
            continue;
        };
        let two = info.takes_two_entries();
        if let Some((phys, _)) = info.memory_address_size() {
            let (mut va, mut size) = (0usize, 0u64);
            if api().pci_bar_map(df.bus, df.device, df.function, bar, &mut va, &mut size) == 0 && va != 0 {
                remember_mmio(Region { phys, va, len: size as usize });
            }
        }
        bar += if two { 2 } else { 1 };
    }
}

fn remember_mmio(r: Region) {
    let mut t = MMIO.lock();
    if t.iter().flatten().any(|m| m.phys == r.phys) {
        return;
    }
    if let Some(slot) = t.iter_mut().find(|s| s.is_none()) {
        *slot = Some(r);
    }
}

/// Every virtio PCI function of `kind` that is not `known`, opened as a
/// modern transport (a transitional device with the modern capabilities
/// too), for `found`. The function is enabled (memory, bus master) first.
pub fn probe_pci(kind: DeviceType, known: &mut dyn FnMut(Id) -> bool, found: &mut dyn FnMut(Id, Device)) {
    let api = api();
    for id in pci_ids(kind).into_iter().flatten() {
        for i in 0..16u32 {
            let (mut bus, mut slot, mut func) = (0u8, 0u8, 0u8);
            if api.pci_find(VIRTIO_VENDOR_ID, id, i, &mut bus, &mut slot, &mut func) != 0 {
                break;
            }
            let dev = Id::Pci { bus, slot, func };
            if known(dev) {
                continue;
            }
            api.pci_enable(bus, slot, func);
            let mut root = root();
            let df = df_of(bus, slot, func);
            map_bars(&mut root, df);
            // A transport of the wrong kind cannot happen (the ids above
            // are the kind's); one that fails to open was never a device.
            match PciTransport::new::<MyosHal, _>(&mut root, df) {
                Ok(t) if t.device_type() == kind => found(dev, SomeTransport::Pci(t)),
                _ => api.write_str("virtio: a PCI function without the modern layout, skipped\n"),
            }
        }
    }
}

/// `MagicValue` and `DeviceID` of a virtio-mmio window.
const MMIO_MAGIC: usize = 0x000;
const MMIO_DEVICE_ID: usize = 0x008;
const MMIO_MAGIC_VALUE: u32 = 0x7472_6976; // "virt"

/// Every `virtio,mmio` node of the device tree holding a device of `kind`
/// that is not `known`, for `found`. None on an arch without a tree. The
/// kind is read from the window before a transport is opened on it: a
/// transport resets its device when dropped, and the other nodes are other
/// modules' running devices.
pub fn probe_mmio(kind: DeviceType, known: &mut dyn FnMut(Id) -> bool, found: &mut dyn FnMut(Id, Device)) {
    let api = api();
    for i in 0.. {
        let mut node = MmioDevice::default();
        if api.dt_mmio_find("virtio,mmio", i, &mut node) != 0 {
            break;
        }
        let dev = Id::Mmio { base: node.base, phys: node.phys, irq: node.irq };
        if known(dev) || node.base == 0 || node.size < MMIO_DEVICE_ID + 4 {
            continue;
        }
        // SAFETY: the kernel mapped `size` bytes of the node's registers at `base`.
        let (magic, id) = unsafe {
            (
                core::ptr::read_volatile((node.base + MMIO_MAGIC) as *const u32),
                core::ptr::read_volatile((node.base + MMIO_DEVICE_ID) as *const u32),
            )
        };
        if magic != MMIO_MAGIC_VALUE || id != kind as u32 {
            continue;
        }
        let Some(header) = NonNull::new(node.base as *mut VirtIOHeader) else {
            continue;
        };
        // SAFETY: the kernel mapped `size` bytes of the node's registers at `base`.
        match unsafe { MmioTransport::new(header, node.size) } {
            Ok(t) if t.device_type() == kind => found(dev, SomeTransport::Mmio(t)),
            _ => {}
        }
    }
}

/// Both transports: PCI, then the device tree.
pub fn probe(kind: DeviceType, known: &mut dyn FnMut(Id) -> bool, found: &mut dyn FnMut(Id, Device)) {
    probe_pci(kind, known, found);
    probe_mmio(kind, known, found);
}

// ---- interrupts -----------------------------------------------------------

/// A virtio-pci capability structure of the function: the kernel virtual
/// address of its `offset` bytes into its BAR.
fn pci_cap(df: DeviceFunction, cfg_type: u8) -> Option<usize> {
    const PCI_CAP_VNDR: u8 = 9;
    let mut root = root();
    let caps: [Option<(u8, u16)>; 16] = {
        let mut out = [None; 16];
        for (slot, cap) in root.capabilities(df).filter(|c| c.id == PCI_CAP_VNDR).take(16).enumerate() {
            out[slot] = Some((cap.offset, cap.private_header));
        }
        out
    };
    for (offset, private_header) in caps.into_iter().flatten() {
        let cap = CapabilityInfoLite { offset, private_header };
        if (cap.private_header >> 8) as u8 != cfg_type {
            continue;
        }
        let bar = KernelCam.read_word(df, cap.offset + 4) as u8;
        let off = KernelCam.read_word(df, cap.offset + 8) as u64;
        let (phys, size) = root.bar_info(df, bar).ok()??.memory_address_size()?;
        if off >= size {
            return None;
        }
        // SAFETY: a mapped BAR; a miss returns a dangling pointer, caught below.
        let va = unsafe { MyosHal::mmio_phys_to_virt(phys + off, 1) }.as_ptr() as usize;
        return (va != NonNull::<u8>::dangling().as_ptr() as usize).then_some(va);
    }
    None
}

/// The two words of a capability `PciRoot::capabilities` yields, kept
/// past the iterator (which borrows the root `bar_info` needs mutably).
struct CapabilityInfoLite {
    offset: u8,
    private_header: u16,
}

const CAP_COMMON: u8 = 1;
const CAP_ISR: u8 = 3;
/// `common_cfg.queue_select` and `queue_msix_vector` (virtio 1.x, 4.1.4.3).
const COMMON_QUEUE_SELECT: usize = 22;
const COMMON_QUEUE_MSIX_VECTOR: usize = 26;

/// How the device's interrupt reaches a handler: the MSI-X table entry a
/// queue has to be pointed at ([`set_queue_msix_vector`]), or a line
/// (INTx, a board interrupt) the handler acknowledges with [`IrqAck`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IrqRoute {
    Msix(u16),
    Line,
}

/// Route the device's interrupt to `handler(ctx)`: a PCI function through
/// `pci_irq_enable` (MSI-X where the kernel has it), a virtio-mmio device
/// through its device-tree interrupt. None when it cannot be routed: the
/// driver polls then.
pub fn irq_enable(id: Id, name: &str, handler: myos_abi::IrqHandler, ctx: *mut core::ffi::c_void) -> Option<IrqRoute> {
    let api = api();
    match id {
        Id::Pci { bus, slot, func } => {
            let mut entry = myos_abi::MYOS_IRQ_INTX;
            if api.pci_irq_enable(bus, slot, func, name, handler, ctx, &mut entry) != 0 {
                return None;
            }
            // Plain MSI (a function without MSI-X) has no vector to name:
            // the device signals as on its line, through its ISR.
            let line = entry == myos_abi::MYOS_IRQ_INTX || entry == myos_abi::MYOS_IRQ_MSI;
            Some(if line { IrqRoute::Line } else { IrqRoute::Msix(entry) })
        }
        Id::Mmio { irq, .. } => (irq != 0 && api.irq_enable(irq, name, handler, ctx) == 0).then_some(IrqRoute::Line),
    }
}

/// Point `queue` of a PCI device at MSI-X table entry `vector` (what the
/// crate's transport leaves at "no vector"). A device set up by the crate
/// is already running (`DRIVER_OK`); the device takes the vector from
/// then on. True when the device kept it.
pub fn set_queue_msix_vector(id: Id, queue: u16, vector: u16) -> bool {
    let Id::Pci { bus, slot, func } = id else {
        return false;
    };
    let Some(common) = pci_cap(df_of(bus, slot, func), CAP_COMMON) else {
        return false;
    };
    unsafe {
        core::ptr::write_volatile((common + COMMON_QUEUE_SELECT) as *mut u16, queue);
        core::ptr::write_volatile((common + COMMON_QUEUE_MSIX_VECTOR) as *mut u16, vector);
        core::ptr::read_volatile((common + COMMON_QUEUE_MSIX_VECTOR) as *const u16) == vector
    }
}

/// What an interrupt handler reads to make the device drop its line:
/// the ISR status of a PCI function, or a virtio-mmio device's
/// `InterruptStatus`, written back to `InterruptACK`. Atomics-friendly: a
/// `usize` the handler can keep in one, 0 for "nothing to acknowledge".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IrqAck {
    None,
    PciIsr(usize),
    Mmio(usize),
}

const MMIO_INTERRUPT_STATUS: usize = 0x60;
const MMIO_INTERRUPT_ACK: usize = 0x64;

impl IrqAck {
    pub fn of(id: Id) -> IrqAck {
        match id {
            Id::Pci { bus, slot, func } => pci_cap(df_of(bus, slot, func), CAP_ISR).map_or(IrqAck::None, IrqAck::PciIsr),
            Id::Mmio { base, .. } => IrqAck::Mmio(base),
        }
    }

    /// Acknowledge the device's interrupt. Safe from interrupt context.
    pub fn ack(self) {
        unsafe {
            match self {
                IrqAck::None => {}
                IrqAck::PciIsr(isr) => {
                    let _ = core::ptr::read_volatile(isr as *const u8);
                }
                IrqAck::Mmio(base) => {
                    let status = core::ptr::read_volatile((base + MMIO_INTERRUPT_STATUS) as *const u32);
                    if status != 0 {
                        core::ptr::write_volatile((base + MMIO_INTERRUPT_ACK) as *mut u32, status);
                    }
                }
            }
        }
    }

    /// Pack for an atomic: the kind in the low two bits of a page-aligned
    /// (or at least 4-aligned) address.
    pub fn to_usize(self) -> usize {
        match self {
            IrqAck::None => 0,
            IrqAck::PciIsr(a) => a | 1,
            IrqAck::Mmio(a) => a | 2,
        }
    }

    pub fn from_usize(v: usize) -> IrqAck {
        match v & 3 {
            1 => IrqAck::PciIsr(v & !3),
            2 => IrqAck::Mmio(v & !3),
            _ => IrqAck::None,
        }
    }
}
