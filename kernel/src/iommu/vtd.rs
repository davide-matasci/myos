//! Intel VT-d, the x86_64 unit: the DMA remapping units of the ACPI DMAR
//! table (Intel Virtualization Technology for Directed I/O, chapters 3, 9
//! and 11). Each unit translates the DMA of the PCI functions in its device
//! scopes, the one flagged `INCLUDE_PCI_ALL` every other function's.
//!
//! A unit walks a root table (one entry per bus) to a context table (one
//! entry per device and function) to a second-level page table, the
//! domain's: the same shape as the CPU's (3 levels for 39-bit addresses, 4
//! for 48), with read and write bits and 2 MiB or 1 GiB superpages where
//! the unit has them. Every function starts in the identity domain
//! ([`IDENTITY_DID`]), whose table maps all of memory one-to-one with
//! superpages, so a kernel driver's device works as before. [`attach`]
//! points a function's context entry at a table of its own; [`map`] puts
//! pages in it, one-to-one too, so a bus address stays a physical one.
//!
//! Invalidation goes through the registers (not the queue), whole: the
//! context cache and the IOTLB after a context entry changes, the IOTLB
//! after a mapping does, the write buffer first where the unit wants it
//! (`CAP.RWBF`). A unit whose page walks do not snoop the caches
//! (`ECAP.C` clear) reads the tables from memory: their lines are flushed
//! after each write. A refused DMA raises the unit's fault event (an MSI
//! to a device vector): the handler reads the fault recording registers
//! and hands each to `iommu::record`.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicUsize, Ordering};

use spin::Mutex;

use super::Fault;
use crate::mm;

// The registers (chapter 11).
const CAP: usize = 0x08;
const ECAP: usize = 0x10;
const GCMD: usize = 0x18;
const GSTS: usize = 0x1C;
const RTADDR: usize = 0x20;
const CCMD: usize = 0x28;
const FSTS: usize = 0x34;
const FECTL: usize = 0x38;
const FEDATA: usize = 0x3C;
const FEADDR: usize = 0x40;
const FEUADDR: usize = 0x44;

const GCMD_TE: u32 = 1 << 31;
const GCMD_SRTP: u32 = 1 << 30;
const GCMD_WBF: u32 = 1 << 27;
const GSTS_TES: u32 = 1 << 31;
const GSTS_RTPS: u32 = 1 << 30;
const GSTS_WBFS: u32 = 1 << 27;
const CCMD_ICC: u64 = 1 << 63;
const CCMD_GLOBAL: u64 = 1 << 61;
const IOTLB_IVT: u64 = 1 << 63;
const IOTLB_GLOBAL: u64 = 1 << 60;
/// Primary pending fault: a fault recording register holds one.
const FSTS_PPF: u32 = 1 << 1;
/// The fault status bits a write of 1 clears (overflow, the queue's errors).
const FSTS_CLEAR: u32 = 0x7D;
/// The fault recording register's `F` bit, in its last dword.
const FRCD_F: u32 = 1 << 31;

const PAGE: u64 = 4096;
/// Second-level page table entries.
const SL_R: u64 = 1;
const SL_W: u64 = 1 << 1;
const SL_PS: u64 = 1 << 7;
const SL_ADDR: u64 = 0x000F_FFFF_FFFF_F000;
const PRESENT: u64 = 1;

/// The identity domain's id (0 is reserved by units in caching mode).
const IDENTITY_DID: u16 = 1;
/// The most units a board has; the fault handler finds its registers here.
const MAX_UNITS: usize = 8;
/// How long a register command may take (a spin count).
const SPINS: u32 = 10_000_000;

/// A device scope of a unit: an endpoint (`kind` 1) or a bridge (2, every
/// function behind it), by its start bus and a path of (device, function).
struct Scope {
    kind: u8,
    start_bus: u8,
    path: Vec<(u8, u8)>,
}

struct Unit {
    base: u64,
    /// The registers, mapped.
    regs: usize,
    cap: u64,
    /// Translates for every function no other unit's scope names.
    include_all: bool,
    scopes: Vec<Scope>,
    /// The root table's physical address.
    root: u64,
    /// The command bits on (what `GCMD` is written with).
    gcmd: u32,
    /// The IOTLB registers' offset (`ECAP.IRO`).
    iro: usize,
}

/// A function's own domain.
struct Domain {
    id: u16,
    /// Its second-level page table's root.
    root: u64,
    dev: (u8, u8, u8),
    pages: usize,
}

