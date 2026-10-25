//! The RISC-V IOMMU (RISC-V IOMMU Architecture Specification, version
//! 1.0), the riscv64 unit: a PCI function of class 08.06 (system
//! peripheral, IOMMU) whose BAR 0 holds the registers, in front of every
//! other function of its host (QEMU's `riscv-iommu-pci`, on `virt` since
//! QEMU 10.1). Each function is a *device id* (its requester id).
//!
//! The unit walks a device directory table (three levels, so the whole
//! 24-bit device id space; a page of 64-byte device contexts at the leaves,
//! 32-byte ones for a unit without MSI translation) to the function's
//! device context: *bare* (the identity domain: a kernel driver's device
//! sees memory as it is, no page table walked) or *second-stage
//! translation* through a G-stage page table of the domain's (Sv39x4 or
//! Sv48x4, the CPU's page table format with a 16 KiB root, tagged with a
//! GSCID). The first stage is never used: there are no processes behind
//! the devices. [`attach`] rewrites a function's context for a table of
//! its own; [`map`] puts pages in it one-to-one, so a bus address stays a
//! physical one.
//!
//! A confined function's interrupt messages are the unit's too (the
//! architecture's MSI translation, chapter 2.3; issue #590). The IMSIC's
//! page would take any identity it is written, the APLIC's wired sources
//! and the other functions' MSIs included, so where the unit has
//! memory-resident interrupt files (`CAP.MSI_MRIF`) the function's
//! messages go to one of its own ([`msi_message`]): a 512-byte page of
//! pending and enable bits in memory, one identity of it enabled, whose
//! write the unit turns into a *notice*, a message of its own raising the
//! function's IMSIC identity. Whatever the function writes, it raises that
//! identity or nothing. A unit without them maps the IMSIC page into the
//! domain instead, and the IMSIC decides.
//!
//! The unit takes commands from a queue in memory (invalidate a context,
//! a domain's translations, a fence) and reports refused DMAs through a
//! fault queue. The PCI unit signals the queue by MSI only (`CAP.IGS`),
//! which the riscv64 kernel does not take (the PLIC has no message
//! interrupts; the AIA's IMSIC could, issue #597): the queue is drained when `/proc/iommu` is read ([`text`])
//! and when a function is let go ([`detach`]), which is when the counts
//! are looked at. The unit's walks are coherent with the harts' caches, as
//! the architecture has them.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicUsize, Ordering};

use spin::Mutex;

use super::Fault;
use crate::mm;
use crate::msi::Message;

// The registers (chapter 5), 64-bit ones read and written whole.
const CAP: usize = 0x00;
const DDTP: usize = 0x10;
const CQB: usize = 0x18;
const CQH: usize = 0x20;
const CQT: usize = 0x24;
const FQB: usize = 0x28;
const FQH: usize = 0x30;
const FQT: usize = 0x34;
const CQCSR: usize = 0x48;
const FQCSR: usize = 0x4C;

const CAP_SV39X4: u64 = 1 << 17;
const CAP_SV48X4: u64 = 1 << 18;
const CAP_MSI_FLAT: u64 = 1 << 22;
const CAP_MSI_MRIF: u64 = 1 << 23;
/// The directory pointer: the mode in bits 0-3 (`4` three levels), busy
/// in bit 4, the root's PPN from bit 10.
const DDTP_3LVL: u64 = 4;
const DDTP_BUSY: u64 = 1 << 4;
/// A queue's control: enable, active, busy, and the memory fault, the
/// illegal command and the overflow it reports.
const Q_ENABLE: u32 = 1;
const Q_FAULT: u32 = 1 << 8;
const Q_ILLEGAL: u32 = 1 << 10;
const Q_OVERFLOW: u32 = 1 << 9;
const Q_ACTIVE: u32 = 1 << 16;
const Q_BUSY: u32 = 1 << 17;

/// Commands (chapter 3): the opcode in bits 0-6, the function in 7-9.
const CMD_IOTINVAL_GVMA: u64 = 1 | 1 << 7;
const CMD_IOFENCE_C: u64 = 2;
const CMD_IODIR_INVAL_DDT: u64 = 3;
/// IOTINVAL: by GSCID (`GV`, the id from bit 44); IODIR: by device id
/// (`DV`, the id from bit 40).
const CMD_GV: u64 = 1 << 33;
const CMD_DV: u64 = 1 << 33;

