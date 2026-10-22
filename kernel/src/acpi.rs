//! The ACPI static tables, from the RSDP Limine hands over: the platform
//! description on x86_64 and on aarch64 firmware that has one (EDK2). Only
//! the tables that name the platform are read, and no AML: MADT (the CPUs
//! and the interrupt controller), MCFG (PCIe ECAM), SPCR (the console
//! UART), GTDT (the arm timers), the FADT's arm boot flags (PSCI), DMAR
//! (the VT-d units, x86_64), IORT (the SMMUv3 and the PCI functions it
//! translates for, aarch64).
//! `modules/acpi` keeps `/proc/acpi` and the ACPI power methods (`_S5`
//! power-off, the reset register: `docs/power.md`). Everything here runs before the heap exists, so nothing
//! allocates; tables are read in place through the HHDM.

use crate::limine_boot;

/// No table is longer than this (a corrupt length is not followed).
const TABLE_MAX: usize = 1 << 20;

/// A table, header included, read in place.
#[derive(Clone, Copy)]
pub struct Table {
    bytes: &'static [u8],
}

impl Table {
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn sig(&self) -> [u8; 4] {
        [self.bytes[0], self.bytes[1], self.bytes[2], self.bytes[3]]
    }

    fn u8(&self, off: usize) -> Option<u8> {
        self.bytes.get(off).copied()
    }

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    fn u16(&self, off: usize) -> Option<u16> {
        let b = self.bytes.get(off..off + 2)?;
        Some(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&self, off: usize) -> Option<u32> {
        let b = self.bytes.get(off..off + 4)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&self, off: usize) -> Option<u64> {
        let b = self.bytes.get(off..off + 8)?;
        Some(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }
}

/// `len` bytes at physical `phys`, through the HHDM.
unsafe fn bytes(phys: u64, len: usize) -> Option<&'static [u8]> {
    if phys == 0 || len == 0 || len > TABLE_MAX {
        return None;
    }
    let va = (phys + limine_boot::hhdm_offset()) as *const u8;
    Some(unsafe { core::slice::from_raw_parts(va, len) })
}

/// The table at `phys`, as long as its header says.
fn table_at(phys: u64) -> Option<Table> {
    let hdr = unsafe { bytes(phys, 36)? };
    let len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
    if len < 36 {
        return None;
    }
    Some(Table { bytes: unsafe { bytes(phys, len)? } })
}

/// The root table (the XSDT, or the RSDT of an ACPI 1.0 RSDP) and the size
/// of its entries.
fn root() -> Option<(Table, usize)> {
    let va = limine_boot::rsdp_va()?;
    let rsdp = unsafe { core::slice::from_raw_parts(va as *const u8, 20) };
    if &rsdp[..8] != b"RSD PTR " {
        return None;
    }
    if rsdp[15] >= 2 {
        let rsdp = unsafe { core::slice::from_raw_parts(va as *const u8, 36) };
        let xsdt = u64::from_le_bytes(rsdp[24..32].try_into().ok()?);
        if let Some(t) = table_at(xsdt) {
            return Some((t, 8));
        }
    }
    let rsdt = u32::from_le_bytes(rsdp[16..20].try_into().ok()?) as u64;
    Some((table_at(rsdt)?, 4))
}

/// The firmware has ACPI tables.
pub fn present() -> bool {
    root().is_some()
}

/// The root table's OEM ID (`BOCHS ` on QEMU), without trailing spaces.
pub fn oem_id() -> Option<([u8; 6], usize)> {
    let (root, _) = root()?;
    let b = root.bytes.get(10..16)?;
    let mut id = [0u8; 6];
    id.copy_from_slice(b);
    let len = id.iter().rposition(|&c| c != b' ' && c != 0).map_or(0, |i| i + 1);
    Some((id, len))
}

/// The first table with signature `sig`.
pub fn find(sig: &[u8; 4]) -> Option<Table> {
    let (root, entry) = root()?;
    let n = (root.len() - 36) / entry;
    for i in 0..n {
        let off = 36 + i * entry;
        let phys = if entry == 8 { root.u64(off)? } else { root.u32(off)? as u64 };
        let Some(t) = table_at(phys) else {
            continue;
        };
        if t.sig() == *sig {
            return Some(t);
        }
    }
    None
}

/// What the MADT says about the CPUs and the interrupt controller.
#[derive(Clone, Copy, Default)]
pub struct Madt {
    /// Enabled processors (local APIC, x2APIC or GICC entries).
    pub cpus: usize,
    /// The local APIC's physical address (x86_64).
    pub lapic: u64,
    /// The GIC distributor's base and the GIC version it reports (0 when
    /// the firmware did not say).
    pub gicd: Option<(u64, u8)>,
    /// The first GICC entry's CPU interface base (GICv2).
    pub gicc: Option<u64>,
    /// The GICv3 redistributor discovery range: base and length.
    pub gicr: Option<(u64, u64)>,
    /// The GIC ITS base.
    pub its: Option<u64>,
    /// The first I/O APIC (x86_64): its physical address and the first
    /// global system interrupt (GSI) it serves.
    pub ioapic: Option<(u64, u32)>,
}

pub fn madt() -> Option<Madt> {
    let t = find(b"APIC")?;
    let mut m = Madt { lapic: t.u32(36)? as u64, ..Madt::default() };
    let mut off = 44;
    while off + 2 <= t.len() {
        let typ = t.u8(off)?;
        let len = t.u8(off + 1)? as usize;
        if len < 2 || off + len > t.len() {
            break;
        }
        match typ {
            // Local APIC: flags at 4, enabled is bit 0.
            0 if len >= 8 => {
                if t.u32(off + 4)? & 1 != 0 {
                    m.cpus += 1;
                }
            }
            // I/O APIC: address at 4, GSI base at 8.
            1 if len >= 12 => {
                if m.ioapic.is_none() {
                    m.ioapic = Some((t.u32(off + 4)? as u64, t.u32(off + 8)?));
                }
            }
            // Local APIC address override.
            5 if len >= 12 => m.lapic = t.u64(off + 4)?,
            // x2APIC: flags at 8.
            9 if len >= 16 => {
                if t.u32(off + 8)? & 1 != 0 {
                    m.cpus += 1;
                }
            }
            // GICC: flags at 12, the CPU interface's physical base at 32.
            0xB if len >= 40 => {
                if t.u32(off + 12)? & 1 != 0 {
                    m.cpus += 1;
                }
                let base = t.u64(off + 32)?;
                if m.gicc.is_none() && base != 0 {
                    m.gicc = Some(base);
                }
            }
            // GICD: physical base at 8, GIC version at 20.
            0xC if len >= 24 => m.gicd = Some((t.u64(off + 8)?, t.u8(off + 20)?)),
            // GICR: discovery range base at 4, length at 12.
            0xE if len >= 16 => m.gicr = Some((t.u64(off + 4)?, t.u32(off + 12)? as u64)),
            // GIC ITS: physical base at 8.
            0xF if len >= 20 => m.its = Some(t.u64(off + 8)?),
            _ => {}
        }
        off += len;
    }
    Some(m)
}

/// Where legacy ISA interrupt `irq` arrives (x86_64): its GSI and the MPS
/// INTI flags of its interrupt source override (polarity in bits 0-1,
/// trigger mode in bits 2-3; 0 is the ISA default, edge and active high).
/// Without an override the GSI is the IRQ number.
#[cfg(target_arch = "x86_64")]
pub fn isa_irq(irq: u8) -> (u32, u16) {
    let Some(t) = find(b"APIC") else {
        return (u32::from(irq), 0);
    };
    let mut off = 44;
    while off + 2 <= t.len() {
        let (Some(typ), Some(len)) = (t.u8(off), t.u8(off + 1)) else {
            break;
        };
        let len = len as usize;
        if len < 2 || off + len > t.len() {
            break;
        }
        // Interrupt source override: bus at 2 (0, ISA), source at 3, GSI
        // at 4, flags at 8.
        if typ == 2 && len >= 10 && t.u8(off + 2) == Some(0) && t.u8(off + 3) == Some(irq) {
            if let (Some(gsi), Some(lo), Some(hi)) = (t.u32(off + 4), t.u8(off + 8), t.u8(off + 9)) {
                return (gsi, u16::from_le_bytes([lo, hi]));
            }
        }
        off += len;
    }
    (u32::from(irq), 0)
}

/// The first PCIe configuration space allocation of the MCFG.
#[derive(Clone, Copy)]
pub struct Mcfg {
    pub base: u64,
    pub bus_start: u8,
    pub bus_end: u8,
}

pub fn mcfg() -> Option<Mcfg> {
    let t = find(b"MCFG")?;
    if t.len() < 60 {
        return None;
    }
    Some(Mcfg {
        base: t.u64(44)?,
        bus_start: t.u8(54)?,
        bus_end: t.u8(55)?,
    })
}

/// The console UART of the SPCR (Serial Port Console Redirection table).
#[derive(Clone, Copy)]
pub struct Spcr {
    /// The interface type: 0 and 1 a 16550, 3 a PL011, 0xD and 0xE the SBSA
    /// UART (a PL011 subset).
    pub interface: u8,
    /// The base is an I/O port (x86) rather than a memory address.
    pub io: bool,
    pub base: u64,
    /// The GAS access size: 1 byte, 2 word, 3 dword (0 undefined).
    pub access: u8,
    /// The interrupt as the interrupt controller numbers it (a GIC INTID).
    pub gsiv: u32,
}

pub fn spcr() -> Option<Spcr> {
    let t = find(b"SPCR")?;
    if t.len() < 58 {
        return None;
    }
    Some(Spcr {
        interface: t.u8(36)?,
        io: t.u8(40)? == 1,
        base: t.u64(44)?,
        access: t.u8(43)?,
        gsiv: t.u32(54)?,
    })
}

/// The arm generic timer interrupts of the GTDT, as INTIDs.
#[derive(Clone, Copy)]
pub struct Gtdt {
    /// The non-secure EL1 physical timer.
    pub el1_phys: u32,
    /// The EL1 virtual timer.
    pub el1_virt: u32,
}

pub fn gtdt() -> Option<Gtdt> {
    let t = find(b"GTDT")?;
    if t.len() < 72 {
        return None;
    }
    Some(Gtdt { el1_phys: t.u32(56)?, el1_virt: t.u32(64)? })
}

/// How the FADT's arm boot flags (ACPI 5.1) say PSCI is called: `Some(true)`
/// through HVC, `Some(false)` through SMC, `None` when they name no PSCI.
pub fn fadt_psci_hvc() -> Option<bool> {
    let t = find(b"FACP")?;
    let flags = t.u8(129)?;
    (flags & 1 != 0).then_some(flags & 2 != 0)
}

/// The DMAR table: the DMA remapping units of an Intel platform (VT-d),
/// `kernel/src/iommu/vtd.rs`.
#[cfg(target_arch = "x86_64")]
#[derive(Clone, Copy)]
pub struct Dmar {
    t: Table,
}

#[cfg(target_arch = "x86_64")]
pub fn dmar() -> Option<Dmar> {
    let t = find(b"DMAR")?;
    (t.len() >= 48).then_some(Dmar { t })
}

#[cfg(target_arch = "x86_64")]
impl Dmar {
    /// The platform supports interrupt remapping (the table's `INTR_REMAP`
    /// flag).
    pub fn intr_remap(&self) -> bool {
        self.t.u8(36).is_some_and(|f| f & 1 != 0)
    }

