//! Batteries and AC adapters through the firmware's AML (x86_64): the
//! embedded controller's operation regions, which their methods read on a
//! laptop ([`install_ec`]), and `/proc/acpi/battery`, made at each read
//! ([`serve`]).
//!
//! The embedded controller (`PNP0C09`) is the ACPI one: its data and
//! command/status ports are the first two I/O ports of its `_CRS`, a byte
//! read is command 0x80 then the address, a write 0x81 then the address and
//! the value, each step waiting for the controller's input buffer to empty
//! (and its output buffer to fill before the data is read).
//!
//! A battery (`PNP0C0A`) has `_STA` (bit 4: one is in the slot), `_BIX` or
//! `_BIF` (its capacities and their unit) and `_BST` (state, rate,
//! remaining capacity, voltage); an AC adapter (`ACPI0003`) has `_PSR`
//! (1: on line power). The interpreter can still panic on AML it does not
//! implement, so it runs only in its own thread: a read of the file asks
//! that thread ([`read`]) and waits for its answer a bounded time.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicU32, Ordering};

use acpi::aml::namespace::{AmlName, NamespaceLevelKind};
use acpi::aml::object::Object;
use acpi::aml::op_region::{OpRegion, RegionHandler, RegionSpace};
use acpi::aml::resource::{resource_descriptor_list, Resource};
use acpi::aml::{AmlError, Interpreter};
use myos_abi::{Lock, SleepLock};

use crate::aml::{inb, name, outb, Host};
use crate::{api, API};

/// The ids (`_HID`) as EISA ids: an embedded controller, a battery.
const PNP0C09: u64 = 0x090c_d041;
const PNP0C0A: u64 = 0x0a0c_d041;
/// An AC adapter's `_HID`, a string.
const ACPI0003: &str = "ACPI0003";

/// The status port's bits: output buffer full, input buffer full.
const EC_OBF: u8 = 1;
const EC_IBF: u8 = 2;
/// The read and write commands.
const EC_READ: u8 = 0x80;
const EC_WRITE: u8 = 0x81;
/// How long a step may take before the access fails (the specification's
/// is far shorter; a controller busy with its own work can take this).
const EC_WAIT_NS: u64 = 100_000_000;

/// The embedded controller: its data and command/status ports.
struct Ec {
    data: u16,
    command: u16,
}

/// The controller's steps that ran out of time, and its status when the
/// last one did: `/proc/acpi/aml` and a failed read of the battery say so.
static EC_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static EC_LAST_STATUS: AtomicU32 = AtomicU32::new(0);

/// A line on the controller's timeouts, empty when there were none.
pub fn ec_note() -> String {
    match EC_TIMEOUTS.load(Ordering::Relaxed) {
        0 => String::new(),
        n => format!("ec timeouts {n} (last status {:#04x})\n", EC_LAST_STATUS.load(Ordering::Relaxed)),
    }
}

impl Ec {
    /// Wait until the status says `ready`.
    fn wait(&self, ready: impl Fn(u8) -> bool) -> Result<(), AmlError> {
        let end = api().monotonic_ns() + EC_WAIT_NS;
        loop {
            if ready(inb(self.command)) {
                return Ok(());
            }
            if api().monotonic_ns() > end {
                EC_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
                EC_LAST_STATUS.store(u32::from(inb(self.command)), Ordering::Relaxed);
                return Err(AmlError::LibUnimplemented);
            }
            core::hint::spin_loop();
        }
    }

    fn read(&self, address: u8) -> Result<u8, AmlError> {
        self.wait(|s| s & EC_IBF == 0)?;
        outb(self.command, EC_READ);
        self.wait(|s| s & EC_IBF == 0)?;
        outb(self.data, address);
        self.wait(|s| s & EC_OBF != 0)?;
        Ok(inb(self.data))
    }

    fn write(&self, address: u8, value: u8) -> Result<(), AmlError> {
        self.wait(|s| s & EC_IBF == 0)?;
        outb(self.command, EC_WRITE);
        self.wait(|s| s & EC_IBF == 0)?;
        outb(self.data, address);
        self.wait(|s| s & EC_IBF == 0)?;
        outb(self.data, value);
        self.wait(|s| s & EC_IBF == 0)
    }