/// A directory entry or a device context's translation control: valid.
const VALID: u64 = 1;
/// `iohgatp`: the mode in bits 60-63 (`8` Sv39x4, `9` Sv48x4), the GSCID
/// from bit 44, the root's PPN below.
const IOHGATP_SV39X4: u64 = 8 << 60;
const IOHGATP_SV48X4: u64 = 9 << 60;
/// A page table entry: valid alone points to the next table; a leaf is
/// readable and writable (`PTE_LEAF`, with execute, tells the two apart),
/// user (every G-stage leaf is), accessed, dirty. The PPN from bit 10.
const PTE_TABLE: u64 = 1;
const PTE_LEAF: u64 = 0b1110;
const PTE_PAGE: u64 = 1 | 1 << 1 | 1 << 2 | 1 << 4 | 1 << 6 | 1 << 7;
const PTE_PPN: u64 = 0x003F_FFFF_FFFF_FC00;

const PAGE: u64 = 4096;

// The device context's MSI translation (its 64-byte format): the MSI page
// table (`msiptp`, flat), and the addresses that are an interrupt file's,
// a page number matched under a mask (`msi_addr_mask`, `msi_addr_pattern`).
const DC_MSIPTP: usize = 4;
const DC_MSI_MASK: usize = 5;
const DC_MSI_PATTERN: usize = 6;
const MSIPTP_FLAT: u64 = 1 << 60;
/// An MSI page table entry in MRIF mode (valid, `M` 1), the MRIF's
/// address from bit 7 (its 512-byte number); its second doubleword the
/// notice's IMSIC page from bit 10, its identity's low 10 bits at bit 0
/// and the 11th at bit 60.
const MSI_PTE_MRIF: u64 = 1 | 1 << 1;
const NOTICE_ID_MSB: u64 = 1 << 60;
/// Where in its frame (after the MSI page table's one entry) the MRIF
/// is, and the one identity of it the function's messages carry (in the
/// first 64, whose pending and enable doublewords are the MRIF's first
/// two in every reading of its layout).
const MRIF_OFFSET: u64 = 512;
const MRIF_ID: u32 = 1;
/// The identity domain's number (its contexts have no GSCID); the
/// functions programs drive get the next.
const IDENTITY: u16 = 1;
/// How long a command or an enable may take (a spin count).
const SPINS: u32 = 10_000_000;
/// The queues: 256 commands of 16 bytes (one page), 128 fault records of
/// 32 (one page); the registers take log2 minus one.
const CMDQ_LOG2: u32 = 8;
const FAULTQ_LOG2: u32 = 7;

struct Domain {
    gscid: u16,
    /// Its G-stage table's root (16 KiB, aligned).
    root: u64,
    dev: (u8, u8, u8),
    pages: usize,
    /// The function's MRIF while its interrupt is routed: the frame of
    /// its MSI page table and MRIF, and the IMSIC identity the notices
    /// raise.
    mrif: Option<(u64, u32)>,
}

struct State {
    regs: usize,
    unit: (u8, u8, u8),
    base: u64,
    /// The directory's root, and the size of a device context (64 bytes,
    /// 32 without MSI translation).
    ddt: u64,
    dc_len: usize,
    /// How many device ids have a context (every one of a populated bus).
    devices: u32,
    /// The command queue's entries and its tail index.
    cmdq: *mut [u64; 2],
    cmdq_tail: u32,
    /// The fault queue's records and its head index.
    faultq: *const [u64; 4],
    faultq_head: u32,
    /// The G-stage walk: its levels (3 Sv39x4, 4 Sv48x4) and the guest
    /// physical address bits.
    levels: u8,
    gpa_bits: u8,
    domains: Vec<Domain>,
    /// Whether the unit has memory-resident interrupt files, and the
    /// frames of the MRIFs given back, for the next ones: never freed, so
    /// [`msi_notice`] may still clear one it read before it went.
    mrif: bool,
    spare: Vec<u64>,
}

