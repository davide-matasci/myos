//! Arm SMMUv3, the aarch64 unit (Arm System Memory Management Unit
//! Architecture Specification, version 3): the one the platform describes
//! (`platform::Iommu`: the IORT's SMMUv3 node and its root complex's ID
//! mappings, or the tree's `arm,smmu-v3` node and the PCIe host's
//! `iommu-map`), in front of the PCIe host's functions, each a stream
//! (`StreamID` = its requester id, moved by the mapping).
//!
//! The unit walks a linear stream table, one 64-byte entry per stream, to
//! the stream's configuration: *bypass* (the identity domain: a kernel
//! driver's device sees memory as it is, no table walked) or *stage-2
//! translation* through a VMSAv8 stage-2 page table of the domain's (4 KiB
//! granule; the start level and the input size from the unit's output
//! size). Stage 1 is never used: there are no processes behind the
//! devices. [`attach`] rewrites a function's entry for a table of its own;
//! [`map`] puts pages in it one-to-one, so a bus address stays a physical
//! one.
//!
//! The unit takes commands from a queue in memory (invalidate an entry, a
//! domain's translations, a sync) and reports refused DMAs through an
//! event queue, whose interrupt (edge-triggered, as the SMMU pulses it)
//! this driver drains into `iommu::record`. A unit whose walks do not
//! snoop the caches (`IDR0.COHACC` clear, unless the IORT overrides it)
//! has its tables and queues written back after each write.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use spin::Mutex;

use super::Fault;
use crate::mm;

// The registers (chapter 6), page 0 and page 1 (the queues' indices).
const IDR0: usize = 0x00;
const IDR1: usize = 0x04;
const IDR5: usize = 0x14;
const CR0: usize = 0x20;
const CR0ACK: usize = 0x24;
const CR1: usize = 0x28;
const CR2: usize = 0x2C;
const IRQ_CTRL: usize = 0x50;
const IRQ_CTRLACK: usize = 0x54;
const GERROR: usize = 0x60;
const GERRORN: usize = 0x64;
const STRTAB_BASE: usize = 0x80;
const STRTAB_BASE_CFG: usize = 0x88;
const CMDQ_BASE: usize = 0x90;
const CMDQ_PROD: usize = 0x98;
const CMDQ_CONS: usize = 0x9C;
const EVENTQ_BASE: usize = 0xA0;
const EVENTQ_PROD: usize = 0x100A8;
const EVENTQ_CONS: usize = 0x100AC;

const CR0_SMMUEN: u32 = 1;
const CR0_EVENTQEN: u32 = 1 << 2;
const CR0_CMDQEN: u32 = 1 << 3;
/// Queues and tables inner shareable, write-back cacheable.
const CR1_CACHED: u32 = 0xD75;
/// Invalidations by stream id, private TLB maintenance.
const CR2_SETUP: u32 = 0x6;
const IRQ_GERROR: u32 = 1;
const IRQ_EVENTQ: u32 = 1 << 2;
/// A queue's or table's base: read-allocate hint.
const RA: u64 = 1 << 62;

/// Commands (chapter 4): the opcode in word 0.
const CMD_CFGI_STE: u32 = 0x03;
const CMD_CFGI_ALL: u32 = 0x04;
const CMD_TLBI_S12_VMALL: u32 = 0x28;
const CMD_TLBI_NSNH_ALL: u32 = 0x30;
const CMD_SYNC: u32 = 0x46;

/// Stream table entry word 0: valid, and the configuration in bits 1-3
/// (`0b100` bypass, `0b110` stage 2 only).
const STE_VALID: u32 = 1;
const STE_BYPASS: u32 = 0b100 << 1;
const STE_S2: u32 = 0b110 << 1;
/// Word 1: the shareability of a bypassed stream's accesses as they came.
const STE_SHCFG_INCOMING: u32 = 1 << 12;

/// Stage-2 descriptors (VMSAv8-64): the memory attributes of a block or
/// page (normal write-back, read and write, inner shareable, accessed).
const S2_TABLE: u64 = 0b11;
const S2_PAGE: u64 = 0b11;
const S2_ATTRS: u64 = (0b1111 << 2) | (0b11 << 6) | (0b11 << 8) | (1 << 10);
const S2_ADDR: u64 = 0x0000_FFFF_FFFF_F000;

