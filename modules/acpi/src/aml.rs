//! The firmware's AML, run by the `acpi` crate's interpreter (x86_64):
//! every PCI bridge's `_PRT` (the routes of its devices' INTx pins, through
//! link devices where the firmware has them) handed to the kernel
//! (`pci_intx_route`), `_S5` (the power-off sleep types), and the
//! batteries and AC adapters (`battery`: `/proc/acpi/battery`, the thread
//! answering its reads after the boot's work).
//!
//! The interpreter is someone else's code over the firmware's, and stops
//! with a panic at what it does not implement yet (`todo!`). It runs in a
//! thread of its own ([`start`]): a panic parks that thread
//! ([`panicked`]), and the module's init waits for it only until a
//! deadline. Either way the boot goes on as without it: no routes (the
//! drivers poll), `_S5` from the byte scan.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write;
use core::ptr::NonNull;
use core::str::FromStr;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use acpi::aml::namespace::{AmlName, NameSeg, NamespaceLevelKind};
use acpi::aml::object::Object;
use acpi::aml::pci_routing::{Pin, PciRoutingTable};
use acpi::aml::resource::{InterruptPolarity, InterruptTrigger};
use acpi::aml::{AmlError, Interpreter};
use acpi::platform::AcpiPlatform;
use acpi::{AcpiTables, Handle, Handler, PciAddress, PhysicalMapping};
use myos_abi::{KernelApi, Lock, PCI_INTX_ACTIVE_LOW, PCI_INTX_LEVEL};

use crate::{api, battery};

/// The thread's state: running, done (its results below), or parked by a
/// panic (its message in [`REPORT`]).
const RUNNING: u32 = 0;
const DONE: u32 = 1;
const PANICKED: u32 = 2;
static STATE: AtomicU32 = AtomicU32::new(RUNNING);
/// The thread is the one running AML: a panic parks it.
static IN_THREAD: AtomicBool = AtomicBool::new(false);
/// `_S5`'s sleep types, when the interpreter had them (bit 16 set).
static S5: AtomicU32 = AtomicU32::new(0);
/// What happened, for `/proc/acpi/aml`.
static REPORT: Lock<String> = Lock::new(String::new());
/// What the thread is doing, for the report of a panic.
static STEP: Lock<String> = Lock::new(String::new());

/// The thread is now doing `what` (a panic's report names it).
pub(crate) fn step(what: impl Into<String>) {
    *STEP.lock() = what.into();
}

/// How long the module's init waits for the interpreter.
pub const DEADLINE_NS: u64 = 5_000_000_000;

/// The ACPI root bridges' ids (`_HID`, `_CID`): PCI and PCI Express, as
/// EISA ids (`PNP0A03`, `PNP0A08`) or strings.
const PNP0A03: u64 = 0x030a_d041;
const PNP0A08: u64 = 0x080a_d041;

/// The crate's way into the machine, through the module ABI.
#[derive(Clone, Copy)]
pub(crate) struct Host;

/// A port of the machine's I/O space.
pub(crate) fn inb(port: u16) -> u8 {
    let v: u8;
    unsafe { core::arch::asm!("in al, dx", out("al") v, in("dx") port, options(nomem, nostack, preserves_flags)) };
    v
}
fn inw(port: u16) -> u16 {
    let v: u16;
    unsafe { core::arch::asm!("in ax, dx", out("ax") v, in("dx") port, options(nomem, nostack, preserves_flags)) };
    v
}
fn inl(port: u16) -> u32 {
    let v: u32;
    unsafe { core::arch::asm!("in eax, dx", out("eax") v, in("dx") port, options(nomem, nostack, preserves_flags)) };
    v
}
pub(crate) fn outb(port: u16, v: u8) {
    unsafe { core::arch::asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack, preserves_flags)) };
}
fn outw(port: u16, v: u16) {
    unsafe { core::arch::asm!("out dx, ax", in("dx") port, in("ax") v, options(nomem, nostack, preserves_flags)) };
}
fn outl(port: u16, v: u32) {
    unsafe { core::arch::asm!("out dx, eax", in("dx") port, in("eax") v, options(nomem, nostack, preserves_flags)) };
}