    /// Each DMA remapping hardware unit (DRHD): its registers, whether it
    /// translates for every function no other unit's scope names
    /// (`INCLUDE_PCI_ALL`), and its scopes.
    pub fn units(&self) -> impl Iterator<Item = Drhd> + '_ {
        let mut off = 48;
        core::iter::from_fn(move || loop {
            let typ = self.t.u16(off)?;
            let len = self.t.u16(off + 2)? as usize;
            if len < 4 || off + len > self.t.len() {
                return None;
            }
            let here = off;
            off += len;
            if typ == 0 && len >= 16 {
                return Some(Drhd {
                    base: self.t.u64(here + 8)?,
                    include_all: self.t.u8(here + 4)? & 1 != 0,
                    scopes: &self.t.bytes[here + 16..here + len],
                });
            }
        })
    }
}

/// A DMA remapping unit.
#[cfg(target_arch = "x86_64")]
#[derive(Clone, Copy)]
pub struct Drhd {
    /// Its registers' physical address.
    pub base: u64,
    pub include_all: bool,
    scopes: &'static [u8],
}

#[cfg(target_arch = "x86_64")]
impl Drhd {
    /// The functions it translates for: endpoints (`kind` 1) and bridges
    /// (2, every function behind one), each a start bus and a path of
    /// (device, function) pairs from it.
    pub fn scopes(&self) -> impl Iterator<Item = DeviceScope> + '_ {
        let mut off = 0;
        core::iter::from_fn(move || {
            if off + 6 > self.scopes.len() {
                return None;
            }
            let len = self.scopes[off + 1] as usize;
            if len < 6 || off + len > self.scopes.len() {
                return None;
            }
            let s = DeviceScope { kind: self.scopes[off], start_bus: self.scopes[off + 5], path: &self.scopes[off + 6..off + len] };
            off += len;
            Some(s)
        })
    }
}