unsafe impl Send for State {}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Each IMSIC identity's MRIF (where the HHDM maps it) while a function's
/// notices raise it, for [`msi_notice`]; 0 for none.
static NOTICES: [AtomicUsize; 2048] = [const { AtomicUsize::new(0) }; 2048];

fn read32(va: usize) -> u32 {
    unsafe { core::ptr::read_volatile(va as *const u32) }
}

fn read64(va: usize) -> u64 {
    unsafe { core::ptr::read_volatile(va as *const u64) }
}

fn write32(va: usize, v: u32) {
    unsafe { core::ptr::write_volatile(va as *mut u32, v) }
}

fn write64(va: usize, v: u64) {
    unsafe { core::ptr::write_volatile(va as *mut u64, v) }
}

/// Wait for a register to show `want` in the bits of `mask`.
fn wait(va: usize, mask: u32, want: u32) -> bool {
    for _ in 0..SPINS {
        if read32(va) & mask == want {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// A table's or queue's base register value: its PPN from bit 10, `low`
/// in the bits below.
fn ppn(phys: u64, low: u64) -> u64 {
    (phys >> 12) << 10 | low
}

/// Queue a command for the unit and wait for it to take it (false when it
/// did not, or reported it illegal or unreadable).
fn command(st: &mut State, dword0: u64, dword1: u64) -> bool {
    let mask = (1 << CMDQ_LOG2) - 1;
    let i = (st.cmdq_tail & mask) as usize;
    unsafe { st.cmdq.add(i).write_volatile([dword0, dword1]) };
    crate::dma::cache::barrier();
    st.cmdq_tail = (st.cmdq_tail + 1) & mask;
    write32(st.regs + CQT, st.cmdq_tail);
    for _ in 0..SPINS {
        if read32(st.regs + CQCSR) & (Q_FAULT | Q_ILLEGAL) != 0 {
            return false;
        }
        if read32(st.regs + CQH) & mask == st.cmdq_tail {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// The unit forgets device id `devid`'s context (every context for
/// `None`) and GSCID `gscid`'s translations (every one for `None`).
fn invalidate(st: &mut State, devid: Option<u32>, gscid: Option<u16>) {
    let dir = devid.map_or(0, |d| CMD_DV | u64::from(d) << 40);
    command(st, CMD_IODIR_INVAL_DDT | dir, 0);
    let vm = gscid.map_or(0, |g| CMD_GV | u64::from(g) << 44);
    command(st, CMD_IOTINVAL_GVMA | vm, 0);
    command(st, CMD_IOFENCE_C, 0);
}

/// The fault queue's records so far, each refused DMA recorded, the queue
/// consumed; an overflow (records lost) cleared.
fn poll(st: &mut State) {
    let mask = (1 << FAULTQ_LOG2) - 1;
    let tail = read32(st.regs + FQT) & mask;
    while st.faultq_head != tail {
        let r = unsafe { st.faultq.add(st.faultq_head as usize).read_volatile() };
        let (cause, ttype) = (r[0] & 0xfff, r[0] >> 34 & 0x3f);
        super::record(Fault {
            sid: (r[0] >> 40) as u16,
            addr: r[2] & !0xfff,
            reason: cause.min(0xff) as u8,
            // Transaction types: 1 and 5 instruction fetches, 2 and 6 reads.
            read: matches!(ttype, 1 | 2 | 5 | 6),
        });
        st.faultq_head = (st.faultq_head + 1) & mask;
    }
    write32(st.regs + FQH, st.faultq_head);
    if read32(st.regs + FQCSR) & Q_OVERFLOW != 0 {
        write32(st.regs + FQCSR, Q_ENABLE | Q_OVERFLOW);
    }
}

/// The index into the G-stage table at `level` (0 the root, with 2 more
/// bits than the 9 of the others).
fn index(st: &State, iova: u64, level: u8) -> usize {
    let shift = 12 + 9 * u64::from(st.levels - 1 - level);
    let bits = if level == 0 { 0x7ff } else { 0x1ff };
    ((iova >> shift) & bits) as usize
}

/// The leaf entry for `iova` in the table rooted at `root`, the tables
/// above it made when `create`.
fn entry(st: &State, root: u64, iova: u64, create: bool) -> Option<*mut u64> {
    let mut table = root;
    for level in 0..st.levels {
        let e = unsafe { (mm::hhdm(table) as *mut u64).add(index(st, iova, level)) };
        if level == st.levels - 1 {
            return Some(e);
        }
        let v = unsafe { e.read_volatile() };
        if v & VALID == 0 {
            if !create {
                return None;
            }
            table = mm::alloc_frame();
            unsafe { e.write_volatile(ppn(table, PTE_TABLE)) };
        } else if v & PTE_LEAF == 0 {
            table = (v & PTE_PPN) << 2;
        } else {
            return None;
        }
    }
    None
}

/// Map the pages of `[phys, phys + len)` one-to-one into the table at
/// `root` (`phys` and `len` whole pages).
fn map_pages(st: &State, root: u64, phys: u64, len: u64) -> bool {
    let mut a = phys;
    while a < phys + len {
        let Some(e) = entry(st, root, a, true) else {
            return false;
        };
        unsafe { e.write_volatile(ppn(a, PTE_PAGE)) };
        a += PAGE;
    }
    crate::dma::cache::barrier();
    true
}

/// Give the tables under `table` (at `level`) back, then `table`'s frames
/// (4 for the root).
fn free_tables(st: &State, table: u64, level: u8) {
    let frames = if level == 0 { 4 } else { 1 };
    if level < st.levels - 1 {
        for i in 0..512 * frames {
            let v = unsafe { (mm::hhdm(table) as *const u64).add(i).read_volatile() };
            if v & VALID != 0 && v & PTE_LEAF == 0 {
                free_tables(st, (v & PTE_PPN) << 2, level + 1);
            }
        }
    }
    for i in 0..frames {
        mm::free_frame(table + i as u64 * PAGE);
    }
}

/// A G-stage root: 16 KiB aligned to 16 KiB, zeroed (the frames of a
/// longer run that are not it go back).
fn alloc_root() -> Option<u64> {
    let (run, _) = mm::alloc_contiguous_pages(7, mm::ANYWHERE)?;
    let root = (run + 4 * PAGE - 1) & !(4 * PAGE - 1);
    for i in 0..7 {
        let f = run + i * PAGE;
        if f < root || f >= root + 4 * PAGE {
            mm::free_frame(f);
        }
    }
    Some(root)
}

/// Device id `devid`'s context (its `dc_len` bytes) in the directory, the
/// tables down to it made when `create`.
fn context(st: &State, devid: u32, create: bool) -> Option<*mut u64> {
    // The leaf holds 4096 / dc_len contexts: 6 index bits (7 for 32-byte
    // ones), the two levels above 9 each.
    let leaf_bits = if st.dc_len == 64 { 6 } else { 7 };
    let mut table = st.ddt;
    for shift in [leaf_bits + 9, leaf_bits] {
        let e = unsafe { (mm::hhdm(table) as *mut u64).add((devid >> shift & 0x1ff) as usize) };
        let v = unsafe { e.read_volatile() };
        if v & VALID != 0 {
            table = (v & PTE_PPN) << 2;
        } else {
            if !create {
                return None;
            }
            table = mm::alloc_frame();
            unsafe { e.write_volatile(ppn(table, VALID)) };
        }
    }
    let i = (devid & ((1 << leaf_bits) - 1)) as usize;
    Some(unsafe { mm::hhdm(table).add(i * st.dc_len) as *mut u64 })
}

/// Write device id `devid`'s context: invalid first, `iohgatp` (the
/// G-stage table or bare), the first stage bare, then valid; the unit
/// told to forget the old one.
fn set_context(st: &mut State, devid: u32, iohgatp: u64, gscid: Option<u16>) -> bool {
    let Some(dc) = context(st, devid, true) else {
        return false;
    };
    unsafe {
        dc.write_volatile(0);
        crate::dma::cache::barrier();
        for i in 2..st.dc_len / 8 {
            dc.add(i).write_volatile(0);
        }
        dc.add(1).write_volatile(iohgatp);
        crate::dma::cache::barrier();
        dc.write_volatile(VALID);
    }
    crate::dma::cache::barrier();
    invalidate(st, Some(devid), gscid);
    true
}

fn devid_of(bus: u8, slot: u8, func: u8) -> u32 {
    u32::from(bus) << 8 | u32::from(slot) << 3 | u32::from(func)
}

/// Find the unit, give every function of a populated bus a bare context,
/// start the queues and translation.
pub fn init() {
    let Some(bdf) = crate::pci::find_class(0x08, 0x06, 0) else {
        return;
    };
    let unit = (bdf.bus, bdf.slot, bdf.func);
    let Some((base, _)) = crate::pci::bar_mmio(bdf, 0) else {
        return;
    };
    // The kernel's: a program must not take the unit through `/dev/pci`.
    crate::pci::claim(bdf.bus, bdf.slot, bdf.func, b"iommu");
    crate::pci::enable(bdf.bus, bdf.slot, bdf.func);
    let Some((regs, _)) = crate::pci::bar_map(bdf.bus, bdf.slot, bdf.func, 0) else {
        crate::console::status_warn("iommu: riscv unit: cannot map its registers");
        return;
    };
    let cap = read64(regs + CAP);
    let (levels, gpa_bits) = if cap & CAP_SV39X4 != 0 {
        (3, 41)
    } else if cap & CAP_SV48X4 != 0 {
        (4, 50)
    } else {
        crate::console::status_warn("iommu: riscv unit without Sv39x4 or Sv48x4, left off");
        return;
    };
    let dc_len = if cap & CAP_MSI_FLAT != 0 { 64 } else { 32 };
    let Some((cmdq, cmdq_va)) = mm::alloc_contiguous_pages(1, mm::ANYWHERE) else {
        return;
    };
    let Some((faultq, faultq_va)) = mm::alloc_contiguous_pages(1, mm::ANYWHERE) else {
        return;
    };
    let mut st = State {
        regs,
        unit,
        base,
        ddt: mm::alloc_frame(),
        dc_len,
        devices: 0,
        cmdq: cmdq_va as *mut [u64; 2],
        cmdq_tail: 0,
        faultq: faultq_va as *const [u64; 4],
        faultq_head: 0,
        levels,
        gpa_bits,
        domains: Vec::new(),
        mrif: cap & CAP_MSI_FLAT != 0 && cap & CAP_MSI_MRIF != 0,
        spare: Vec::new(),
    };
    // Every device id of a bus with a function on it: the ones there and
    // the ones a later rescan may find.
    let mut buses = [false; 256];
    for f in crate::pci::functions() {
        buses[usize::from(f.bus)] = true;
    }
    for bus in 0..256u32 {
        if !buses[bus as usize] {
            continue;
        }
        for devfn in 0..256 {
            if let Some(dc) = context(&st, bus << 8 | devfn, true) {
                unsafe { dc.write_volatile(VALID) };
                st.devices += 1;
            }
        }
    }
    crate::dma::cache::barrier();

    // Off while the tables are named, then the command queue, the fault
    // queue, the directory (translation on), the whole invalidation.
    write64(regs + DDTP, 0);
    write64(regs + CQB, ppn(cmdq, u64::from(CMDQ_LOG2 - 1)));
    write32(regs + CQT, 0);
    write32(regs + CQCSR, Q_ENABLE);
    if !wait(regs + CQCSR, Q_ACTIVE | Q_BUSY, Q_ACTIVE) {
        crate::console::status_warn("iommu: riscv unit's command queue stays off, left off");
        return;
    }
    write64(regs + FQB, ppn(faultq, u64::from(FAULTQ_LOG2 - 1)));
    write32(regs + FQH, 0);
    write32(regs + FQCSR, Q_ENABLE);
    if !wait(regs + FQCSR, Q_ACTIVE | Q_BUSY, Q_ACTIVE) {
        crate::console::status_warn("iommu: riscv unit's fault queue stays off, left off");
        return;
    }
    write64(regs + DDTP, ppn(st.ddt, DDTP_3LVL));
    for _ in 0..SPINS {
        if read64(regs + DDTP) & DDTP_BUSY == 0 {
            break;
        }
        core::hint::spin_loop();
    }
    if read64(regs + DDTP) & 0xf != DDTP_3LVL {
        crate::console::status_warn("iommu: riscv unit refuses a 3-level directory, left off");
        return;
    }
    invalidate(&mut st, None, None);
    crate::console::status_ok(&alloc::format!(
        "iommu: riscv unit at {:02x}:{:02x}.{}, {} device ids, Sv{}x4 second stage{}",
        unit.0,
        unit.1,
        unit.2,
        st.devices,
        if levels == 3 { 39 } else { 48 },
        if st.mrif { ", MRIFs" } else { "" }
    ));
    *STATE.lock() = Some(st);
}

/// `iohgatp` for a domain's table at `root`, tagged `gscid`.
fn iohgatp(st: &State, root: u64, gscid: u16) -> u64 {
    let mode = if st.levels == 3 { IOHGATP_SV39X4 } else { IOHGATP_SV48X4 };
    mode | u64::from(gscid) << 44 | root >> 12
}

pub fn attach(bus: u8, slot: u8, func: u8) -> Option<u32> {
    let mut guard = STATE.lock();
    let st = guard.as_mut()?;
    let dev = (bus, slot, func);
    if dev == st.unit {
        return None;
    }
    if let Some(d) = st.domains.iter().find(|d| d.dev == dev) {
        return Some(u32::from(d.gscid));
    }
    let gscid = (IDENTITY + 1..=0xffff).find(|g| !st.domains.iter().any(|d| d.gscid == *g))?;
    let root = alloc_root()?;
    let atp = iohgatp(st, root, gscid);
    if !set_context(st, devid_of(bus, slot, func), atp, Some(gscid)) {
        free_tables(st, root, 0);
        return None;
    }
    st.domains.push(Domain { gscid, root, dev, pages: 0, mrif: None });
    Some(u32::from(gscid))
}

pub fn map(domain: u32, phys: u64, len: usize) -> bool {
    let mut guard = STATE.lock();
    let Some(st) = guard.as_mut() else {
        return false;
    };
    let Some(i) = st.domains.iter().position(|d| u32::from(d.gscid) == domain) else {
        return false;
    };
    let root = st.domains[i].root;
    let (phys, len) = (phys & !0xfff, (len as u64 + (phys & 0xfff) + 0xfff) & !0xfff);
    if len == 0 || phys + len > 1 << st.gpa_bits || !map_pages(st, root, phys, len) {
        return false;
    }
    st.domains[i].pages += (len / PAGE) as usize;
    true
}

pub fn detach(bus: u8, slot: u8, func: u8) {
    let mut guard = STATE.lock();
    let Some(st) = guard.as_mut() else {
        return;
    };
    let dev = (bus, slot, func);
    let Some(i) = st.domains.iter().position(|d| d.dev == dev) else {
        return;
    };
    let d = st.domains.swap_remove(i);
    poll(st);
    set_context(st, devid_of(bus, slot, func), 0, Some(d.gscid));
    free_tables(st, d.root, 0);
    if let Some((frame, irq)) = d.mrif {
        NOTICES[irq as usize].store(0, Ordering::SeqCst);
        st.spare.push(frame);
    }
}

/// The message confined function `bus:slot.func` writes for its IMSIC
/// identity `irq`, whose message is `msg` (the IMSIC page, the identity).
/// With MRIFs: an MRIF of the function's own, its identity [`MRIF_ID`]
/// enabled and noticed as `irq` at `msg`'s page, which the function's
/// context makes the one interrupt file its writes to that page reach;
/// the message names that page and [`MRIF_ID`]. Without: `msg`, the
/// IMSIC page mapped into the domain. A function in passthrough writes
/// `msg` as it is.
pub fn msi_message(bus: u8, slot: u8, func: u8, irq: u32, msg: Message) -> Message {
    let mut guard = STATE.lock();
    let Some(st) = guard.as_mut() else {
        return msg;
    };
    let Some(i) = st.domains.iter().position(|d| d.dev == (bus, slot, func)) else {
        return msg;
    };
    let devid = devid_of(bus, slot, func);
    let dc = context(st, devid, false);
    let (Some(dc), true, true) = (dc, st.mrif, (irq as usize) < NOTICES.len()) else {
        let root = st.domains[i].root;
        if map_pages(st, root, msg.address & !0xfff, PAGE) {
            st.domains[i].pages += 1;
        }
        return msg;
    };
    release_mrif(st, i);
    let frame = st.spare.pop().unwrap_or_else(mm::alloc_frame);
    let mrif = frame + MRIF_OFFSET;
    unsafe {
        let t = mm::hhdm(frame) as *mut u64;
        core::ptr::write_bytes(t, 0, PAGE as usize / 8);
        // The MSI page table's one entry (interrupt file 0: the mask
        // below is 0), then the MRIF's enable bits for identities 0..63.
        t.write_volatile(MSI_PTE_MRIF | (mrif >> 9) << 7);
        t.add(1).write_volatile(
            (msg.address >> 12) << 10 | u64::from(irq & 0x3ff) | if irq >> 10 != 0 { NOTICE_ID_MSB } else { 0 },
        );
        t.add((MRIF_OFFSET / 8 + 1) as usize).write_volatile(1 << MRIF_ID);
        crate::dma::cache::barrier();
        dc.add(DC_MSI_MASK).write_volatile(0);
        dc.add(DC_MSI_PATTERN).write_volatile(msg.address >> 12);
        crate::dma::cache::barrier();
        dc.add(DC_MSIPTP).write_volatile(MSIPTP_FLAT | frame >> 12);
    }
    crate::dma::cache::barrier();
    NOTICES[irq as usize].store(mm::hhdm(mrif) as usize, Ordering::SeqCst);
    let gscid = st.domains[i].gscid;
    invalidate(st, Some(devid), Some(gscid));
    st.domains[i].mrif = Some((frame, irq));
    Message { address: msg.address, data: MRIF_ID }
}

/// Domain `i`'s MRIF gone: its function's context translates no messages
/// (they reach no interrupt file: the IMSIC page is not in the domain).
fn release_mrif(st: &mut State, i: usize) {
    let Some((frame, irq)) = st.domains[i].mrif.take() else {
        return;
    };
    let (dev, gscid) = (st.domains[i].dev, st.domains[i].gscid);
    let devid = devid_of(dev.0, dev.1, dev.2);
    if let Some(dc) = context(st, devid, false) {
        unsafe { dc.add(DC_MSIPTP).write_volatile(0) };
        crate::dma::cache::barrier();
        invalidate(st, Some(devid), Some(gscid));
    }
    NOTICES[irq as usize].store(0, Ordering::SeqCst);
    st.spare.push(frame);
}

/// The function's interrupt given back (`msi::pci_release`): its MRIF, if
/// it has one.
pub fn msi_release(bus: u8, slot: u8, func: u8) {
    let mut guard = STATE.lock();
    let Some(st) = guard.as_mut() else {
        return;
    };
    if let Some(i) = st.domains.iter().position(|d| d.dev == (bus, slot, func)) {
        release_mrif(st, i);
    }
}

/// IMSIC identity `id` claimed (`aia`): when it is a function's notice,
/// its MRIF's pending bits cleared before its handlers run, so the
/// function's next message is noticed again. From interrupt context.
pub fn msi_notice(id: u32) {
    let Some(n) = NOTICES.get(id as usize) else {
        return;
    };
    let va = n.load(Ordering::SeqCst);
    if va != 0 {
        unsafe { (va as *mut u64).write_volatile(0) };
    }
}

pub fn text() -> String {
    let mut s = String::new();
    let mut guard = STATE.lock();
    let Some(st) = guard.as_mut() else {
        s.push_str("none\n");
        return s;
    };
    poll(st);
    let _ = writeln!(
        s,
        "riscv {:02x}:{:02x}.{} 0x{:x} devices {} sv{}x4 {}-bit{}",
        st.unit.0,
        st.unit.1,
        st.unit.2,
        st.base,
        st.devices,
        if st.levels == 3 { 39 } else { 48 },
        st.gpa_bits,
        if st.mrif { " mrif" } else { "" }
    );
    let _ = writeln!(s, "domain {} bare", IDENTITY);
    for d in &st.domains {
        let _ = write!(s, "domain {} {:02x}:{:02x}.{} pages {}", d.gscid, d.dev.0, d.dev.1, d.dev.2, d.pages);
        if let Some((frame, irq)) = d.mrif {
            // The MRIF's pending bits for identities 0..63.
            let pending = unsafe { (mm::hhdm(frame + MRIF_OFFSET) as *const u64).read_volatile() };
            let _ = write!(s, " mrif {irq} pending 0x{pending:x}");
        }
        s.push('\n');
    }
    s
}