const PAGE: u64 = 4096;
/// The identity domain's VMID; the functions programs drive get the next.
const IDENTITY_VMID: u16 = 1;
/// How long a command or an enable may take (a spin count).
const SPINS: u32 = 10_000_000;
/// The queues: 256 commands (one page), 128 events (one page).
const CMDQ_LOG2: u32 = 8;
const EVENTQ_LOG2: u32 = 7;

struct Domain {
    vmid: u16,
    /// Its stage-2 table's root.
    root: u64,
    dev: (u8, u8, u8),
    pages: usize,
}

struct State {
    regs: usize,
    base: u64,
    /// The stream table and how many entries it has (a power of two).
    strtab: u64,
    entries: u32,
    /// Requester ids `[rid_base, rid_base + rid_count)` are streams from
    /// `sid_base`.
    rid_base: u32,
    rid_count: u32,
    sid_base: u32,
    /// The command queue: its entries, and the producer index with its
    /// wrap bit as `CMDQ_PROD` takes it.
    cmdq: *mut [u32; 4],
    cmdq_prod: u32,
    /// The stage-2 walk: its start level (0 or 1), input and output sizes.
    start_level: u8,
    ipa_bits: u8,
    oas: u8,
    domains: Vec<Domain>,
    max_vmid: u16,
    /// The unit snoops the caches on its walks: no flushing.
    coherent: bool,
}

unsafe impl Send for State {}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// For the event queue's handler (no lock there): the registers, the
/// queue's entries, its consumer index with its wrap bit, and the global
/// errors seen.
static IRQ_REGS: AtomicUsize = AtomicUsize::new(0);
static IRQ_QUEUE: AtomicUsize = AtomicUsize::new(0);
static IRQ_EVENTQ_CONS: AtomicU32 = AtomicU32::new(0);
static GERRORS: AtomicU32 = AtomicU32::new(0);

fn read32(va: usize) -> u32 {
    unsafe { core::ptr::read_volatile(va as *const u32) }
}

fn write32(va: usize, v: u32) {
    unsafe { core::ptr::write_volatile(va as *mut u32, v) }
}

fn write64(va: usize, v: u64) {
    unsafe { core::ptr::write_volatile(va as *mut u64, v) }
}

