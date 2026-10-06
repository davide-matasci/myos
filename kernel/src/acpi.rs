//! The ACPI static tables, from the RSDP Limine hands over: the platform
//! description on x86_64 and on aarch64 firmware that has one (EDK2). Only
//! the tables that name the platform are read, and no AML: MADT (the CPUs
//! and the interrupt controller), MCFG (PCIe ECAM), SPCR (the console
//! UART), GTDT (the arm timers), the FADT's arm boot flags (PSCI).
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