/// A configuration register of the function `address` (segment 0 only, the
/// first 256 bytes: what the module ABI reaches): the 32-bit word holding
/// `offset`, `None` past them.
fn cfg_word(address: PciAddress, offset: u16) -> Option<(u8, u8, u8, u8)> {
    if address.segment() != 0 || offset > 0xff {
        return None;
    }
    Some((address.bus(), address.device(), address.function(), (offset & !3) as u8))
}

fn cfg_read(address: PciAddress, offset: u16, bytes: u32) -> u32 {
    let Some((bus, dev, func, off)) = cfg_word(address, offset) else {
        return u32::MAX;
    };
    let shift = u32::from(offset & 3) * 8;
    let mask = if bytes == 4 { u32::MAX } else { (1 << (bytes * 8)) - 1 };
    (api().pci_cfg_read32(bus, dev, func, off) >> shift) & mask
}

fn cfg_write(address: PciAddress, offset: u16, bytes: u32, value: u32) {
    let Some((bus, dev, func, off)) = cfg_word(address, offset) else {
        return;
    };
    let shift = u32::from(offset & 3) * 8;
    let mask = if bytes == 4 { u32::MAX } else { ((1 << (bytes * 8)) - 1) << shift };
    let old = api().pci_cfg_read32(bus, dev, func, off);
    api().pci_cfg_write32(bus, dev, func, off, (old & !mask) | ((value << shift) & mask));
}

/// Mutexes for the AML: one task runs it, so each is just a number.
static NEXT_MUTEX: AtomicU32 = AtomicU32::new(1);

impl Handler for Host {
    unsafe fn map_physical_region<T>(&self, physical_address: usize, size: usize) -> PhysicalMapping<Self, T> {
        let va = api().phys_map(physical_address as u64, size).unwrap_or(0);
        // A region the kernel cannot map ends the interpreter's work here
        // (its own panic, in its thread).
        let virtual_start = NonNull::new(va as *mut T).expect("acpi: physical region not mapped");
        PhysicalMapping { physical_start: physical_address, virtual_start, region_length: size, mapped_length: size, handler: *self }
    }

    // The kernel keeps its mappings.
    fn unmap_physical_region<T>(_region: &PhysicalMapping<Self, T>) {}

    fn read_u8(&self, address: usize) -> u8 {
        let va = api().phys_map(address as u64, 1).expect("acpi: memory not mapped");
        unsafe { core::ptr::read_volatile(va as *const u8) }
    }
    fn read_u16(&self, address: usize) -> u16 {
        let va = api().phys_map(address as u64, 2).expect("acpi: memory not mapped");
        unsafe { core::ptr::read_volatile(va as *const u16) }
    }
    fn read_u32(&self, address: usize) -> u32 {
        let va = api().phys_map(address as u64, 4).expect("acpi: memory not mapped");
        unsafe { core::ptr::read_volatile(va as *const u32) }
    }
    fn read_u64(&self, address: usize) -> u64 {
        let va = api().phys_map(address as u64, 8).expect("acpi: memory not mapped");
        unsafe { core::ptr::read_volatile(va as *const u64) }
    }
    fn write_u8(&self, address: usize, value: u8) {
        let va = api().phys_map(address as u64, 1).expect("acpi: memory not mapped");
        unsafe { core::ptr::write_volatile(va as *mut u8, value) }
    }
    fn write_u16(&self, address: usize, value: u16) {
        let va = api().phys_map(address as u64, 2).expect("acpi: memory not mapped");
        unsafe { core::ptr::write_volatile(va as *mut u16, value) }
    }
    fn write_u32(&self, address: usize, value: u32) {
        let va = api().phys_map(address as u64, 4).expect("acpi: memory not mapped");
        unsafe { core::ptr::write_volatile(va as *mut u32, value) }
    }
    fn write_u64(&self, address: usize, value: u64) {
        let va = api().phys_map(address as u64, 8).expect("acpi: memory not mapped");
        unsafe { core::ptr::write_volatile(va as *mut u64, value) }
    }