    /// `bytes` bytes from `offset` in `region`, the first the lowest.
    fn read_n(&self, region: &OpRegion, offset: usize, bytes: usize) -> Result<u64, AmlError> {
        let mut v = 0;
        for i in 0..bytes {
            v |= u64::from(self.read((region.base as usize + offset + i) as u8)?) << (i * 8);
        }
        Ok(v)
    }

    fn write_n(&self, region: &OpRegion, offset: usize, bytes: usize, value: u64) -> Result<(), AmlError> {
        for i in 0..bytes {
            self.write((region.base as usize + offset + i) as u8, (value >> (i * 8)) as u8)?;
        }
        Ok(())
    }
}

impl RegionHandler for Ec {
    fn read_u8(&self, region: &OpRegion, offset: usize) -> Result<u8, AmlError> {
        self.read_n(region, offset, 1).map(|v| v as u8)
    }
    fn read_u16(&self, region: &OpRegion, offset: usize) -> Result<u16, AmlError> {
        self.read_n(region, offset, 2).map(|v| v as u16)
    }
    fn read_u32(&self, region: &OpRegion, offset: usize) -> Result<u32, AmlError> {
        self.read_n(region, offset, 4).map(|v| v as u32)
    }
    fn read_u64(&self, region: &OpRegion, offset: usize) -> Result<u64, AmlError> {
        self.read_n(region, offset, 8)
    }
    fn write_u8(&self, region: &OpRegion, offset: usize, value: u8) -> Result<(), AmlError> {
        self.write_n(region, offset, 1, value.into())
    }
    fn write_u16(&self, region: &OpRegion, offset: usize, value: u16) -> Result<(), AmlError> {
        self.write_n(region, offset, 2, value.into())
    }
    fn write_u32(&self, region: &OpRegion, offset: usize, value: u32) -> Result<(), AmlError> {
        self.write_n(region, offset, 4, value.into())
    }
    fn write_u64(&self, region: &OpRegion, offset: usize, value: u64) -> Result<(), AmlError> {
        self.write_n(region, offset, 8, value)
    }
}

/// The devices whose `_HID` is `id` (an EISA id) or `s` (a string).
fn devices(interp: &Interpreter<Host>, id: Option<u64>, s: Option<&str>) -> Vec<AmlName> {
    let mut all = Vec::new();
    let _ = interp.namespace.lock().traverse(|path, level| {
        if level.kind == NamespaceLevelKind::Device {
            all.push(path.clone());
        }
        Ok(true)
    });
    all.into_iter()
        .filter(|path| {
            let Ok(hid) = name("_HID").resolve(path) else {
                return false;
            };
            match interp.evaluate_if_present(hid, vec![]) {
                Ok(Some(v)) => match &*v {
                    Object::Integer(v) => Some(*v) == id,
                    Object::String(v) => Some(v.as_str()) == s,
                    _ => false,
                },
                _ => false,
            }
        })
        .collect()
}

/// `n` of the device at `path`, evaluated.
fn eval(interp: &Interpreter<Host>, path: &AmlName, n: &str) -> Option<acpi::aml::object::WrappedObject> {
    interp.evaluate_if_present(name(n).resolve(path).ok()?, vec![]).ok()?
}

/// The same for `/proc/acpi/battery`, where a method that fails says so on
/// a line of its own (`BAT0 error _BST: ...`), its error the interpreter's:
/// an unknown state with no reason told nothing (a ThinkPad T15's `_BST`
/// and `_PSR`).
fn eval_said(
    interp: &Interpreter<Host>,
    path: &AmlName,
    n: &str,
    out: &mut String,
) -> Option<acpi::aml::object::WrappedObject> {
    let resolved = name(n).resolve(path).ok()?;
    match interp.evaluate_if_present(resolved, vec![]) {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(out, "{} error {n}: {e:?}", short(path));
            None
        }
    }
}

/// Find the embedded controller and handle its operation regions: its
/// ports from its `_CRS`, before the namespace's `_INI`s, which may read
/// it. A note for the report.
pub fn install_ec(interp: &Interpreter<Host>) -> String {
    let Some(path) = devices(interp, Some(PNP0C09), None).into_iter().next() else {
        return String::from("ec none\n");
    };
    let ports: Vec<u16> = eval(interp, &path, "_CRS")
        .and_then(|crs| resource_descriptor_list(crs).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| match r {
            Resource::IOPort(p) => Some(p.memory_range.0),
            _ => None,
        })
        .collect();
    let [data, command, ..] = ports[..] else {
        return format!("ec {}: no ports in _CRS\n", path.as_string());
    };
    interp.install_region_handler(RegionSpace::EmbeddedControl, Ec { data, command });
    format!("ec {} data {data:#x} command {command:#x}\n", path.as_string())
}

