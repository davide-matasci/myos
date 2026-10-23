//! AMD-Vi, the unit of an AMD x86_64 board: the IOMMUs of the ACPI IVRS
//! table (AMD I/O Virtualization Technology (IOMMU) Specification,
//! revision 3: chapter 2 the data structures, 3 the registers, 5 the ACPI
//! table). Each unit translates the DMA of the functions its device entries
//! name, or of every function (an `all` entry).
//!
//! A unit walks a flat device table (one 32-byte entry per device id,
//! `bus << 8 | slot << 3 | func`) to a page table of AMD's own format (a
//! present bit, the next level in bits 11:9, read and write at the top).
//! Every function starts in passthrough: an entry whose mode is 0
//! translates nothing, so a kernel driver's device works as before and no
//! identity table is needed. [`attach`] gives a function a 4-level table
//! of its own and a domain id; [`map`] puts pages in it one-to-one, so a
//! bus address stays a physical one.
//!
//! The unit takes commands from a ring in memory (section 2.4): invalidate
//! a device table entry, a domain's pages, an interrupt table, everything;
//! a completion wait writes a word when the ones before it are done. Its
//! event log (another ring) gets the refused DMAs; the log's interrupt is
//! the MSI of the unit's own PCI function (claimed by the kernel), and the
//! handler hands each entry to `iommu::record`.
//!
//! Interrupts (section 2.2.5): a device's message carries an index in its
//! data (bits 10:0) into the interrupt remapping table its device table
//! entry names, whose entry holds the vector and the destination. Every
//! device gets a table of its own ([`remap`]), so a function raises its own
//! vectors and no other's; a device without one has its interrupts aborted
//! (`IntCtl` 0), and the I/O APIC, a device of the table's special kind, is
//! remapped the same way. The unit's own interrupt is not.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use spin::Mutex;

use super::Fault;
use super::x86::Source;
use crate::mm;

// The registers (section 3.4), an MMIO block of 16 KiB.
const DEV_TABLE_BASE: usize = 0x0000;
const CMD_BASE: usize = 0x0008;
const EVT_BASE: usize = 0x0010;
const CONTROL: usize = 0x0018;
const EFR: usize = 0x0030;
const CMD_HEAD: usize = 0x2000;
const CMD_TAIL: usize = 0x2008;
const EVT_HEAD: usize = 0x2010;
const EVT_TAIL: usize = 0x2018;
const STATUS: usize = 0x2020;
const MMIO_SIZE: u64 = 0x4000;
/// Control: translation on, the event log and its interrupt, the command
/// buffer.
const CTRL_EN: u64 = 1;
const CTRL_EVTLOG_EN: u64 = 1 << 2;
const CTRL_EVTINT_EN: u64 = 1 << 3;
const CTRL_CMDBUF_EN: u64 = 1 << 12;
/// Status: the event log overflowed, has entries, a completion wait's
/// interrupt; written with 1 to clear.
const STATUS_CLEAR: u64 = 0b111;
/// The unit takes `INVALIDATE_IOMMU_ALL` (`EFR.IASup`).
const EFR_IA: u64 = 1 << 6;
/// The command buffer and the event log: 256 entries of 16 bytes, one page
/// each (the length, log2, in bits 59:56 of the base register; the head
/// and tail registers hold byte offsets).
const RING_LOG2: u64 = 8;
const RING_ENTRIES: u32 = 256;
const ENTRY: u64 = 16;
const RING_MASK: u64 = 0x7FFF0;
// Commands: the opcode in bits 63:60 of the first word.
const CMD_COMPLETION_WAIT: u64 = 1 << 60;
const CMD_INVALIDATE_DTE: u64 = 2 << 60;
const CMD_INVALIDATE_PAGES: u64 = 3 << 60;
const CMD_INVALIDATE_IRT: u64 = 5 << 60;
const CMD_INVALIDATE_ALL: u64 = 8 << 60;
/// A completion wait that stores its second word at the address in its
/// first (`S`).
const WAIT_STORE: u64 = 1;
/// `INVALIDATE_IOMMU_PAGES`' second word for a domain whole: every page
/// (`S` with the address all ones) and the directory entries (`PDE`).
const INVALIDATE_PAGES_ALL: u64 = 0x7FFF_FFFF_FFFF_F000 | 0b11;
// A device table entry (section 2.2.2), four words. The first: valid,
// translation valid, the mode (page table levels; 0 translates nothing)
// in bits 11:9, the page table root, read and write. The second: the
// domain id. The third: the interrupt table valid, its length as log2 in
// bits 4:1, its address from bit 6, `IntCtl` in bits 61:60 (0 aborts the
// device's interrupts, 2 remaps them).
const DTE_V: u64 = 1;
const DTE_TV: u64 = 1 << 1;
const DTE_MODE_SHIFT: u32 = 9;
const DTE_IR: u64 = 1 << 61;
const DTE_IW: u64 = 1 << 62;
const DTE_IV: u64 = 1;
const DTE_INT_LEN_SHIFT: u32 = 1;
const DTE_INTCTL_REMAP: u64 = 2 << 60;
// A page table entry (section 2.2.6): present, the next level in bits 11:9
// (0: a page), the address, read and write.
const PTE_PR: u64 = 1;
const PTE_LEVEL_SHIFT: u32 = 9;
const PTE_ADDR: u64 = 0x000F_FFFF_FFFF_F000;
const PTE_IR: u64 = 1 << 61;
const PTE_IW: u64 = 1 << 62;
/// A held function's table: four levels (mode 4, 48-bit addresses), which
/// every unit walks.
const LEVELS: u8 = 4;
/// Events (section 2.5): the code in bits 63:60 of the first word, the
/// device id in its bits 15:0, the address in the second. An I/O page
/// fault (code 2) flags a write in bit 53 (QEMU puts the permission asked
/// at bit 49 instead).
const EVT_CODE_SHIFT: u32 = 60;
const EVT_WRITE: u64 = 1 << 53 | 1 << 49;
/// Interrupt remapping tables: 256 entries of 4 bytes per device
/// (`IntTabLen` 8), each table in a frame of its own. An entry: enabled,
/// the destination APIC id in bits 15:8 (fixed delivery, physical), the
/// vector in bits 23:16.
const IRT_ENTRIES: u16 = 256;
const IRT_LEN_LOG2: u64 = 8;
const IRTE_EN: u32 = 1;