    fn read_io_u8(&self, port: u16) -> u8 {
        inb(port)
    }
    fn read_io_u16(&self, port: u16) -> u16 {
        inw(port)
    }
    fn read_io_u32(&self, port: u16) -> u32 {
        inl(port)
    }
    fn write_io_u8(&self, port: u16, value: u8) {
        outb(port, value)
    }
    fn write_io_u16(&self, port: u16, value: u16) {
        outw(port, value)
    }
    fn write_io_u32(&self, port: u16, value: u32) {
        outl(port, value)
    }

    fn read_pci_u8(&self, address: PciAddress, offset: u16) -> u8 {
        cfg_read(address, offset, 1) as u8
    }
    fn read_pci_u16(&self, address: PciAddress, offset: u16) -> u16 {
        cfg_read(address, offset, 2) as u16
    }
    fn read_pci_u32(&self, address: PciAddress, offset: u16) -> u32 {
        cfg_read(address, offset, 4)
    }
    fn write_pci_u8(&self, address: PciAddress, offset: u16, value: u8) {
        cfg_write(address, offset, 1, u32::from(value))
    }
    fn write_pci_u16(&self, address: PciAddress, offset: u16, value: u16) {
        cfg_write(address, offset, 2, u32::from(value))
    }
    fn write_pci_u32(&self, address: PciAddress, offset: u16, value: u32) {
        cfg_write(address, offset, 4, value)
    }

    fn nanos_since_boot(&self) -> u64 {
        api().monotonic_ns()
    }

    fn stall(&self, microseconds: u64) {
        let end = api().monotonic_ns() + microseconds * 1000;
        while api().monotonic_ns() < end {
            core::hint::spin_loop();
        }
    }

    fn sleep(&self, milliseconds: u64) {
        api().task_sleep_until(api().monotonic_ns() + milliseconds * 1_000_000);
    }

    fn create_mutex(&self) -> Handle {
        Handle(NEXT_MUTEX.fetch_add(1, Ordering::Relaxed))
    }

    fn acquire(&self, _mutex: Handle, _timeout: u16) -> Result<(), AmlError> {
        Ok(())
    }

    fn release(&self, _mutex: Handle) {}
}

/// The interpreter's thread is the one panicking: park it, the report
/// saying so. `false` for a panic anywhere else.
pub fn panicked(info: &core::panic::PanicInfo) -> bool {
    if !IN_THREAD.load(Ordering::Acquire) {
        return false;
    }
    // The crate's panics are mostly a bare `todo!()`: its place in the
    // crate's source says which, the step what the AML was doing.
    let step = STEP.lock().clone();
    let mut r = REPORT.lock();
    let _ = write!(r, "interpreter panicked: {}", info.message());
    if let Some(l) = info.location() {
        // The crate's path from its root (`src/aml/mod.rs`).
        let file = l.file().rsplit_once("acpi-myos/").map_or(l.file(), |(_, f)| f);
        let _ = write!(r, " at {file}:{}", l.line());
    }
    let _ = write!(r, " ({step})\n");
    drop(r);
    STATE.store(PANICKED, Ordering::Release);
    true
}

/// The interpreter's thread stopped on a panic.
pub fn stopped() -> bool {
    STATE.load(Ordering::Acquire) == PANICKED
}

/// Run the interpreter on the tables of RSDP `rsdp_phys` in a thread:
/// whether it started.
pub fn start(api: &KernelApi, rsdp_phys: u64) -> bool {
    RSDP.store(rsdp_phys as usize, Ordering::Relaxed);
    api.thread_spawn("acpi aml", thread, core::ptr::null_mut()) == 0
}

static RSDP: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