struct State {
    units: Vec<Unit>,
    /// Page table levels, 3 or 4.
    levels: u8,
    /// The identity domain's superpage: 30 (1 GiB), 21 (2 MiB) or 12 (none).
    super_shift: u8,
    /// The identity domain's page table root, and the address it maps up to.
    identity: u64,
    top: u64,
    domains: Vec<Domain>,
    max_did: u16,
    /// Every unit snoops the caches on its page walks: no flushing.
    coherent: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Per unit, for the fault handler (no lock there): the registers' address,
/// the first fault recording register's offset and their number.
static IRQ_REGS: [AtomicUsize; MAX_UNITS] = [const { AtomicUsize::new(0) }; MAX_UNITS];
static IRQ_FRO: [AtomicUsize; MAX_UNITS] = [const { AtomicUsize::new(0) }; MAX_UNITS];
static IRQ_NFR: [AtomicUsize; MAX_UNITS] = [const { AtomicUsize::new(0) }; MAX_UNITS];

fn read32(va: usize) -> u32 {
    unsafe { core::ptr::read_volatile(va as *const u32) }
}

fn write32(va: usize, v: u32) {
    unsafe { core::ptr::write_volatile(va as *mut u32, v) }
}

fn read64(va: usize) -> u64 {
    unsafe { core::ptr::read_volatile(va as *const u64) }
}

fn write64(va: usize, v: u64) {
    unsafe { core::ptr::write_volatile(va as *mut u64, v) }
}

/// Write the cache lines of `[va, va + len)` back to memory, for a unit
/// that does not snoop them.
fn flush_lines(va: usize, len: usize) {
    let mut a = va & !63;
    while a < va + len {
        unsafe {
            core::arch::asm!("clflush [{a}]", a = in(reg) a, options(nostack, preserves_flags));
        }
        a += 64;
    }
    unsafe {
        core::arch::asm!("mfence", options(nostack, preserves_flags));
    }
}

/// The table writes done so far reach the unit before what follows.
fn tables_written(coherent: bool, va: usize, len: usize) {
    if !coherent {
        flush_lines(va, len);
    }
    core::sync::atomic::fence(Ordering::SeqCst);
}

/// `GCMD` with `bit` set, waited for in `GSTS`: false when the unit did not
/// take it.
fn command(u: &Unit, bit: u32, done: u32, on: bool) -> bool {
    write32(u.regs + GCMD, u.gcmd | bit);
    for _ in 0..SPINS {
        if (read32(u.regs + GSTS) & done != 0) == on {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// Drop everything the unit cached: the write buffer where it has one,
/// the context entries, the translations.
fn invalidate(u: &Unit) {
    if u.cap & (1 << 4) != 0 {
        command(u, GCMD_WBF, GSTS_WBFS, false);
    }
    write64(u.regs + CCMD, CCMD_ICC | CCMD_GLOBAL);
    for _ in 0..SPINS {
        if read64(u.regs + CCMD) & CCMD_ICC == 0 {
            break;
        }
        core::hint::spin_loop();
    }
    invalidate_iotlb(u);
}

/// Drop every translation the unit cached.
fn invalidate_iotlb(u: &Unit) {
    if u.cap & (1 << 4) != 0 {
        command(u, GCMD_WBF, GSTS_WBFS, false);
    }
    write64(u.regs + u.iro + 8, IOTLB_IVT | IOTLB_GLOBAL);
    for _ in 0..SPINS {
        if read64(u.regs + u.iro + 8) & IOTLB_IVT == 0 {
            break;
        }
        core::hint::spin_loop();
    }
}

/// The unit's fault event: each refused DMA in a fault recording register,
/// read, handed over and cleared.
unsafe extern "C" fn fault_handler(ctx: *mut core::ffi::c_void) {
    let i = ctx as usize;
    if i >= MAX_UNITS {
        return;
    }
    let regs = IRQ_REGS[i].load(Ordering::Relaxed);
    if regs == 0 {
        return;
    }
    let (fro, nfr) = (IRQ_FRO[i].load(Ordering::Relaxed), IRQ_NFR[i].load(Ordering::Relaxed));
    let fsts = read32(regs + FSTS);
    if fsts & FSTS_PPF != 0 {
        for r in 0..nfr {
            let rec = regs + fro + r * 16;
            let hi = read64(rec + 8);
            if hi >> 63 == 0 {
                continue;
            }
            let lo = read64(rec);
            super::record(Fault {
                sid: hi as u16,
                addr: lo & !0xfff,
                reason: (hi >> 32) as u8,
                read: (hi >> 62) & 1 != 0,
            });
            write32(rec + 12, FRCD_F);
        }
    }
    write32(regs + FSTS, fsts & FSTS_CLEAR);
}

/// The index into a table at `level` (1 the 4 KiB leaves, 2 the 2 MiB, 3
/// the 1 GiB, 4 the 512 GiB).
fn index(iova: u64, level: u8) -> usize {
    ((iova >> (12 + 9 * (level as u64 - 1))) & 0x1ff) as usize
}

/// The entry for `iova` at `leaf` in the table rooted at `root` (`levels`
/// deep), the tables above it made when `create`; `None` when there is
/// none to make, or a superpage sits above the level asked for.
fn entry(root: u64, levels: u8, iova: u64, leaf: u8, create: bool) -> Option<*mut u64> {
    let mut table = root;
    let mut level = levels;
    loop {
        let e = unsafe { (mm::table(table) as *mut u64).add(index(iova, level)) };
        if level == leaf {
            return Some(e);
        }
        let v = unsafe { e.read_volatile() };
        if v & PRESENT != 0 {
            if v & SL_PS != 0 {
                return None;
            }
            table = v & SL_ADDR;
        } else {
            if !create {
                return None;
            }
            table = mm::alloc_frame();
            unsafe { e.write_volatile(table | SL_R | SL_W) };
        }
        level -= 1;
    }
}

/// Map `[phys, phys + len)` one-to-one into the table at `root`, in pages
/// of `1 << shift` (12, 21 or 30; `phys` and `len` multiples of it).
fn map_identity(root: u64, levels: u8, coherent: bool, phys: u64, len: u64, shift: u8) -> bool {
    let leaf = (shift - 12) / 9 + 1;
    let ps = if leaf > 1 { SL_PS } else { 0 };
    let mut a = phys;
    while a < phys + len {
        let Some(e) = entry(root, levels, a, leaf, true) else {
            return false;
        };
        unsafe { e.write_volatile(a | SL_R | SL_W | ps) };
        tables_written(coherent, e as usize, 8);
        a += 1 << shift;
    }
    // The tables made on the way, too.
    if !coherent {
        free_or_flush(root, levels, levels, false);
    }
    true
}

/// Walk the table at `root`: flush every table's lines (`free` false), or
/// give every table's frame back, the root's included (`free` true).
fn free_or_flush(table: u64, level: u8, levels: u8, free: bool) {
    let t = mm::table(table);
    if level > 1 {
        for i in 0..512 {
            let v = unsafe { (*t)[i] };
            if v & PRESENT != 0 && v & SL_PS == 0 {
                free_or_flush(v & SL_ADDR, level - 1, levels, free);
            }
        }
    }
    if free {
        mm::free_frame(table);
    } else {
        flush_lines(t as usize, 4096);
    }
}

/// The context entry of `bus:slot.func` in `u`'s tables.
fn context_entry(u: &Unit, bus: u8, slot: u8, func: u8) -> *mut u64 {
    let root = mm::table(u.root) as *mut u64;
    let ctx = unsafe { root.add(bus as usize * 2).read_volatile() } & SL_ADDR;
    unsafe { (mm::table(ctx) as *mut u64).add((slot as usize * 8 + func as usize) * 2) }
}

/// Point the context entry at domain `did`'s table at `root`.
fn set_context(st: &State, u: &Unit, dev: (u8, u8, u8), root: u64, did: u16) {
    let e = context_entry(u, dev.0, dev.1, dev.2);
    let aw = if st.levels == 4 { 2 } else { 1 };
    unsafe {
        // Not present while the pointer changes, then whole.
        e.write_volatile(0);
        e.add(1).write_volatile(aw | (u64::from(did) << 8));
        e.write_volatile(root | PRESENT);
    }
    tables_written(st.coherent, e as usize, 16);
    invalidate(u);
}

/// Whether `scope` names `bus:slot.func`: an endpoint by its path, a
/// bridge by the buses behind it (its secondary and subordinate numbers).
fn in_scope(scope: &Scope, bus: u8, slot: u8, func: u8) -> bool {
    let mut b = scope.start_bus;
    let n = scope.path.len();
    for (i, &(d, f)) in scope.path.iter().enumerate() {
        if i + 1 == n {
            return match scope.kind {
                1 => bus == b && slot == d && func == f,
                2 => {
                    let buses = crate::pci::cfg_read32(b, d, f, 0x18);
                    let (secondary, subordinate) = ((buses >> 8) as u8, (buses >> 16) as u8);
                    bus >= secondary && bus <= subordinate
                }
                _ => false,
            };
        }
        b = (crate::pci::cfg_read32(b, d, f, 0x18) >> 8) as u8;
    }
    false
}

/// The unit that translates for `bus:slot.func`: the one whose scope names
/// it, else the one for every other function.
fn unit_for(st: &State, bus: u8, slot: u8, func: u8) -> Option<usize> {
    st.units
        .iter()
        .position(|u| u.scopes.iter().any(|s| in_scope(s, bus, slot, func)))
        .or_else(|| st.units.iter().position(|u| u.include_all))
}

/// The address every unit must reach for the identity domain: the end of
/// the memory map (every entry: firmware's reserved ranges take DMA too),
/// at least the first 4 GiB (devices below it), rounded up to a superpage.
fn memory_top(super_shift: u8) -> u64 {
    let end = crate::limine_boot::MEMMAP
        .response()
        .map(|r| r.entries().iter().map(|e| e.base + e.length).max().unwrap_or(0))
        .unwrap_or(0)
        .max(1 << 32);
    let align = 1u64 << super_shift;
    (end + align - 1) & !(align - 1)
}

/// Find the units, put every function in the identity domain, turn
/// translation on.
pub fn init() {
    let Some(dmar) = crate::acpi::dmar() else {
        return;
    };
    let mut units = Vec::new();
    for d in dmar.units() {
        if units.len() == MAX_UNITS {
            break;
        }
        let Some(regs) = crate::arch::pci::map_mmio(d.base, PAGE) else {
            crate::console::status_warn(&alloc::format!("iommu: vt-d unit at 0x{:x}: cannot map", d.base));
            continue;
        };
        let cap = read64(regs + CAP);
        let ecap = read64(regs + ECAP);
        let scopes = d
            .scopes()
            .map(|s| Scope { kind: s.kind, start_bus: s.start_bus, path: s.path().collect() })
            .collect();
        units.push((
            Unit {
                base: d.base,
                regs,
                cap,
                include_all: d.include_all,
                scopes,
                root: 0,
                gcmd: 0,
                iro: ((ecap >> 8) & 0x3ff) as usize * 16,
            },
            ecap,
        ));
    }
    if units.is_empty() {
        return;
    }
    // What every unit supports: the deepest common page walk, the largest
    // common superpage, whether all snoop the tables.
    let sagaw = units.iter().fold(0x1f, |a, (u, _)| a & ((u.cap >> 8) & 0x1f));
    let sllps = units.iter().fold(0xf, |a, (u, _)| a & ((u.cap >> 34) & 0xf));
    let coherent = units.iter().all(|(_, ecap)| ecap & 1 != 0);
    let levels = if sagaw & 0b100 != 0 {
        4
    } else if sagaw & 0b010 != 0 {
        3
    } else {
        crate::console::status_warn("iommu: vt-d units share no page table depth, left off");
        return;
    };
    let super_shift = if sllps & 0b10 != 0 { 30 } else if sllps & 0b01 != 0 { 21 } else { 12 };
    let max_did = (1u32 << (4 + 2 * (units[0].0.cap & 7))).saturating_sub(1).min(0xffff) as u16;
    let top = memory_top(super_shift).min(1 << (39 + 9 * (levels as u64 - 3)));

    let identity = mm::alloc_frame();
    if !map_identity(identity, levels, coherent, 0, top, super_shift) {
        crate::console::status_warn("iommu: vt-d identity domain: out of memory, left off");
        return;
    }
    let mut st = State { units: Vec::new(), levels, super_shift, identity, top, domains: Vec::new(), max_did, coherent };
    let aw = if levels == 4 { 2u64 } else { 1 };
    for (i, (mut u, _)) in units.into_iter().enumerate() {
        // The root table: every bus a context table, every function in it
        // the identity domain.
        u.root = mm::alloc_frame();
        let root = mm::table(u.root) as *mut u64;
        for bus in 0..256usize {
            let ctx = mm::alloc_frame();
            let t = mm::table(ctx) as *mut u64;
            for devfn in 0..256usize {
                unsafe {
                    t.add(devfn * 2 + 1).write_volatile(aw | (u64::from(IDENTITY_DID) << 8));
                    t.add(devfn * 2).write_volatile(identity | PRESENT);
                }
            }
            tables_written(coherent, t as usize, 4096);
            unsafe { root.add(bus * 2).write_volatile(ctx | PRESENT) };
        }
        tables_written(coherent, root as usize, 4096);

        write64(u.regs + RTADDR, u.root);
        if !command(&u, GCMD_SRTP, GSTS_RTPS, true) {
            crate::console::status_warn(&alloc::format!("iommu: vt-d unit at 0x{:x}: no root table", u.base));
            continue;
        }
        invalidate(&u);
        // The fault event: an MSI to a device vector on the BSP.
        if let Some((vector, address)) = crate::arch::msi_vector() {
            IRQ_REGS[i].store(u.regs, Ordering::Relaxed);
            IRQ_FRO[i].store(((u.cap >> 24) & 0x3ff) as usize * 16, Ordering::Relaxed);
            IRQ_NFR[i].store(((u.cap >> 40) & 0xff) as usize + 1, Ordering::Relaxed);
            if crate::irq::register(vector, "iommu", fault_handler, i) {
                write32(u.regs + FEDATA, vector);
                write32(u.regs + FEADDR, address);
                write32(u.regs + FEUADDR, 0);
                write32(u.regs + FECTL, 0);
            }
        }
        if !command(&u, GCMD_TE, GSTS_TES, true) {
            crate::console::status_warn(&alloc::format!("iommu: vt-d unit at 0x{:x}: translation stays off", u.base));
            continue;
        }
        u.gcmd |= GCMD_TE;
        st.units.push(u);
    }
    if st.units.is_empty() {
        return;
    }
    crate::console::status_ok(&alloc::format!(
        "iommu: vt-d, {} unit(s), {}-level tables, {} superpages, memory to 0x{:x}",
        st.units.len(),
        levels,
        match super_shift {
            30 => "1 GiB",
            21 => "2 MiB",
            _ => "no",
        },
        top
    ));
    *STATE.lock() = Some(st);
}

pub fn attach(bus: u8, slot: u8, func: u8) -> Option<u32> {
    let mut guard = STATE.lock();
    let st = guard.as_mut()?;
    let dev = (bus, slot, func);
    if let Some(d) = st.domains.iter().find(|d| d.dev == dev) {
        return Some(u32::from(d.id));
    }
    let unit = unit_for(st, bus, slot, func)?;
    let id = (IDENTITY_DID + 1..=st.max_did).find(|id| !st.domains.iter().any(|d| d.id == *id))?;
    let root = mm::alloc_frame();
    tables_written(st.coherent, mm::table(root) as usize, 4096);
    set_context(st, &st.units[unit], dev, root, id);
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
    let (root, dev) = (st.domains[i].root, st.domains[i].dev);
    let (phys, len) = (phys & !0xfff, (len as u64 + (phys & 0xfff) + 0xfff) & !0xfff);
    if len == 0 || !map_identity(root, st.levels, st.coherent, phys, len, 12) {
        return false;
    }
    st.domains[i].pages += (len / PAGE) as usize;
    if let Some(u) = unit_for(st, dev.0, dev.1, dev.2) {
        invalidate_iotlb(&st.units[u]);
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
    if let Some(u) = unit_for(st, bus, slot, func) {
        set_context(st, &st.units[u], dev, st.identity, IDENTITY_DID);
    }
    free_or_flush(d.root, st.levels, st.levels, true);
}

pub fn text() -> String {
    let mut s = String::new();
    let guard = STATE.lock();
    let Some(st) = guard.as_ref() else {
        s.push_str("none\n");
        return s;
    };
    for u in &st.units {
        let _ = write!(s, "vtd 0x{:x} levels {} superpage ", u.base, st.levels);
        let _ = match st.super_shift {
            30 => write!(s, "1G"),
            21 => write!(s, "2M"),
            _ => write!(s, "none"),
        };
        let _ = writeln!(s, " scopes {}{}", u.scopes.len(), if u.include_all { " all" } else { "" });
    }
    let _ = writeln!(s, "domain {} identity 0x0-0x{:x}", IDENTITY_DID, st.top);
    for d in &st.domains {
        let _ = writeln!(s, "domain {} {:02x}:{:02x}.{} pages {}", d.id, d.dev.0, d.dev.1, d.dev.2, d.pages);
    }
    s
}