/// Passthrough's domain id (every function's at first).
const IDENTITY_DID: u16 = 1;
/// The most units a board has; the event handler finds its unit here.
const MAX_UNITS: usize = 4;
/// How long a command may take (a spin count).
const SPINS: u32 = 10_000_000;
const PAGE: u64 = 4096;

struct Unit {
    base: u64,
    /// The registers, mapped.
    regs: usize,
    /// The unit's own PCI function's device id.
    devid: u16,
    /// Translates for every function (an `all` entry), or for the device
    /// ids in these ranges.
    all: bool,
    ranges: Vec<(u16, u16)>,
    /// The I/O APIC's device id (the special entry of that variety).
    ioapic: Option<u16>,
    /// The device table (physical) and its entries: the buses with a
    /// function on them, whole.
    dev_table: u64,
    dev_entries: usize,
    /// The command buffer, the page after it taking the completion waits'
    /// words, and its tail index.
    cmd: u64,
    cmd_tail: AtomicU32,
    /// The event log.
    evt: u64,
    /// The unit takes `INVALIDATE_IOMMU_ALL`.
    inv_all: bool,
}

/// A device's interrupt remapping table and the entries given out.
struct IrTable {
    devid: u16,
    table: u64,
    used: u16,
}

/// A function's own domain.
struct Domain {
    id: u16,
    /// Its page table's root.
    root: u64,
    dev: (u8, u8, u8),
    pages: usize,
}