unsafe extern "C" fn thread(_ctx: *mut core::ffi::c_void) {
    IN_THREAD.store(true, Ordering::Release);
    let (report, serve) = match run(RSDP.load(Ordering::Relaxed)) {
        Ok((r, interp, power)) => (r, power.map(|p| (interp, p))),
        Err(e) => (alloc::format!("interpreter: {e}\n"), None),
    };
    REPORT.lock().push_str(&report);
    STATE.store(DONE, Ordering::Release);
    // With a battery or an adapter the thread stays, the one running AML
    // (a panic still parks it).
    match serve {
        Some((interp, power)) => battery::answer(&interp, &power),
        None => IN_THREAD.store(false, Ordering::Release),
    }
}

/// Wait for the thread until `deadline` (monotonic ns): `_S5`'s sleep
/// types when it found them, and the report.
pub fn wait(api: &KernelApi, deadline: u64) -> (Option<(u8, u8)>, String) {
    while STATE.load(Ordering::Acquire) == RUNNING && api.monotonic_ns() < deadline {
        api.task_sleep_until(api.monotonic_ns() + 10_000_000);
    }
    let mut report = REPORT.lock().clone();
    let s5 = match STATE.load(Ordering::Acquire) {
        DONE => {
            let v = S5.load(Ordering::Acquire);
            (v & 1 << 16 != 0).then_some((v as u8, (v >> 8) as u8))
        }
        PANICKED => None,
        _ => {
            report.push_str("interpreter: still running at the deadline\n");
            None
        }
    };
    (s5, report)
}

/// Load the tables, set the namespace up, then read every bridge's `_PRT`
/// and `_S5` and find the batteries: what was found, for the report, the
/// interpreter, and the batteries when there are any.
fn run(rsdp: usize) -> Result<(String, Interpreter<Host>, Option<battery::Power>), String> {
    step("loading the tables");
    let tables = unsafe { AcpiTables::from_rsdp(Host, rsdp) }.map_err(|e| alloc::format!("tables: {e:?}"))?;
    let platform = AcpiPlatform::new(tables, Host).map_err(|e| alloc::format!("platform: {e:?}"))?;
    let interp = Interpreter::new_from_platform(&platform).map_err(|e| alloc::format!("loading the AML: {e:?}"))?;
    // Laptops' `_INI`s and battery methods read the embedded controller.
    // In the report at once: a panic below keeps it.
    step("finding the embedded controller");
    let ec = battery::install_ec(&interp);
    REPORT.lock().push_str(&ec);
    step("the devices' _STA and _INI");
    interp.initialize_namespace();
    step("the embedded controller's _REG");
    battery::reg_ec(&interp);
    step("\\_PIC");
    // The OS uses the APIC: the firmware's `_PRT` then gives GSIs, not
    // the 8259's IRQs.
    let pic = name("\\_PIC");
    if let Err(e) = interp.evaluate_if_present(pic, vec![Object::Integer(1).wrap()]) {
        return Err(alloc::format!("\\_PIC: {e:?}"));
    }
    let mut out = String::new();
    let mut routes = 0;
    step("finding the PCI bridges");
    for (path, root) in bridges(&interp) {
        step(alloc::format!("{}._PRT", path.as_string()));
        let Some(bus) = bus_of(&interp, &path, root, 0) else {
            let _ = writeln!(out, "{}: no bus", path.as_string());
            continue;
        };
        let prt = name("_PRT").resolve(&path).map_err(|e| alloc::format!("{e:?}"))?;
        let table = match PciRoutingTable::from_prt_path(prt, &interp) {
            Ok(t) => t,
            Err(e) => {
                let _ = writeln!(out, "{} bus {bus}: _PRT {e:?}", path.as_string());
                continue;
            }
        };
        let mut n = 0;
        for device in 0..32u16 {
            for (i, pin) in [Pin::IntA, Pin::IntB, Pin::IntC, Pin::IntD].into_iter().enumerate() {
                let Ok(irq) = table.route(device, 0, pin, &interp) else {
                    continue;
                };
                let mut flags = 0;
                if irq.trigger == InterruptTrigger::Level {
                    flags |= PCI_INTX_LEVEL;
                }
                if irq.polarity == InterruptPolarity::ActiveLow {
                    flags |= PCI_INTX_ACTIVE_LOW;
                }
                if api().pci_intx_route(bus, device as u8, i as u8, irq.irq, flags) {
                    n += 1;
                }
            }
        }
        routes += n;
        let _ = writeln!(out, "prt {} bus {bus} routes {n}", path.as_string());
    }
    let _ = writeln!(out, "routes {routes}");
    step("\\_S5");
    match s5(&interp) {
        Some((a, b)) => {
            S5.store(1 << 16 | u32::from(b) << 8 | u32::from(a), Ordering::Release);
            let _ = writeln!(out, "_S5 {a} {b}");
        }
        None => out.push_str("_S5 none\n"),
    }
    step("finding the batteries");
    let power = battery::find(&interp, &mut out);
    Ok((out, interp, power))
}