/// What was written to memory for the unit reaches it: written back when
/// it does not snoop, ordered before the register write that follows.
fn written(coherent: bool, va: usize, len: usize) {
    if !coherent {
        crate::dma::cache::clean(va, len);
    }
    crate::dma::cache::barrier();
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

/// The next index of a queue of `1 << log2` entries, the wrap bit above.
fn next(index: u32, log2: u32) -> u32 {
    (index + 1) & ((2 << log2) - 1)
}

/// Queue `words` for the unit and wait for it to take them (false when it
/// did not, or reported an error in `CMDQ_CONS`).
fn command(st: &mut State, words: [u32; 4]) -> bool {
    let i = (st.cmdq_prod & ((1 << CMDQ_LOG2) - 1)) as usize;
    unsafe { st.cmdq.add(i).write_volatile(words) };
    written(st.coherent, unsafe { st.cmdq.add(i) } as usize, 16);
    st.cmdq_prod = next(st.cmdq_prod, CMDQ_LOG2);
    write32(st.regs + CMDQ_PROD, st.cmdq_prod);
    for _ in 0..SPINS {
        let cons = read32(st.regs + CMDQ_CONS);
        if cons >> 24 & 0x7f != 0 {
            return false;
        }
        if cons & ((2 << CMDQ_LOG2) - 1) == st.cmdq_prod {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// The unit forgets stream `sid`'s entry and VMID `vmid`'s translations.
fn invalidate(st: &mut State, sid: u32, vmid: u16) {
    command(st, [CMD_CFGI_STE, sid, 1, 0]);
    command(st, [CMD_TLBI_S12_VMALL, u32::from(vmid), 0, 0]);
    command(st, [CMD_SYNC, 0, 0, 0]);
}

/// The event queue's interrupt: each refused DMA recorded, the queue
/// consumed. Also the global error interrupt (`ctx` 1): acknowledged and
/// counted.
unsafe extern "C" fn interrupt(ctx: *mut core::ffi::c_void) {
    let regs = IRQ_REGS.load(Ordering::Relaxed);
    if regs == 0 {
        return;
    }
    if ctx as usize == 1 {
        let gerror = read32(regs + GERROR);
        GERRORS.fetch_add(1, Ordering::Relaxed);
        write32(regs + GERRORN, gerror);
        return;
    }
    let queue = IRQ_QUEUE.load(Ordering::Relaxed) as *const [u32; 8];
    let mask = (2 << EVENTQ_LOG2) - 1;
    let prod = read32(regs + EVENTQ_PROD) & mask;
    let mut cons = IRQ_EVENTQ_CONS.load(Ordering::Relaxed);
    while cons != prod {
        let e = unsafe { queue.add((cons & ((1 << EVENTQ_LOG2) - 1)) as usize).read_volatile() };
        super::record(Fault {
            sid: e[1] as u16,
            addr: (u64::from(e[5]) << 32 | u64::from(e[4])) & !0xfff,
            reason: e[0] as u8,
            read: e[3] & (1 << 3) != 0,
        });
        cons = next(cons, EVENTQ_LOG2);
    }
    IRQ_EVENTQ_CONS.store(cons, Ordering::Relaxed);
    write32(regs + EVENTQ_CONS, cons);
}

/// The index into the table at `level` (0 the 512 GiB entries, 1 the
/// 1 GiB, 2 the 2 MiB, 3 the 4 KiB pages).
fn index(iova: u64, level: u8) -> usize {
    ((iova >> (39 - 9 * level as u64)) & 0x1ff) as usize
}

/// The entry for `iova` at `leaf` in the table rooted at `root` (starting
/// at `start`), the tables above it made when `create`.
fn entry(root: u64, start: u8, iova: u64, leaf: u8, create: bool) -> Option<*mut u64> {
    let mut table = root;
    let mut level = start;
    loop {
        let e = unsafe { (mm::table(table) as *mut u64).add(index(iova, level)) };
        if level == leaf {
            return Some(e);
        }
        let v = unsafe { e.read_volatile() };
        if v & 0b11 == S2_TABLE {
            table = v & S2_ADDR;
        } else if v & 1 != 0 {
            return None;
        } else {
            if !create {
                return None;
            }
            table = mm::alloc_frame();
            unsafe { e.write_volatile(table | S2_TABLE) };
        }
        level += 1;
    }
}

/// Map the pages of `[phys, phys + len)` one-to-one into the table at
/// `root` (`phys` and `len` whole pages).
fn map_pages(st: &State, root: u64, phys: u64, len: u64) -> bool {
    let mut a = phys;
    while a < phys + len {
        let Some(e) = entry(root, st.start_level, a, 3, true) else {
            return false;
        };
        unsafe { e.write_volatile(a | S2_ATTRS | S2_PAGE) };
        a += PAGE;
    }
    if !st.coherent {
        walk(root, st.start_level, false);
    }
    crate::dma::cache::barrier();
    true
}

/// Walk the table at `root`: flush every table's lines (`free` false), or
/// give every table's frame back, the root's included (`free` true).
fn walk(table: u64, level: u8, free: bool) {
    let t = mm::table(table);
    if level < 3 {
        for i in 0..512 {
            let v = unsafe { (*t)[i] };
            if v & 0b11 == S2_TABLE {
                walk(v & S2_ADDR, level + 1, free);
            }
        }
    }
    if free {
        mm::free_frame(table);
    } else {
        crate::dma::cache::clean(t as usize, 4096);
    }
}

/// The stream id of `bus:slot.func`, when the unit translates for it.
fn sid_of(st: &State, bus: u8, slot: u8, func: u8) -> Option<u32> {
    let rid = u32::from(bus) << 8 | u32::from(slot) << 3 | u32::from(func);
    if rid < st.rid_base || rid - st.rid_base >= st.rid_count {
        return None;
    }
    let sid = st.sid_base + (rid - st.rid_base);
    (sid < st.entries).then_some(sid)
}

fn ste(st: &State, sid: u32) -> *mut [u32; 16] {
    unsafe { (mm::hhdm(st.strtab) as *mut [u32; 16]).add(sid as usize) }
}

/// Write stream `sid`'s entry: invalid first, the words, then word 0 with
/// the configuration, each step reaching the unit before the next.
fn set_ste(st: &State, sid: u32, words: [u32; 16]) {
    let e = ste(st, sid) as *mut u32;
    unsafe {
        e.write_volatile(0);
        written(st.coherent, e as usize, 4);
        for (i, w) in words.iter().enumerate().skip(1) {
            e.add(i).write_volatile(*w);
        }
        written(st.coherent, e as usize, 64);
        e.write_volatile(words[0]);
        written(st.coherent, e as usize, 4);
    }
}

/// A bypass entry: the stream's accesses go to memory as they are.
fn ste_bypass() -> [u32; 16] {
    let mut w = [0u32; 16];
    w[0] = STE_VALID | STE_BYPASS;
    w[1] = STE_SHCFG_INCOMING;
    w
}

/// A stage-2 entry: the stream's accesses translated through the table at
/// `root`, tagged `vmid`, faults recorded.
fn ste_s2(st: &State, root: u64, vmid: u16) -> [u32; 16] {
    let mut w = [0u32; 16];
    w[0] = STE_VALID | STE_S2;
    w[1] = STE_SHCFG_INCOMING;
    w[4] = u32::from(vmid);
    let t0sz = 64 - u32::from(st.ipa_bits);
    let sl0 = if st.start_level == 0 { 2 } else { 1 };
    // S2T0SZ, S2SL0, S2IR0 and S2OR0 write-back, S2SH0 inner shareable,
    // S2TG 4 KiB, S2PS, S2AA64, S2R (record faults).
    w[5] = t0sz | sl0 << 6 | 1 << 8 | 1 << 10 | 3 << 12 | u32::from(st.oas) << 16 | 1 << 19 | 1 << 26;
    w[6] = root as u32;
    w[7] = (root >> 32) as u32;
    w
}

/// Find the unit, put every stream in bypass, start it.
pub fn init() {
    let Some(c) = crate::platform::get().iommu else {
        return;
    };
    let unit = c.value;
    let Some(regs) = crate::arch::pci::map_mmio(unit.base, unit.size.max(0x20000)) else {
        crate::console::status_warn(&alloc::format!("iommu: smmuv3 at 0x{:x}: cannot map", unit.base));
        return;
    };
    let (idr0, idr1, idr5) = (read32(regs + IDR0), read32(regs + IDR1), read32(regs + IDR5));
    // Stage 2 with AArch64 tables and 4 KiB granules, a linear stream table.
    if idr0 & 1 == 0 || idr0 >> 2 & 0b11 == 0b01 || idr5 & (1 << 4) == 0 {
        crate::console::status_warn("iommu: smmuv3 without stage 2, AArch64 tables or 4 KiB granules, left off");
        return;
    }
    if idr1 & (1 << 29 | 1 << 30) != 0 {
        crate::console::status_warn("iommu: smmuv3 with fixed queues or tables, left off");
        return;
    }
    let coherent = idr0 & (1 << 4) != 0 || unit.coherent;
    let oas = (idr5 & 7) as u8;
    let oas_bits = [32u8, 36, 40, 42, 44, 48, 52, 52][oas as usize];
    let ipa_bits = oas_bits.min(48);
    let start_level = if ipa_bits > 39 { 0 } else { 1 };
    let max_vmid = if idr0 & (1 << 18) != 0 { 0xffff } else { 0xff };
    let sidsize = idr1 & 0x3f;
    if (idr1 >> 21 & 0x1f) < CMDQ_LOG2 || (idr1 >> 16 & 0x1f) < EVENTQ_LOG2 {
        crate::console::status_warn("iommu: smmuv3 queues too small, left off");
        return;
    }

    // The stream table: the host's functions and no more (QEMU `virt`
    // maps 65536 requester ids; its buses hold 4096), a power of two.
    let buses = crate::platform::get().pci.map_or(256, |p| u32::from(p.value.bus_end) + 1);
    let sids = (unit.sid_base + unit.rid_count.min(buses << 8)).max(1).next_power_of_two().min(1 << sidsize.min(16));
    let entries = sids;
    let log2 = entries.trailing_zeros();
    // The table's base is aligned to its size (the unit ignores the bits
    // below): a run twice as long holds an aligned one.
    let size = (u64::from(entries) * 64).max(PAGE);
    let frames = (size / PAGE) as usize;
    let Some((run, _)) = mm::alloc_contiguous_pages(if frames > 1 { frames * 2 } else { 1 }, mm::ANYWHERE) else {
        crate::console::status_warn("iommu: smmuv3 stream table: out of memory, left off");
        return;
    };
    let strtab = (run + size - 1) & !(size - 1);
    let Some((cmdq, cmdq_va)) = mm::alloc_contiguous_pages(1, mm::ANYWHERE) else {
        return;
    };
    let Some((eventq, eventq_va)) = mm::alloc_contiguous_pages(1, mm::ANYWHERE) else {
        return;
    };

    let mut st = State {
        regs,
        base: unit.base,
        strtab,
        entries,
        rid_base: unit.rid_base,
        rid_count: unit.rid_count,
        sid_base: unit.sid_base,
        cmdq: cmdq_va as *mut [u32; 4],
        cmdq_prod: 0,
        start_level,
        ipa_bits,
        oas,
        domains: Vec::new(),
        max_vmid,
        coherent,
    };
    for sid in 0..entries {
        let e = ste(&st, sid) as *mut u32;
        let w = ste_bypass();
        for (i, v) in w.iter().enumerate() {
            unsafe { e.add(i).write_volatile(*v) };
        }
    }
    written(coherent, mm::hhdm(strtab) as usize, size as usize);

    // Off while the tables are named, then the command queue, the global
    // invalidation, the event queue and its interrupt, translation.
    write32(regs + CR0, 0);
    if !wait(regs + CR0ACK, CR0_SMMUEN | CR0_CMDQEN | CR0_EVENTQEN, 0) {
        crate::console::status_warn("iommu: smmuv3 does not stop, left off");
        return;
    }
    write32(regs + CR1, CR1_CACHED);
    write32(regs + CR2, CR2_SETUP);
    write64(regs + STRTAB_BASE, strtab | RA);
    write32(regs + STRTAB_BASE_CFG, log2);
    write64(regs + CMDQ_BASE, cmdq | RA | u64::from(CMDQ_LOG2));
    write32(regs + CMDQ_PROD, 0);
    write32(regs + CMDQ_CONS, 0);
    write32(regs + CR0, CR0_CMDQEN);
    if !wait(regs + CR0ACK, CR0_CMDQEN, CR0_CMDQEN) {
        crate::console::status_warn("iommu: smmuv3 command queue stays off, left off");
        return;
    }
    if !command(&mut st, [CMD_CFGI_ALL, 0, 31, 0])
        || !command(&mut st, [CMD_TLBI_NSNH_ALL, 0, 0, 0])
        || !command(&mut st, [CMD_SYNC, 0, 0, 0])
    {
        crate::console::status_warn("iommu: smmuv3 refuses commands, left off");
        return;
    }
    write64(regs + EVENTQ_BASE, eventq | RA | u64::from(EVENTQ_LOG2));
    write32(regs + EVENTQ_PROD, 0);
    write32(regs + EVENTQ_CONS, 0);
    IRQ_REGS.store(regs, Ordering::Relaxed);
    IRQ_QUEUE.store(eventq_va as usize, Ordering::Relaxed);
    write32(regs + CR0, CR0_CMDQEN | CR0_EVENTQEN);
    wait(regs + CR0ACK, CR0_EVENTQEN, CR0_EVENTQEN);
    let mut irqs = 0;
    for (irq, ctx) in [(unit.event_irq, 0usize), (unit.gerror_irq, 1)] {
        if let Some(irq) = irq
            && crate::irq::register(irq, "iommu", interrupt, ctx)
        {
            crate::irqchip::root().unmask_edge(irq);
            irqs |= if ctx == 0 { IRQ_EVENTQ } else { IRQ_GERROR };
        }
    }
    write32(regs + IRQ_CTRL, irqs);
    wait(regs + IRQ_CTRLACK, irqs, irqs);
    write32(regs + CR0, CR0_CMDQEN | CR0_EVENTQEN | CR0_SMMUEN);
    if !wait(regs + CR0ACK, CR0_SMMUEN, CR0_SMMUEN) {
        crate::console::status_warn("iommu: smmuv3 translation stays off");
        return;
    }
    crate::console::status_ok(&alloc::format!(
        "iommu: smmuv3 at 0x{:x}, {} streams, {}-bit stage 2",
        unit.base,
        entries,
        ipa_bits
    ));
    *STATE.lock() = Some(st);
}

pub fn attach(bus: u8, slot: u8, func: u8) -> Option<u32> {
    let mut guard = STATE.lock();
    let st = guard.as_mut()?;
    let dev = (bus, slot, func);
    if let Some(d) = st.domains.iter().find(|d| d.dev == dev) {
        return Some(u32::from(d.vmid));
    }
    let sid = sid_of(st, bus, slot, func)?;
    let vmid = (IDENTITY_VMID + 1..=st.max_vmid).find(|v| !st.domains.iter().any(|d| d.vmid == *v))?;
    let root = mm::alloc_frame();
    written(st.coherent, mm::table(root) as usize, 4096);
    let words = ste_s2(st, root, vmid);
    set_ste(st, sid, words);
    invalidate(st, sid, vmid);
    st.domains.push(Domain { vmid, root, dev, pages: 0 });
    Some(u32::from(vmid))
}

pub fn map(domain: u32, phys: u64, len: usize) -> bool {
    let mut guard = STATE.lock();
    let Some(st) = guard.as_mut() else {
        return false;
    };
    let Some(i) = st.domains.iter().position(|d| u32::from(d.vmid) == domain) else {
        return false;
    };
    let (root, vmid) = (st.domains[i].root, st.domains[i].vmid);
    let (phys, len) = (phys & !0xfff, (len as u64 + (phys & 0xfff) + 0xfff) & !0xfff);
    if len == 0 || phys + len > 1 << st.ipa_bits || !map_pages(st, root, phys, len) {
        return false;
    }
    st.domains[i].pages += (len / PAGE) as usize;
    command(st, [CMD_TLBI_S12_VMALL, u32::from(vmid), 0, 0]);
    command(st, [CMD_SYNC, 0, 0, 0]);
    true
}

/// The domain `bus:slot.func` is confined to ([`attach`]), if any.
pub fn domain_of(bus: u8, slot: u8, func: u8) -> Option<u32> {
    let guard = STATE.lock();
    guard.as_ref()?.domains.iter().find(|d| d.dev == (bus, slot, func)).map(|d| u32::from(d.vmid))
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
    if let Some(sid) = sid_of(st, bus, slot, func) {
        set_ste(st, sid, ste_bypass());
        invalidate(st, sid, d.vmid);
    }
    walk(d.root, st.start_level, true);
}

pub fn text() -> String {
    let mut s = String::new();
    let guard = STATE.lock();
    let Some(st) = guard.as_ref() else {
        s.push_str("none\n");
        return s;
    };
    let _ = writeln!(
        s,
        "smmuv3 0x{:x} streams {} rid 0x{:x}+0x{:x} sid 0x{:x} stage2 {}-bit{}",
        st.base,
        st.entries,
        st.rid_base,
        st.rid_count,
        st.sid_base,
        st.ipa_bits,
        if st.coherent { "" } else { " noncoherent" }
    );
    let _ = writeln!(s, "domain {} bypass", IDENTITY_VMID);
    for d in &st.domains {
        let _ = writeln!(s, "domain {} {:02x}:{:02x}.{} pages {}", d.vmid, d.dev.0, d.dev.1, d.dev.2, d.pages);
    }
    let gerrors = GERRORS.load(Ordering::Relaxed);
    if gerrors != 0 {
        let _ = writeln!(s, "gerrors {gerrors}");
    }
    s
}