struct State {
    units: Vec<Unit>,
    domains: Vec<Domain>,
    irts: Vec<IrTable>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Per unit, for the event handler (no lock there): the registers' address
/// and the event log's.
static IRQ_REGS: [AtomicUsize; MAX_UNITS] = [const { AtomicUsize::new(0) }; MAX_UNITS];
static IRQ_EVT: [AtomicU64; MAX_UNITS] = [const { AtomicU64::new(0) }; MAX_UNITS];

fn read64(va: usize) -> u64 {
    unsafe { core::ptr::read_volatile(va as *const u64) }
}

fn write64(va: usize, v: u64) {
    unsafe { core::ptr::write_volatile(va as *mut u64, v) }
}

fn devid(bus: u8, slot: u8, func: u8) -> u16 {
    u16::from(bus) << 8 | u16::from(slot) << 3 | u16::from(func)
}

/// The device table entry of `devid` (its first word), when the table has
/// it.
fn dte(u: &Unit, devid: u16) -> Option<*mut u64> {
    (usize::from(devid) < u.dev_entries).then(|| unsafe { (mm::hhdm(u.dev_table) as *mut u64).add(usize::from(devid) * 4) })
}

/// Queue `cmds` and a completion wait after them: true once the unit has
/// done them (the wait's word written), false when it has not within
/// [`SPINS`].
fn submit(u: &Unit, cmds: &[[u64; 2]]) -> bool {
    let ring = mm::hhdm(u.cmd) as *mut [u64; 2];
    let word = u.cmd + PAGE;
    let mut tail = u.cmd_tail.load(Ordering::Relaxed);
    // The word the wait stores: never 0, which it is cleared to first.
    let seq = u64::from(tail) + 1;
    unsafe { (mm::hhdm(word) as *mut u64).write_volatile(0) };
    let wait = [CMD_COMPLETION_WAIT | WAIT_STORE | word, seq];
    for c in cmds.iter().chain(core::iter::once(&wait)) {
        unsafe { ring.add(tail as usize).write_volatile(*c) };
        tail = (tail + 1) % RING_ENTRIES;
    }
    u.cmd_tail.store(tail, Ordering::Relaxed);
    core::sync::atomic::fence(Ordering::SeqCst);
    write64(u.regs + CMD_TAIL, u64::from(tail) * ENTRY);
    for _ in 0..SPINS {
        if unsafe { (mm::hhdm(word) as *const u64).read_volatile() } == seq {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// A device table entry's first word for passthrough: valid, translation
/// valid with mode 0 (nothing walked), read and write.
const fn passthrough() -> u64 {
    DTE_V | DTE_TV | DTE_IR | DTE_IW
}

/// Rewrite `devid`'s entry (not valid while its words change) and tell the
/// unit: the entry it cached, the pages of the domains `dids` the device
/// leaves and joins, everything where it takes that.
fn set_dte(u: &Unit, devid: u16, q0: u64, q1: u64, q2: u64, dids: &[u16]) -> bool {
    let Some(e) = dte(u, devid) else {
        return false;
    };
    unsafe {
        e.write_volatile(0);
        e.add(1).write_volatile(q1);
        e.add(2).write_volatile(q2);
        e.add(3).write_volatile(0);
        e.write_volatile(q0);
    }
    let mut cmds = alloc::vec![[CMD_INVALIDATE_DTE | u64::from(devid), 0]];
    for &d in dids {
        cmds.push([CMD_INVALIDATE_PAGES | u64::from(d) << 32, INVALIDATE_PAGES_ALL]);
    }
    if u.inv_all {
        cmds.push([CMD_INVALIDATE_ALL, 0]);
    }
    submit(u, &cmds)
}

/// The unit's event log since the handler last read it: each entry a
/// refused DMA (or a table the unit could not read), handed over.
unsafe extern "C" fn event_handler(ctx: *mut core::ffi::c_void) {
    let i = ctx as usize;
    if i >= MAX_UNITS {
        return;
    }
    let (regs, evt) = (IRQ_REGS[i].load(Ordering::Relaxed), IRQ_EVT[i].load(Ordering::Relaxed));
    if regs == 0 {
        return;
    }
    let tail = read64(regs + EVT_TAIL) & RING_MASK;
    let mut head = read64(regs + EVT_HEAD) & RING_MASK;
    while head != tail {
        let e = mm::hhdm(evt + head) as *const u64;
        let (w0, addr) = unsafe { (e.read_volatile(), e.add(1).read_volatile()) };
        let code = (w0 >> EVT_CODE_SHIFT) as u8;
        if code != 0 {
            super::record(Fault { sid: w0 as u16, addr, reason: code, read: w0 & EVT_WRITE == 0 });
        }
        head = (head + ENTRY) % (u64::from(RING_ENTRIES) * ENTRY);
    }
    write64(regs + EVT_HEAD, head);
    write64(regs + STATUS, read64(regs + STATUS) & STATUS_CLEAR);
}

/// The index into a table at `level` (1 the 4 KiB leaves).
fn index(iova: u64, level: u8) -> usize {
    ((iova >> (12 + 9 * (level as u64 - 1))) & 0x1ff) as usize
}

/// The leaf entry for `iova` in the table at `root`, the tables above it
/// made when `create`; `None` when there is none to make.
fn leaf(root: u64, iova: u64, create: bool) -> Option<*mut u64> {
    let mut table = root;
    for level in (2..=LEVELS).rev() {
        let e = unsafe { (mm::table(table) as *mut u64).add(index(iova, level)) };
        let v = unsafe { e.read_volatile() };
        if v & PTE_PR != 0 {
            table = v & PTE_ADDR;
        } else {
            if !create {
                return None;
            }
            table = mm::alloc_frame();
            unsafe { e.write_volatile(table | PTE_PR | u64::from(level - 1) << PTE_LEVEL_SHIFT | PTE_IR | PTE_IW) };
        }
    }
    Some(unsafe { (mm::table(table) as *mut u64).add(index(iova, 1)) })
}

/// Give the table at `table` (`level` deep) and every table under it back.
fn free_table(table: u64, level: u8) {
    if level > 1 {
        let t = mm::table(table);
        for i in 0..512 {
            let v = unsafe { (*t)[i] };
            if v & PTE_PR != 0 {
                free_table(v & PTE_ADDR, level - 1);
            }
        }
    }
    mm::free_frame(table);
}

/// The unit that translates for `devid`: the one whose entries name it,
/// else the one for every function.
fn unit_for(st: &State, devid: u16) -> Option<usize> {
    st.units
        .iter()
        .position(|u| u.ranges.iter().any(|&(a, b)| a <= devid && devid <= b))
        .or_else(|| st.units.iter().position(|u| u.all))
}

/// Find the units, put every device in passthrough with its interrupts
/// aborted, start the command buffer, the event log and translation.
pub fn init() {
    let Some(ivrs) = crate::acpi::ivrs() else {
        return;
    };
    let mut units: Vec<Unit> = Vec::new();
    for h in ivrs.units() {
        // A unit described in more than one block (one per type): the
        // first block's.
        if units.len() == MAX_UNITS || units.iter().any(|u| u.base == h.base) {
            continue;
        }
        let Some(regs) = crate::arch::pci::map_mmio(h.base, MMIO_SIZE) else {
            crate::console::status_warn(&alloc::format!("iommu: amd-vi unit at 0x{:x}: cannot map", h.base));
            continue;
        };
        let (mut all, mut ranges, mut ioapic) = (false, Vec::new(), None);
        for e in h.entries() {
            match e {
                crate::acpi::IvhdEntry::All => all = true,
                crate::acpi::IvhdEntry::Select(d) => ranges.push((d, d)),
                crate::acpi::IvhdEntry::Range(a, b) => ranges.push((a, b)),
                crate::acpi::IvhdEntry::Special { variety: 1, devid } => ioapic = Some(devid),
                crate::acpi::IvhdEntry::Special { .. } => {}
            }
        }
        units.push(Unit {
            base: h.base,
            regs,
            devid: h.devid,
            all,
            ranges,
            ioapic,
            dev_table: 0,
            dev_entries: 0,
            cmd: 0,
            cmd_tail: AtomicU32::new(0),
            evt: 0,
            inv_all: read64(regs + EFR) & EFR_IA != 0,
        });
    }
    if units.is_empty() {
        return;
    }
    // The device table: every id of the buses with a function on them
    // (the ones there and the ones a later rescan may find).
    let max_bus = crate::pci::functions().iter().map(|f| f.bus).max().unwrap_or(0);
    let entries = (usize::from(max_bus) + 1) * 256;
    let pages = (entries * 32).div_ceil(PAGE as usize);
    let mut st = State { units: Vec::new(), domains: Vec::new(), irts: Vec::new() };
    for (i, mut u) in units.into_iter().enumerate() {
        let (Some((table, _)), Some((cmd, _)), Some((evt, _))) = (
            mm::alloc_contiguous_pages(pages, mm::ANYWHERE),
            mm::alloc_contiguous_pages(2, mm::ANYWHERE),
            mm::alloc_contiguous_pages(1, mm::ANYWHERE),
        ) else {
            crate::console::status_warn(&alloc::format!("iommu: amd-vi unit at 0x{:x}: out of memory, left off", u.base));
            continue;
        };
        (u.dev_table, u.dev_entries, u.cmd, u.evt) = (table, entries, cmd, evt);
        let t = mm::hhdm(table) as *mut u64;
        for d in 0..entries {
            unsafe {
                t.add(d * 4).write_volatile(passthrough());
                t.add(d * 4 + 1).write_volatile(u64::from(IDENTITY_DID));
                t.add(d * 4 + 2).write_volatile(DTE_IV);
                t.add(d * 4 + 3).write_volatile(0);
            }
        }
        core::sync::atomic::fence(Ordering::SeqCst);
        // Off while the tables are named.
        write64(u.regs + CONTROL, 0);
        write64(u.regs + DEV_TABLE_BASE, table | (pages as u64 - 1));
        write64(u.regs + CMD_BASE, cmd | RING_LOG2 << 56);
        write64(u.regs + EVT_BASE, evt | RING_LOG2 << 56);
        for r in [CMD_HEAD, CMD_TAIL, EVT_HEAD, EVT_TAIL] {
            write64(u.regs + r, 0);
        }
        // The event log's interrupt: the unit's own function's MSI. The
        // function is the kernel's: a program must not take it through
        // `/dev/pci`.
        let (bus, slot, func) = ((u.devid >> 8) as u8, (u.devid >> 3 & 0x1f) as u8, (u.devid & 7) as u8);
        crate::pci::claim(bus, slot, func, b"iommu");
        crate::pci::enable(bus, slot, func);
        IRQ_REGS[i].store(u.regs, Ordering::Relaxed);
        IRQ_EVT[i].store(evt, Ordering::Relaxed);
        if let Some(irq) = crate::arch::pci_irq_setup(bus, slot, func) {
            crate::irq::register(irq.irq, "iommu", event_handler, i);
        }
        write64(u.regs + CONTROL, CTRL_CMDBUF_EN | CTRL_EVTLOG_EN | CTRL_EVTINT_EN | CTRL_EN);
        let whole: &[[u64; 2]] = if u.inv_all { &[[CMD_INVALIDATE_ALL, 0]] } else { &[] };
        if !submit(&u, whole) {
            crate::console::status_warn(&alloc::format!("iommu: amd-vi unit at 0x{:x}: takes no command, left off", u.base));
            write64(u.regs + CONTROL, 0);
            continue;
        }
        st.units.push(u);
    }
    if st.units.is_empty() {
        return;
    }
    crate::console::status_ok(&alloc::format!(
        "iommu: amd-vi, {} unit(s), {} device ids, {}-level tables, interrupts remapped",
        st.units.len(),
        entries,
        LEVELS
    ));
    *STATE.lock() = Some(st);
    // The I/O APIC pins routed before this (the console's) through the
    // I/O APIC's table now.
    crate::arch::ioapic_refresh();
}

/// Whether a unit is driven.
pub fn active() -> bool {
    STATE.lock().is_some()
}

/// An entry in `source`'s interrupt remapping table (made at its first
/// interrupt) that raises `vector` on the BSP: its index, which the
/// message's data carries. `None` when no unit remaps for the source, for
/// the unit's own function (its message goes as it is), or the table is
/// full.
pub fn remap(source: Source, vector: u8) -> Option<u16> {
    let mut guard = STATE.lock();
    let st = guard.as_mut()?;
    let (ui, dev) = match source {
        Source::Pci(bus, slot, func) => {
            let d = devid(bus, slot, func);
            (unit_for(st, d)?, d)
        }
        Source::IoApic => {
            let ui = st.units.iter().position(|u| u.ioapic.is_some())?;
            (ui, st.units[ui].ioapic?)
        }
    };
    let u = &st.units[ui];
    if dev == u.devid {
        return None;
    }
    // The entry names the BSP in 8 bits.
    let apic = crate::smp::cpu_hw_id(0);
    if apic > 0xff {
        return None;
    }
    let ti = match st.irts.iter().position(|t| t.devid == dev) {
        Some(i) => i,
        None => {
            let e = dte(u, dev)?;
            let table = mm::alloc_frame();
            unsafe { e.add(2).write_volatile(DTE_IV | IRT_LEN_LOG2 << DTE_INT_LEN_SHIFT | table | DTE_INTCTL_REMAP) };
            core::sync::atomic::fence(Ordering::SeqCst);
            submit(u, &[[CMD_INVALIDATE_DTE | u64::from(dev), 0]]);
            st.irts.push(IrTable { devid: dev, table, used: 0 });
            st.irts.len() - 1
        }
    };
    let t = &mut st.irts[ti];
    if t.used >= IRT_ENTRIES {
        return None;
    }
    let idx = t.used;
    t.used += 1;
    unsafe { (mm::hhdm(t.table) as *mut u32).add(usize::from(idx)).write_volatile(IRTE_EN | (apic as u32 & 0xff) << 8 | u32::from(vector) << 16) };
    core::sync::atomic::fence(Ordering::SeqCst);
    submit(u, &[[CMD_INVALIDATE_IRT | u64::from(dev), 0]]);
    Some(idx)
}

pub fn attach(bus: u8, slot: u8, func: u8) -> Option<u32> {
    let mut guard = STATE.lock();
    let st = guard.as_mut()?;
    let dev = (bus, slot, func);
    if let Some(d) = st.domains.iter().find(|d| d.dev == dev) {
        return Some(u32::from(d.id));
    }
    let id_dev = devid(bus, slot, func);
    let u = &st.units[unit_for(st, id_dev)?];
    let e = dte(u, id_dev)?;
    let id = (IDENTITY_DID + 1..=u16::MAX).find(|id| !st.domains.iter().any(|d| d.id == *id))?;
    let root = mm::alloc_frame();
    // The interrupt part of the entry stays.
    let q2 = unsafe { e.add(2).read_volatile() };
    let q0 = DTE_V | DTE_TV | u64::from(LEVELS) << DTE_MODE_SHIFT | root | DTE_IR | DTE_IW;
    if !set_dte(u, id_dev, q0, u64::from(id), q2, &[IDENTITY_DID, id]) {
        mm::free_frame(root);
        return None;
    }
    st.domains.push(Domain { id, root, dev, pages: 0 });
    Some(u32::from(id))
}

pub fn map(domain: u32, phys: u64, len: usize) -> bool {
    let mut guard = STATE.lock();
    let Some(st) = guard.as_mut() else {
        return false;
    };
    let Some(i) = st.domains.iter().position(|d| u32::from(d.id) == domain) else {
        return false;
    };
    let (root, dev, id) = (st.domains[i].root, st.domains[i].dev, st.domains[i].id);
    let (phys, len) = (phys & !0xfff, (len as u64 + (phys & 0xfff) + 0xfff) & !0xfff);
    if len == 0 || phys + len > 1 << (12 + 9 * u64::from(LEVELS)) {
        return false;
    }
    let mut a = phys;
    while a < phys + len {
        let Some(e) = leaf(root, a, true) else {
            return false;
        };
        unsafe { e.write_volatile(a | PTE_PR | PTE_IR | PTE_IW) };
        a += PAGE;
    }
    core::sync::atomic::fence(Ordering::SeqCst);
    st.domains[i].pages += (len / PAGE) as usize;
    if let Some(ui) = unit_for(st, devid(dev.0, dev.1, dev.2)) {
        submit(&st.units[ui], &[[CMD_INVALIDATE_PAGES | u64::from(id) << 32, INVALIDATE_PAGES_ALL]]);
    }
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
    let id_dev = devid(bus, slot, func);
    if let Some(ui) = unit_for(st, id_dev)
        && let Some(e) = dte(&st.units[ui], id_dev)
    {
        let q2 = unsafe { e.add(2).read_volatile() };
        set_dte(&st.units[ui], id_dev, passthrough(), u64::from(IDENTITY_DID), q2, &[d.id, IDENTITY_DID]);
    }
    free_table(d.root, LEVELS);
}

pub fn text() -> String {
    let mut s = String::new();
    let guard = STATE.lock();
    let Some(st) = guard.as_ref() else {
        s.push_str("none\n");
        return s;
    };
    for u in &st.units {
        let tables = st.irts.len();
        let _ = writeln!(
            s,
            "amdvi 0x{:x} unit {:02x}:{:02x}.{} devices {} levels {}{} intremap {} table(s)",
            u.base,
            u.devid >> 8,
            (u.devid >> 3) & 0x1f,
            u.devid & 7,
            u.dev_entries,
            LEVELS,
            if u.all { " all" } else { "" },
            tables
        );
    }
    let _ = writeln!(s, "domain {} passthrough", IDENTITY_DID);
    for d in &st.domains {
        let _ = writeln!(s, "domain {} {:02x}:{:02x}.{} pages {}", d.id, d.dev.0, d.dev.1, d.dev.2, d.pages);
    }
    s
}