/// Tell the AML the controller's regions can be used (`_REG (3, 1)` of
/// the controller, which laptops' AML waits for before it reads them):
/// after the namespace is set up.
pub fn reg_ec(interp: &Interpreter<Host>) {
    if let Some(path) = devices(interp, Some(PNP0C09), None).into_iter().next() {
        if let Ok(reg) = name("_REG").resolve(&path) {
            let _ = interp.evaluate_if_present(reg, vec![Object::Integer(3).wrap(), Object::Integer(1).wrap()]);
        }
    }
}

/// The last component of `path`, its trailing `_` padding off (`BAT0`).
fn short(path: &AmlName) -> String {
    let s = path.as_string();
    String::from(s.rsplit('.').next().unwrap_or(&s).trim_start_matches('\\').trim_end_matches('_'))
}

/// A package's integer `i`, `None` when it is not one or reads unknown
/// (all ones, the specification's "unknown").
fn int(items: &[acpi::aml::object::WrappedObject], i: usize) -> Option<u64> {
    let v = items.get(i)?.as_integer().ok()?;
    (v != 0xffff_ffff && v != u64::MAX).then_some(v)
}

/// The battery at `path`, as lines of `/proc/acpi/battery`.
fn battery(interp: &Interpreter<Host>, path: &AmlName, out: &mut String) {
    let n = short(path);
    let sta = eval_said(interp, path, "_STA", out).and_then(|v| v.as_integer().ok()).unwrap_or(0x1f);
    if sta & 0x10 == 0 {
        let _ = writeln!(out, "{n} present 0");
        return;
    }
    let _ = writeln!(out, "{n} present 1");
    // `_BIX` (revision, unit, design, full, ...) or `_BIF` (unit, design,
    // full, ...): the unit 0 is mWh (rates mW), 1 mAh (mA).
    let (unit, design, full) = match eval_said(interp, path, "_BIX", out).as_deref() {
        Some(Object::Package(p)) => (int(p, 1), int(p, 2), int(p, 3)),
        _ => match eval_said(interp, path, "_BIF", out).as_deref() {
            Some(Object::Package(p)) => (int(p, 0), int(p, 1), int(p, 2)),
            _ => (None, None, None),
        },
    };
    let (energy, power) = if unit == Some(1) { ("mAh", "mA") } else { ("mWh", "mW") };
    let Some(Object::Package(bst)) = eval_said(interp, path, "_BST", out).as_deref().cloned() else {
        let _ = writeln!(out, "{n} state unknown");
        return;
    };
    let state = int(&bst, 0).unwrap_or(0);
    let s = if state & 2 != 0 {
        "charging"
    } else if state & 1 != 0 {
        "discharging"
    } else if full.is_some() && int(&bst, 2) >= full {
        "full"
    } else {
        "idle"
    };
    let _ = writeln!(out, "{n} state {s}");
    if state & 4 != 0 {
        let _ = writeln!(out, "{n} critical 1");
    }
    let remaining = int(&bst, 2);
    if let (Some(r), Some(f)) = (remaining, full) {
        if f != 0 {
            let _ = writeln!(out, "{n} percent {}", (r * 100 / f).min(100));
        }
    }
    let field = |out: &mut String, key: &str, v: Option<u64>, unit: &str| {
        if let Some(v) = v {
            let _ = writeln!(out, "{n} {key} {v} {unit}");
        }
    };
    field(out, "remaining", remaining, energy);
    field(out, "full", full, energy);
    field(out, "design", design, energy);
    field(out, "rate", int(&bst, 1), power);
    field(out, "voltage", int(&bst, 3), "mV");
}

