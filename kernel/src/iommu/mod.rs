//! The IOMMU: what a device's DMA may reach (issue #501). A device a
//! kernel module drives sees all of memory as before (passthrough: the
//! identity domain). A function a program drives through `/dev/pci`
//! ([`attach`]) gets a domain of its own, holding the DMA buffers the
//! driver asked for ([`map`]) and nothing else, each at its physical
//! address, so the numbers `ctl` shows stay the device's: a DMA (or an MSI
//! write) anywhere else is refused by the hardware and recorded
//! ([`record`], `/proc/iommu`). The driver's device cannot read or write
//! kernel memory or another process's pages, which without an IOMMU any
//! bus-mastering device can.
//!
//! The units are the platform's: Intel VT-d on x86_64 (`vtd`, the ACPI
//! DMAR table), Arm's SMMUv3 on aarch64 (`smmuv3`, the IORT or the device
//! tree through `platform`), the RISC-V IOMMU on riscv64 (`riscv`, a PCI
//! function of its own class); a board without one leaves `/dev/pci`
//! unconfined, which `/proc/iommu` says (`none`). Kernel drivers need
//! nothing from here.

use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};

use spin::Mutex;

#[cfg(target_arch = "x86_64")]
mod vtd;
#[cfg(target_arch = "x86_64")]
use vtd as backend;
#[cfg(target_arch = "aarch64")]
mod smmuv3;
#[cfg(target_arch = "aarch64")]
use smmuv3 as backend;
#[cfg(target_arch = "riscv64")]
mod riscv;
#[cfg(target_arch = "riscv64")]
use riscv as backend;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64")))]
mod none;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64")))]
use none as backend;

/// A DMA the IOMMU refused.
#[derive(Clone, Copy)]
pub struct Fault {
    /// The requester: `bus << 8 | slot << 3 | func`.
    pub sid: u16,
    pub addr: u64,
    /// The unit's reason code (VT-d: 0x5 a write to no mapping, 0x6 a
    /// read; SMMUv3: the event type, 0x10 a translation fault; RISC-V:
    /// the cause, 21 a read's guest-page fault, 23 a write's).
    pub reason: u8,
    pub read: bool,
}

static FAULTS: AtomicU64 = AtomicU64::new(0);
static LAST: Mutex<Option<Fault>> = Mutex::new(None);

/// A unit's fault handler reports a refused DMA: counted, the last one kept
/// for `/proc/iommu`. From interrupt context: the record is skipped, not
/// waited for, when a reader holds it.
#[cfg_attr(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64")), allow(dead_code))]
pub(crate) fn record(f: Fault) {
    FAULTS.fetch_add(1, Ordering::Relaxed);
    if let Some(mut last) = LAST.try_lock() {
        *last = Some(f);
    }
}

/// How many DMAs the units refused since boot.
pub fn faults() -> u64 {
    FAULTS.load(Ordering::Relaxed)
}

/// Find the platform's units and put every device in passthrough. After
/// the interrupt controllers (the units' fault interrupt), before the
/// drivers (their devices must not meet a half-programmed unit).
pub fn init() {
    backend::init();
}

/// Confine `bus:slot.func` to a domain of its own, empty: its id, or `None`
/// when no unit translates for the function (its DMA stays unconfined).
pub fn attach(bus: u8, slot: u8, func: u8) -> Option<u32> {
    backend::attach(bus, slot, func)
}

/// Let the devices of domain `domain` reach `[phys, phys + len)`, at that
/// address (whole pages). False when the domain is not one [`attach`]
/// gave, or memory ran out.
pub fn map(domain: u32, phys: u64, len: usize) -> bool {
    backend::map(domain, phys, len)
}

/// The function back in passthrough, its domain gone.
pub fn detach(bus: u8, slot: u8, func: u8) {
    backend::detach(bus, slot, func)
}

/// `/proc/iommu`: the units, the domains and what they map, the refused
/// DMAs (`faults N`, and the last one).
pub fn text() -> String {
    use core::fmt::Write;
    let mut s = backend::text();
    let _ = writeln!(s, "faults {}", faults());
    if let Some(f) = *LAST.lock() {
        let _ = writeln!(
            s,
            "last {:02x}:{:02x}.{} {} 0x{:x} reason 0x{:x}",
            f.sid >> 8,
            (f.sid >> 3) & 0x1f,
            f.sid & 7,
            if f.read { "read" } else { "write" },
            f.addr,
            f.reason
        );
    }
    s
}
