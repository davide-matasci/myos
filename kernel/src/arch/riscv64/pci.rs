//! ECAM config access and BAR mapping, with the host bridge's ECAM window,
//! bus range and 64-bit MMIO window from the device tree (`set_host`; QEMU
//! `virt`: ECAM 0x30000000 for 256 buses in the identity-mapped low 1 GiB,
//! BARs assigned from the 64-bit window at 0x4_0000_0000 because the
//! 32-bit one is user address space). BAR pages are mapped when the scan
//! finds them.

use core::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};

use super::paging;
use crate::platform::{PciHost, PciWindow};

static ECAM_BASE: AtomicUsize = AtomicUsize::new(0);
static BUS_END: AtomicU8 = AtomicU8::new(0);
static MMIO_CPU: AtomicU64 = AtomicU64::new(0);
static MMIO_PCI: AtomicU64 = AtomicU64::new(0);
static MMIO_SIZE: AtomicU64 = AtomicU64::new(0);

/// The PCIe host bridge from the device tree: ECAM window (mapped here),
/// last bus, and the MMIO window BARs are assigned from.
pub fn set_host(host: PciHost, mmio: PciWindow) {
    let va = map_mmio(host.base, host.size.max(1)).unwrap_or(host.base as usize);
    ECAM_BASE.store(va, Ordering::SeqCst);
    // One bus takes 1 MiB of ECAM: never read past the window.
    let by_size = (host.size >> 20).saturating_sub(1).min(255) as u8;
    BUS_END.store(host.bus_end.min(by_size), Ordering::SeqCst);
    MMIO_CPU.store(mmio.cpu, Ordering::SeqCst);
    MMIO_PCI.store(mmio.pci, Ordering::SeqCst);
    MMIO_SIZE.store(mmio.size, Ordering::SeqCst);
}

/// The host bridge's I/O window (`set_io`): mapped at [`IO_VA`], bus
/// addresses `[IO_PCI, IO_PCI + IO_SIZE)`; [`IO_NEXT`] the next bus
/// address [`io_assign`] hands out.
static IO_VA: AtomicUsize = AtomicUsize::new(0);
static IO_PCI: AtomicU64 = AtomicU64::new(0);
static IO_SIZE: AtomicU64 = AtomicU64::new(0);
static IO_NEXT: AtomicU64 = AtomicU64::new(0);

/// The host bridge's I/O window from the device tree, mapped: PCI I/O
/// space is memory here (`pci::io_map`).
pub fn set_io(io: PciWindow) {
    let Some(va) = map_mmio(io.cpu, io.size.max(1)) else {
        return;
    };
    IO_PCI.store(io.pci, Ordering::SeqCst);
    IO_SIZE.store(io.size, Ordering::SeqCst);
    // Port 0 means unassigned to a BAR: hand out from 0x1000 on, as PCs do.
    IO_NEXT.store(io.pci.max(0x1000), Ordering::SeqCst);
    IO_VA.store(va, Ordering::SeqCst);
}

/// Where an I/O BAR at bus address `addr` is reached: its place in the
/// mapped I/O window.
pub fn io_base(addr: u32, size: u32) -> Option<usize> {
    let (va, pci, len) = (IO_VA.load(Ordering::Relaxed), IO_PCI.load(Ordering::Relaxed), IO_SIZE.load(Ordering::Relaxed));
    let (addr, size) = (u64::from(addr), u64::from(size));
    (va != 0 && addr >= pci && addr + size <= pci + len).then(|| va + (addr - pci) as usize)
}

/// A bus address in the I/O window for a BAR of `size` bytes the firmware
/// left unassigned (aligned to its size), or `None` when the window is
/// full or missing.
pub fn io_assign(size: u32) -> Option<u32> {
    let (pci, len) = (IO_PCI.load(Ordering::Relaxed), IO_SIZE.load(Ordering::Relaxed));
    let size = u64::from(size);
    let mut next = IO_NEXT.load(Ordering::Relaxed);
    loop {
        let at = (next + size - 1) & !(size - 1);
        if IO_VA.load(Ordering::Relaxed) == 0 || at + size > pci + len || at + size > u64::from(u32::MAX) {
            return None;
        }
        match IO_NEXT.compare_exchange(next, at + size, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => return Some(at as u32),
            Err(now) => next = now,
        }
    }
}

/// Last bus the ECAM window covers.
pub fn max_bus() -> u8 {
    BUS_END.load(Ordering::Relaxed)
}

/// Where BARs are assigned (the 64-bit MMIO window), as a bus address.
pub fn mmio_assign() -> u64 {
    MMIO_PCI.load(Ordering::Relaxed)
}

/// The CPU address of bus address `addr` (a BAR's): moved by the MMIO
/// window it is in, one-to-one outside it.
pub fn bus_to_cpu(addr: u64) -> u64 {
    let (cpu, pci, size) =
        (MMIO_CPU.load(Ordering::Relaxed), MMIO_PCI.load(Ordering::Relaxed), MMIO_SIZE.load(Ordering::Relaxed));
    if addr >= pci && addr - pci < size { addr - pci + cpu } else { addr }
}

fn ecam(bus: u8, slot: u8, func: u8, offset: u8) -> usize {
    ECAM_BASE.load(Ordering::Relaxed)
        + ((bus as usize) << 20)
        + ((slot as usize) << 15)
        + ((func as usize) << 12)
        + (offset as usize & !3)
}

pub fn cfg_read32(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    if bus > max_bus() || ECAM_BASE.load(Ordering::Relaxed) == 0 {
        return 0xFFFF_FFFF;
    }
    unsafe { core::ptr::read_volatile(ecam(bus, slot, func, offset) as *const u32) }
}

pub fn cfg_write32(bus: u8, slot: u8, func: u8, offset: u8, value: u32) {
    if bus > max_bus() || ECAM_BASE.load(Ordering::Relaxed) == 0 {
        return;
    }
    unsafe { core::ptr::write_volatile(ecam(bus, slot, func, offset) as *mut u32, value) }
}

pub fn map_mmio(phys: u64, size: u64) -> Option<usize> {
    paging::map_mmio(phys, size)
}
