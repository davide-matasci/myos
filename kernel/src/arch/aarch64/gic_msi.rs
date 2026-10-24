//! The GIC's MSI controllers (`irqchip::MsiChip`, `crate::msi`): a PCI
//! function's message raises an interrupt of the BSP's.
//!
//! * GICv2m (the GICv2 boards': QEMU `virt`'s frame): a range of SPIs a
//!   write of its INTID to the frame's `MSI_SETSPI_NS` raises, edge
//!   triggered. The frame's `MSI_TYPER` (or the description) names them.
//! * The GICv3 ITS: a device's write of an event id to `GITS_TRANSLATER`
//!   becomes an LPI (from INTID 8192) of the redistributor its collection
//!   names. The ITS keeps its tables in memory the kernel gives it (the
//!   devices, the collections, a translation table per device) and takes
//!   commands from a queue: `MAPD` gives a device its table, `MAPTI` maps
//!   one of its events to an LPI in collection 0 (the BSP's redistributor,
//!   `MAPC`), `DISCARD` takes it back. The redistributor's LPIs need two
//!   tables of their own: their configuration (a byte each: priority,
//!   enable) and their pending bits.
//!
//! Both deliver to the BSP, as the SPIs are; the device id an ITS knows a
//! function by is its requester id through the host's `msi-map`.

use alloc::vec::Vec;
use spin::Mutex;

use super::gic;
use crate::dma::cache;
use crate::irqchip::MsiChip;
use crate::mm;
use crate::msi::Message;
use crate::platform::{Msi, MsiKind};