#[cfg(target_arch = "x86_64")]
#[derive(Clone, Copy)]
pub struct DeviceScope {
    pub kind: u8,
    pub start_bus: u8,
    path: &'static [u8],
}

#[cfg(target_arch = "x86_64")]
impl DeviceScope {
    pub fn path(&self) -> impl Iterator<Item = (u8, u8)> + '_ {
        self.path.chunks_exact(2).map(|p| (p[0], p[1]))
    }
}

/// The IORT's SMMUv3 (I/O Remapping Table, Arm DEN 0049): the unit's
/// registers and interrupts, and the requester ids the PCI root complex
/// node maps to its streams.
#[cfg(target_arch = "aarch64")]
#[derive(Clone, Copy)]
pub struct IortSmmu {
    pub base: u64,
    /// GSIVs (GIC INTIDs) of the event queue and the global error
    /// interrupt; 0 for none.
    pub event_gsiv: u32,
    pub gerror_gsiv: u32,
    /// The node's `COHACC override` flag: the unit snoops the caches.
    pub coherent: bool,
    /// The root complex's first ID mapping to this unit: requester ids
    /// `[rid_base, rid_base + rid_count)` are streams from `sid_base`.
    pub rid_base: u32,
    pub rid_count: u32,
    pub sid_base: u32,
}