pub(crate) fn name(s: &str) -> AmlName {
    AmlName::from_str(s).unwrap_or_else(|_| AmlName::root())
}

/// The devices with a `_PRT`: their path, and whether each is a root
/// bridge (its `_HID` or `_CID` a PCI host bridge's).
fn bridges(interp: &Interpreter<Host>) -> Vec<(AmlName, bool)> {
    let prt = NameSeg::from_str("_PRT").ok();
    let mut found = Vec::new();
    let _ = interp.namespace.lock().traverse(|path, level| {
        if level.kind == NamespaceLevelKind::Device && prt.is_some_and(|p| level.values.contains_key(&p)) {
            found.push(path.clone());
        }
        Ok(true)
    });
    found.into_iter().map(|p| {
        let root = is_root(interp, &p);
        (p, root)
    }).collect()
}

/// Whether the device at `path` is a PCI host bridge (`_HID` or `_CID`
/// `PNP0A03` or `PNP0A08`).
fn is_root(interp: &Interpreter<Host>, path: &AmlName) -> bool {
    let host = |o: &Object| match o {
        Object::Integer(v) => *v == PNP0A03 || *v == PNP0A08,
        Object::String(s) => s == "PNP0A03" || s == "PNP0A08",
        _ => false,
    };
    ["_HID", "_CID"].iter().any(|n| {
        let Ok(p) = name(n).resolve(path) else {
            return false;
        };
        match interp.evaluate_if_present(p, vec![]) {
            Ok(Some(v)) => match &*v {
                Object::Package(ids) => ids.iter().any(|id| host(id)),
                o => host(o),
            },
            _ => false,
        }
    })
}

/// An integer the device at `path` has (`_BBN`, `_ADR`), evaluated.
fn integer(interp: &Interpreter<Host>, path: &AmlName, n: &str) -> Option<u64> {
    let p = name(n).resolve(path).ok()?;
    interp.evaluate_if_present(p, vec![]).ok()??.as_integer().ok()
}

/// The PCI bus of the bridge at `path`: a root bridge's `_BBN` (0 when it
/// has none); a PCI-to-PCI bridge's secondary bus, read from its
/// configuration at its `_ADR` on its parent's bus.
fn bus_of(interp: &Interpreter<Host>, path: &AmlName, root: bool, depth: u32) -> Option<u8> {
    if root {
        return Some(integer(interp, path, "_BBN").unwrap_or(0) as u8);
    }
    if depth > 8 {
        return None;
    }
    let parent = path.parent().ok()?;
    let parent_bus = bus_of(interp, &parent, is_root(interp, &parent), depth + 1)?;
    let adr = integer(interp, path, "_ADR")?;
    let (dev, func) = ((adr >> 16) as u8, adr as u8);
    let secondary = (api().pci_cfg_read32(parent_bus, dev, func, 0x18) >> 8) as u8;
    (secondary != 0 && secondary != 0xff).then_some(secondary)
}

/// `\_S5`'s first two integers, evaluated (a method or a package).
fn s5(interp: &Interpreter<Host>) -> Option<(u8, u8)> {
    let v = interp.evaluate_if_present(name("\\_S5"), vec![]).ok()??;
    let Object::Package(items) = &*v else {
        return None;
    };
    let a = items.first()?.as_integer().ok()?;
    let b = items.get(1).and_then(|o| o.as_integer().ok()).unwrap_or(a);
    Some((a as u8, b as u8))
}