/// Start the MSI controller the platform description names, after the
/// root GIC (`irqchip::start_msi`): a GICv2m frame with a GICv2, an ITS
/// with a GICv3. Nothing without one: PCI functions keep their INTx.
pub fn start(p: &crate::platform::Platform) {
    let (Some(msi), Some(intc)) = (p.msi.map(|c| c.value), p.intc.map(|c| c.value)) else {
        return;
    };
    let started = match (msi.kind, intc.driver) {
        (MsiKind::V2m, "gicv2" | "gicv3") => V2m::start(msi).map(|v| v as &'static dyn MsiChip),
        (MsiKind::Its, "gicv3") => Its::start(msi).map(|i| i as &'static dyn MsiChip),
        _ => Err("no driver for it"),
    };
    match started {
        Ok(chip) => crate::irqchip::msi_register(chip),
        Err(e) => crate::console::status_warn(&alloc::format!("msi: {e}: PCI functions keep their INTx")),
    }
}

fn read32(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

fn read64(addr: usize) -> u64 {
    unsafe { core::ptr::read_volatile(addr as *const u64) }
}

fn write32(addr: usize, value: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, value) }
}

fn write64(addr: usize, value: u64) {
    unsafe { core::ptr::write_volatile(addr as *mut u64, value) }
}

/// A set of up to 256 numbers given out, a bit each.
struct Bitmap([u64; 4]);

impl Bitmap {
    /// The lowest free one below `count`, taken.
    fn take(&mut self, count: u32) -> Option<u32> {
        let n = (0..count.min(256)).find(|&n| self.0[n as usize / 64] & 1 << (n % 64) == 0)?;
        self.0[n as usize / 64] |= 1 << (n % 64);
        Some(n)
    }

    fn give(&mut self, n: u32) {
        if n < 256 {
            self.0[n as usize / 64] &= !(1 << (n % 64));
        }
    }
}

// --- GICv2m ---

/// `MSI_TYPER`: the first SPI (bits 25:16) and how many (9:0).
const V2M_TYPER: usize = 0x008;
/// Where a device writes an SPI's INTID to raise it.
const V2M_SETSPI: u64 = 0x040;

struct V2m {
    phys: u64,
    first: u32,
    count: u32,
    used: Mutex<Bitmap>,
}

impl V2m {
    fn start(msi: Msi) -> Result<&'static V2m, &'static str> {
        let (first, count) = match msi.spis {
            Some(spis) => spis,
            None => {
                let va = super::paging::map_mmio(msi.base, 0x1000).ok_or("cannot map the GICv2m frame")?;
                let typer = read32(va + V2M_TYPER);
                ((typer >> 16) & 0x3FF, typer & 0x3FF)
            }
        };
        if count == 0 || first < 32 {
            return Err("GICv2m frame without SPIs");
        }
        let v2m = alloc::boxed::Box::leak(alloc::boxed::Box::new(V2m { phys: msi.base, first, count, used: Mutex::new(Bitmap([0; 4])) }));
        crate::console::status_ok(&alloc::format!("msi: gicv2m, SPIs {}-{}", first, first + count - 1));
        Ok(v2m)
    }
}

impl MsiChip for V2m {
    fn alloc(&self, _rid: u16) -> Option<(u32, Message)> {
        let irq = self.first + self.used.lock().take(self.count)?;
        crate::irqchip::root().unmask_edge(irq);
        Some((irq, Message { address: self.phys + V2M_SETSPI, data: irq }))
    }

    fn free(&self, irq: u32) {
        if let Some(n) = irq.checked_sub(self.first) {
            self.used.lock().give(n);
        }
    }
}

// --- GICv3 ITS ---

const GITS_CTLR: usize = 0x0000;
const GITS_TYPER: usize = 0x0008;
const GITS_CBASER: usize = 0x0080;
const GITS_CWRITER: usize = 0x0088;
const GITS_CREADR: usize = 0x0090;
const GITS_BASER: usize = 0x0100;
/// In the second 64 KiB frame: where devices write their event ids.
const GITS_TRANSLATER: u64 = 0x1_0040;
const GITS_CTLR_ENABLED: u32 = 1;
const GITS_CTLR_QUIESCENT: u32 = 1 << 31;
const GITS_TYPER_PTA: u64 = 1 << 19;
const BASER_VALID: u64 = 1 << 63;
const BASER_TYPE_DEVICE: u64 = 1;
const BASER_TYPE_COLLECTION: u64 = 4;
/// Table and queue attributes: inner shareable, write-back cacheable
/// (read- and write-allocate), as the kernel's memory is.
const ATTR_INNER_WB: u64 = 7 << 59;
const ATTR_SHAREABLE: u64 = 1 << 10;

// Redistributor LPI registers (RD frame).
const GICR_CTLR: usize = 0x0000;
const GICR_TYPER: usize = 0x0008;
const GICR_PROPBASER: usize = 0x0070;
const GICR_PENDBASER: usize = 0x0078;
const GICR_CTLR_ENABLE_LPIS: u32 = 1;
const GICR_TYPER_PLPIS: u64 = 1;
/// `GICR_PROPBASER`/`PENDBASER` cacheability (bits 9:7) and
/// shareability (11:10).
const RD_ATTR: u64 = 7 << 7 | 1 << 10;

/// The LPIs given out: 256 from 8192. Their configuration table covers
/// interrupt ids up to `1 << LPI_ID_BITS`.
const LPIS: u32 = 256;
const LPI_ID_BITS: u32 = 14;
/// An LPI's configuration byte: priority 0xa0, group 1 (bit 1, RES1),
/// enabled (bit 0).
const LPI_ON: u8 = 0xa0 | 1 << 1 | 1;
/// The device ids the device table covers, and the events of each device.
const DEVICE_ID_BITS: u32 = 12;
const EVENT_BITS: u32 = 3;
const EVENTS: usize = 1 << EVENT_BITS;

// ITS commands (the first doubleword's low byte).
const CMD_SYNC: u64 = 0x05;
const CMD_MAPD: u64 = 0x08;
const CMD_MAPC: u64 = 0x09;
const CMD_MAPTI: u64 = 0x0A;
const CMD_INV: u64 = 0x0C;
const CMD_DISCARD: u64 = 0x0F;
/// The command queue: one page of 32-byte commands.
const QUEUE: usize = 4096;

/// A device the ITS knows: its translation table, and the LPI each of its
/// events raises (0: free).
struct Device {
    id: u32,
    lpis: [u32; EVENTS],
}

struct ItsState {
    /// The queue's next free slot (`GITS_CWRITER`).
    write: usize,
    lpis: Bitmap,
    devices: Vec<Device>,
}

struct Its {
    regs: usize,
    phys: u64,
    queue: usize,
    /// The LPI configuration table (a byte per LPI, from 8192).
    prop: usize,
    /// The BSP's redistributor as the commands name it (`RDbase`).
    rdbase: u64,
    itt_size: usize,
    state: Mutex<ItsState>,
}

/// `n` zeroed, physically contiguous frames aligned to `align` bytes: their
/// physical address and the kernel's address of them.
fn frames(n: usize, align: u64) -> Option<(u64, usize)> {
    let extra = (align / 4096).saturating_sub(1) as usize;
    let phys = mm::alloc_contiguous_frames(n + extra)?;
    let phys = (phys + align - 1) & !(align - 1);
    Some((phys, mm::hhdm(phys) as usize))
}

impl Its {
    fn start(msi: Msi) -> Result<&'static Its, &'static str> {
        let regs = super::paging::map_mmio(msi.base, 0x2_0000).ok_or("cannot map the ITS")?;
        // Off while it is set up.
        write32(regs + GITS_CTLR, 0);
        let mut spins = 0u32;
        while read32(regs + GITS_CTLR) & GITS_CTLR_QUIESCENT == 0 {
            spins += 1;
            if spins > 1_000_000 {
                return Err("ITS does not quiesce");
            }
            core::hint::spin_loop();
        }
        let typer = read64(regs + GITS_TYPER);
        let itt_entry = ((typer >> 4) & 0xF) as usize + 1;
        let dev_bits = ((typer >> 13) & 0x1F) as u32 + 1;
        let event_bits = ((typer >> 8) & 0x1F) as u32 + 1;
        if event_bits < EVENT_BITS {
            return Err("ITS with too few event ids");
        }
        // The command queue, empty.
        let (queue_phys, queue) = frames(1, 4096).ok_or("no memory for the ITS")?;
        write64(regs + GITS_CBASER, BASER_VALID | ATTR_INNER_WB | ATTR_SHAREABLE | queue_phys);
        write64(regs + GITS_CWRITER, 0);
        // The device and collection tables, flat.
        for n in 0..8 {
            let reg = regs + GITS_BASER + n * 8;
            let baser = read64(reg);
            let kind = (baser >> 56) & 7;
            let entry = ((baser >> 48) & 0x1F) as usize + 1;
            let entries = match kind {
                BASER_TYPE_DEVICE => 1usize << dev_bits.min(DEVICE_ID_BITS),
                BASER_TYPE_COLLECTION => 1,
                _ => continue,
            };
            let pages = (entries * entry).div_ceil(4096).max(1);
            let (phys, va) = frames(pages, 4096).ok_or("no memory for the ITS")?;
            cache::clean(va, pages * 4096);
            let keep = baser & (0x1F << 48 | 7 << 56);
            write64(reg, BASER_VALID | ATTR_INNER_WB | ATTR_SHAREABLE | keep | phys | (pages as u64 - 1));
        }
        // The BSP's redistributor takes LPIs.
        let (rd, rd_phys) = gic::redistributor().ok_or("no redistributor for the BSP")?;
        if read64(rd + GICR_TYPER) & GICR_TYPER_PLPIS == 0 {
            return Err("redistributor without LPIs");
        }
        if read32(rd + GICR_CTLR) & GICR_CTLR_ENABLE_LPIS != 0 {
            return Err("LPIs already on (the firmware's tables)");
        }
        let id_bits = LPI_ID_BITS.min(gic::id_bits());
        if (1u32 << id_bits) < gic::LPI_FIRST + LPIS {
            return Err("GIC without enough interrupt ids for LPIs");
        }
        let prop_size = (1usize << id_bits) - gic::LPI_FIRST as usize;
        let (prop_phys, prop) = frames(prop_size.div_ceil(4096), 4096).ok_or("no memory for the LPIs")?;
        let (pend_phys, pend) = frames(((1usize << id_bits) / 8).div_ceil(4096), 0x1_0000).ok_or("no memory for the LPIs")?;
        cache::clean(prop, prop_size);
        cache::clean(pend, (1usize << id_bits) / 8);
        write64(rd + GICR_PROPBASER, prop_phys | RD_ATTR | u64::from(id_bits - 1));
        write64(rd + GICR_PENDBASER, pend_phys | RD_ATTR);
        write32(rd + GICR_CTLR, read32(rd + GICR_CTLR) | GICR_CTLR_ENABLE_LPIS);
        cache::barrier();
        write32(regs + GITS_CTLR, GITS_CTLR_ENABLED);
        // Collection 0 is the BSP's redistributor: by its address, or by
        // its processor number.
        let rdbase = if typer & GITS_TYPER_PTA != 0 { rd_phys >> 16 } else { (read64(rd + GICR_TYPER) >> 8) & 0xFFFF };
        let its = alloc::boxed::Box::leak(alloc::boxed::Box::new(Its {
            regs,
            phys: msi.base,
            queue,
            prop,
            rdbase,
            itt_size: (EVENTS * itt_entry).max(256),
            state: Mutex::new(ItsState { write: 0, lpis: Bitmap([0; 4]), devices: Vec::new() }),
        }));
        {
            let mut st = its.state.lock();
            its.command(&mut st, [CMD_MAPC, 0, 1 << 63 | rdbase << 16, 0]);
            if !its.command(&mut st, [CMD_SYNC, 0, rdbase << 16, 0]) {
                return Err("ITS does not take commands");
            }
        }
        crate::console::status_ok(&alloc::format!("msi: its, {} LPIs from {}", LPIS, gic::LPI_FIRST));
        Ok(its)
    }

    /// Queue `cmd` and wait for the ITS to have read it: false when it
    /// stalls (a command error) or takes too long.
    fn command(&self, st: &mut ItsState, cmd: [u64; 4]) -> bool {
        let slot = self.queue + st.write;
        for (i, dw) in cmd.iter().enumerate() {
            unsafe { core::ptr::write_volatile((slot + i * 8) as *mut u64, *dw) };
        }
        cache::clean(slot, 32);
        cache::barrier();
        st.write = (st.write + 32) % QUEUE;
        write64(self.regs + GITS_CWRITER, st.write as u64);
        for _ in 0..10_000_000 {
            let read = read64(self.regs + GITS_CREADR);
            if read & 1 != 0 {
                return false;
            }
            if (read & 0xF_FFE0) as usize == st.write {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    /// LPI `lpi`'s configuration byte, written back for the redistributor.
    fn set_prop(&self, lpi: u32, value: u8) {
        let at = self.prop + (lpi - gic::LPI_FIRST) as usize;
        unsafe { core::ptr::write_volatile(at as *mut u8, value) };
        cache::clean(at, 1);
        cache::barrier();
    }
}

impl MsiChip for Its {
    fn alloc(&self, rid: u16) -> Option<(u32, Message)> {
        let id = crate::dt::pci_msi_devid(rid);
        if id >= 1 << DEVICE_ID_BITS {
            return None;
        }
        let mut st = self.state.lock();
        let lpi = gic::LPI_FIRST + st.lpis.take(LPIS)?;
        let dev = match st.devices.iter().position(|d| d.id == id) {
            Some(i) => i,
            None => {
                // Its translation table: a frame, 256-byte aligned.
                let Some((itt, va)) = frames(self.itt_size.div_ceil(4096), 256) else {
                    st.lpis.give(lpi - gic::LPI_FIRST);
                    return None;
                };
                cache::clean(va, self.itt_size);
                let mapd = [CMD_MAPD | u64::from(id) << 32, u64::from(EVENT_BITS - 1), 1 << 63 | itt, 0];
                if !self.command(&mut st, mapd) {
                    st.lpis.give(lpi - gic::LPI_FIRST);
                    return None;
                }
                st.devices.push(Device { id, lpis: [0; EVENTS] });
                st.devices.len() - 1
            }
        };
        let Some(event) = st.devices[dev].lpis.iter().position(|l| *l == 0) else {
            st.lpis.give(lpi - gic::LPI_FIRST);
            return None;
        };
        st.devices[dev].lpis[event] = lpi;
        self.set_prop(lpi, LPI_ON);
        let dw0 = u64::from(id) << 32;
        self.command(&mut st, [CMD_MAPTI | dw0, event as u64 | u64::from(lpi) << 32, 0, 0]);
        self.command(&mut st, [CMD_INV | dw0, event as u64, 0, 0]);
        self.command(&mut st, [CMD_SYNC, 0, self.rdbase << 16, 0]);
        Some((lpi, Message { address: self.phys + GITS_TRANSLATER, data: event as u32 }))
    }

    fn free(&self, irq: u32) {
        let mut st = self.state.lock();
        let found = st.devices.iter_mut().find_map(|d| {
            let event = d.lpis.iter().position(|l| *l == irq)?;
            d.lpis[event] = 0;
            Some((d.id, event))
        });
        let Some((id, event)) = found else {
            return;
        };
        self.command(&mut st, [CMD_DISCARD | u64::from(id) << 32, event as u64, 0, 0]);
        self.command(&mut st, [CMD_SYNC, 0, self.rdbase << 16, 0]);
        self.set_prop(irq, 0);
        st.lpis.give(irq - gic::LPI_FIRST);
    }
}