/// The first SMMUv3 node of the IORT, with the first root complex mapping
/// that names it (an SMMUv3 nobody maps to is not in front of PCI).
#[cfg(target_arch = "aarch64")]
pub fn iort_smmuv3() -> Option<IortSmmu> {
    let t = find(b"IORT")?;
    let count = t.u32(36)? as usize;
    let first = t.u32(40)? as usize;
    // Each node: type, length, revision, identifier, the number of ID
    // mappings and their offset (16 bytes), then the node's own fields.
    let nodes = |mut off: usize| {
        let mut left = count;
        core::iter::from_fn(move || {
            if left == 0 {
                return None;
            }
            left -= 1;
            let len = t.u16(off + 1)? as usize;
            if len < 16 || off + len > t.len() {
                return None;
            }
            let here = off;
            off += len;
            Some(here)
        })
    };
    let smmu = nodes(first).find(|&n| t.u8(n) == Some(4) && t.u16(n + 1).is_some_and(|l| l >= 60))?;
    let mut s = IortSmmu {
        base: t.u64(smmu + 16)?,
        event_gsiv: t.u32(smmu + 44)?,
        gerror_gsiv: t.u32(smmu + 52)?,
        coherent: t.u32(smmu + 24)? & 1 != 0,
        rid_base: 0,
        rid_count: 0,
        sid_base: 0,
    };
    // The root complex (type 2): its mappings, 20 bytes each (input base,
    // count, output base, the output node's offset, flags).
    for rc in nodes(first).filter(|&n| t.u8(n) == Some(2)) {
        let (maps, at) = (t.u32(rc + 8)? as usize, t.u32(rc + 12)? as usize);
        for i in 0..maps {
            let m = rc + at + i * 20;
            if t.u32(m + 12)? as usize == smmu {
                s.rid_base = t.u32(m)?;
                s.rid_count = t.u32(m + 4)?;
                s.sid_base = t.u32(m + 8)?;
                return Some(s);
            }
        }
    }
    None
}
