//! PCI configuration access and BAR mapping, for the host bridge the
//! platform description names (`set_host`), mapped into the kernel's
//! device window (`paging`):
//!
//! - a generic ECAM one (QEMU `virt` with `highmem-ecam=off`: ECAM
//!   0x3f000000 for 16 buses, MMIO at 0x10000000);
//! - a BCM2711's (Raspberry Pi 4), whose registers show the root port's
//!   configuration space at offset 0 and one other function's at a time
//!   through a window ([`BCM_CFG_DATA`]), chosen in [`BCM_CFG_INDEX`]. The
//!   bridge is used as the UEFI firmware left it: its link trained, its
//!   windows set, the root port's bus numbers assigned and the firmware of
//!   the VL805 USB controller behind it loaded. Functions other than the
//!   root port on bus 0, and other than slot 0 on bus 1 (the link's one
//!   device), are never addressed: on bus 0 that hangs the memory
//!   controller.
//!
//! BARs are assigned from the 32-bit MMIO window, which the bridge may move:
//! a BAR holds a bus address, mapped at its CPU address ([`bus_to_cpu`]).

use core::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};

use spin::Mutex;

use super::paging;
use crate::platform::{PciHost, PciHostKind, PciWindow};

/// The ECAM window, or the BCM2711's registers (kernel virtual).
static BASE: AtomicUsize = AtomicUsize::new(0);
/// [`KIND_ECAM`] or [`KIND_BCM2711`].
static KIND: AtomicU8 = AtomicU8::new(KIND_ECAM);
const KIND_ECAM: u8 = 0;
const KIND_BCM2711: u8 = 1;
static BUS_END: AtomicU8 = AtomicU8::new(0);
static MMIO_CPU: AtomicU64 = AtomicU64::new(0);
static MMIO_PCI: AtomicU64 = AtomicU64::new(0);
static MMIO_SIZE: AtomicU64 = AtomicU64::new(0);
/// The BCM2711 shows one function at a time: index and access go together.
static BCM_CFG: Mutex<()> = Mutex::new(());

/// BCM2711: the function [`BCM_CFG_DATA`] shows, as `bus << 20 | slot << 15
/// | func << 12`.
const BCM_CFG_INDEX: usize = 0x9000;
/// BCM2711: the configuration space of the function [`BCM_CFG_INDEX`] names.
const BCM_CFG_DATA: usize = 0x8000;
/// BCM2711: bridge state; bits 4 and 5, the link's physical and data link
/// layers up.
const BCM_STATE: usize = 0x4068;
const BCM_LINK_UP: u32 = 0x30;

/// The PCIe host bridge from the platform description (mapped here), and
/// the 32-bit MMIO window BARs are assigned from.
pub fn set_host(host: PciHost, mmio: PciWindow) {
    // Unmapped (beyond the device window): no bus is scanned.
    let Some(va) = map_mmio(host.base, host.size.max(1)) else {
        return;
    };
    MMIO_CPU.store(mmio.cpu, Ordering::SeqCst);
    MMIO_PCI.store(mmio.pci, Ordering::SeqCst);
    MMIO_SIZE.store(mmio.size, Ordering::SeqCst);
    match host.kind {
        PciHostKind::Ecam => {
            // One bus takes 1 MiB of ECAM: never read past the window.
            let by_size = (host.size >> 20).saturating_sub(1).min(255) as u8;
            BUS_END.store(host.bus_end.min(by_size), Ordering::SeqCst);
            KIND.store(KIND_ECAM, Ordering::SeqCst);
            BASE.store(va, Ordering::SeqCst);
        }
        PciHostKind::Bcm2711 => {
            KIND.store(KIND_BCM2711, Ordering::SeqCst);
            BASE.store(va, Ordering::SeqCst);
            // Without a link only the root port answers. With one, the buses
            // the firmware gave the root port (its subordinate bus number).
            let link_up = read32(va + BCM_STATE) & BCM_LINK_UP == BCM_LINK_UP;
            let subordinate = (read32(va + 0x18) >> 16) as u8;
            let end = if link_up { subordinate.min(host.bus_end) } else { 0 };
            BUS_END.store(end, Ordering::SeqCst);
        }
    }
}

/// Last bus there is to scan.
pub fn max_bus() -> u8 {
    BUS_END.load(Ordering::Relaxed)
}

/// Where BARs are assigned (the 32-bit MMIO window), as a bus address.
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

fn read32(va: usize) -> u32 {
    unsafe { core::ptr::read_volatile(va as *const u32) }
}

fn write32(va: usize, value: u32) {
    unsafe { core::ptr::write_volatile(va as *mut u32, value) }
}

/// Run `access` on the configuration register `offset` of `bus:slot.func`:
/// `None` when there is no such function to address.
fn with_cfg<R>(bus: u8, slot: u8, func: u8, offset: u8, access: impl FnOnce(usize) -> R) -> Option<R> {
    let base = BASE.load(Ordering::Relaxed);
    if bus > max_bus() || base == 0 {
        return None;
    }
    let offset = offset as usize & !3;
    if KIND.load(Ordering::Relaxed) == KIND_ECAM {
        let fn_base = base + ((bus as usize) << 20) + ((slot as usize) << 15) + ((func as usize) << 12);
        return Some(access(fn_base + offset));
    }
    match (bus, slot, func) {
        (0, 0, 0) => Some(access(base + offset)),
        (0 | 1, 1.., _) | (0, _, 1..) => None,
        _ => {
            let _shown = BCM_CFG.lock();
            write32(base + BCM_CFG_INDEX, (bus as u32) << 20 | (slot as u32) << 15 | (func as u32) << 12);
            Some(access(base + BCM_CFG_DATA + offset))
        }
    }
}

pub fn cfg_read32(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    with_cfg(bus, slot, func, offset, read32).unwrap_or(0xFFFF_FFFF)
}

pub fn cfg_write32(bus: u8, slot: u8, func: u8, offset: u8, value: u32) {
    with_cfg(bus, slot, func, offset, |va| write32(va, value));
}

pub fn map_mmio(phys: u64, size: u64) -> Option<usize> {
    paging::map_mmio(phys, size)
}