/// The whole file: each battery's lines, then each adapter's.
fn report(interp: &Interpreter<Host>, batteries: &[AmlName], adapters: &[AmlName]) -> String {
    let mut out = String::new();
    for b in batteries {
        battery(interp, b, &mut out);
    }
    for a in adapters {
        let online = eval_said(interp, a, "_PSR", &mut out).and_then(|v| v.as_integer().ok());
        match online {
            Some(v) => {
                let _ = writeln!(out, "{} online {}", short(a), u64::from(v != 0));
            }
            None => {
                let _ = writeln!(out, "{} online unknown", short(a));
            }
        }
    }
    out
}

/// Reads asked for and answered: a read moves `REQUEST`, the thread
/// answers with the text in `TEXT` and moves `ANSWERED` to it.
static REQUEST: AtomicU32 = AtomicU32::new(0);
static ANSWERED: AtomicU32 = AtomicU32::new(0);
static TEXT: Lock<String> = Lock::new(String::new());
/// One read at a time.
static READING: SleepLock<()> = SleepLock::new(&API, ());

/// How long a read waits for the interpreter's answer.
const ANSWER_NS: u64 = 5_000_000_000;

/// The batteries and AC adapters, found once the namespace is set up.
pub struct Power {
    batteries: Vec<AmlName>,
    adapters: Vec<AmlName>,
}

/// Find the batteries and AC adapters and, with any, publish
/// `/proc/acpi/battery` (its reads answered by [`answer`]). A note for the
/// report.
pub fn find(interp: &Interpreter<Host>, note: &mut String) -> Option<Power> {
    let power = Power { batteries: devices(interp, Some(PNP0C0A), None), adapters: devices(interp, None, Some(ACPI0003)) };
    for d in power.batteries.iter().chain(&power.adapters) {
        let _ = writeln!(note, "power {}", d.as_string());
    }
    if power.batteries.is_empty() && power.adapters.is_empty() {
        note.push_str("power none\n");
        return None;
    }
    let api = api();
    if api.proc_register("acpi/battery", b"") != 0 || api.proc_set_reader("acpi/battery", Some(read)) != 0 {
        note.push_str("power: /proc/acpi/battery not registered\n");
        return None;
    }
    Some(power)
}

/// Answer the reads of `/proc/acpi/battery`, for good: the interpreter's
/// thread, once its boot work is done.
pub fn answer(interp: &Interpreter<Host>, power: &Power) -> ! {
    let api = api();
    loop {
        let seq = api.wait_seq();
        let asked = REQUEST.load(Ordering::Acquire);
        if asked != ANSWERED.load(Ordering::Acquire) {
            crate::aml::step("reading /proc/acpi/battery");
            *TEXT.lock() = report(interp, &power.batteries, &power.adapters);
            ANSWERED.store(asked, Ordering::Release);
            api.wake(&ANSWERED as *const _ as usize);
            continue;
        }
        api.block_until(&REQUEST as *const _ as usize, seq, 0);
    }
}

/// `/proc/acpi/battery`'s reader: ask the interpreter's thread, wait for
/// its answer (a bounded time: the thread may have stopped on AML it
/// could not run), copy it out.
unsafe extern "C" fn read(buf: *mut u8, cap: usize) -> i32 {
    let api = api();
    let _one = READING.lock();
    // An error names why, on one line: the panic that stopped the
    // interpreter, or the controller's timeouts behind a slow answer.
    let stopped = || {
        let why = crate::aml::panic_line().unwrap_or_default();
        format!("error interpreter stopped: {why} {}\n", ec_note().trim_end())
    };
    let text = if crate::aml::stopped() {
        stopped()
    } else {
        let asked = REQUEST.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        api.wake(&REQUEST as *const _ as usize);
        let deadline = api.monotonic_ns() + ANSWER_NS;
        loop {
            let seq = api.wait_seq();
            if ANSWERED.load(Ordering::Acquire) == asked {
                break TEXT.lock().clone();
            }
            if crate::aml::stopped() {
                break stopped();
            }
            if api.monotonic_ns() >= deadline {
                break format!("error no answer from the interpreter in {} s {}\n", ANSWER_NS / 1_000_000_000, ec_note().trim_end());
            }
            api.block_until(&ANSWERED as *const _ as usize, seq, deadline);
        }
    };
    let n = text.len().min(cap);
    // SAFETY: the kernel gave `buf` with room for `cap` bytes.
    unsafe { core::ptr::copy_nonoverlapping(text.as_ptr(), buf, n) };
    text.len() as i32
}
