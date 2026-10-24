//! PCI INTx on x86_64: the interrupt routes the firmware's ACPI `_PRT`
//! gives (the acpi module's AML interpreter reads them and hands them over
//! with `pci_intx_route`), and a function's INTx line found through them
//! and routed through the I/O APIC. Only a function with neither MSI-X nor
//! MSI comes here (`pci_irq_setup`); without routes (no acpi module, its
//! interpreter failed) there is none, and its driver polls.
//!
//! A line is shared, and level-triggered: a function on it that does not
//! use it (a driver that polls, or one on MSI) could hold it asserted and
//! the CPU would take its interrupt for ever, no handler quietening it. So
//! before a line is unmasked every other function on it has its INTx
//! turned off (the command register's disable bit), except those whose
//! driver routed it ([`USERS`]).

use spin::Mutex;

use myos_abi::{PCI_INTX_ACTIVE_LOW, PCI_INTX_LEVEL};

/// One `_PRT` entry: pin `pin` (0 INTA to 3 INTD) of device `device` on
/// bus `bus` (a root bridge's, or a bridge's with a `_PRT`) raises `gsi`.
#[derive(Clone, Copy)]
struct Route {
    bus: u8,
    device: u8,
    pin: u8,
    gsi: u32,
    flags: u32,
}

/// Every device of a root bus has four pins: 32 × 4 entries for one, room
/// for a second.
const MAX_ROUTES: usize = 256;
/// Bridges between a function and its root bus at most.
const MAX_DEPTH: usize = 8;

/// Filled before the drivers ask, early in boot.
static ROUTES: Mutex<[Option<Route>; MAX_ROUTES]> = Mutex::new([None; MAX_ROUTES]);

/// The functions whose INTx a driver routed ([`setup`]): bus, slot,
/// function. The others' INTx stays off while their line is unmasked.
const MAX_USERS: usize = 64;
static USERS: Mutex<[Option<(u8, u8, u8)>; MAX_USERS]> = Mutex::new([None; MAX_USERS]);

/// The command register's INTx disable bit.
const CMD_INTX_OFF: u32 = 1 << 10;

/// Take a route (`KernelApi::pci_intx_route`): false when the table is
/// full or the route is not one (a pin past INTD, a device past 31). A
/// second route for the same pin replaces the first.
pub fn add(bus: u8, device: u8, pin: u8, gsi: u32, flags: u32) -> bool {
    if pin > 3 || device > 31 {
        return false;
    }
    let mut routes = ROUTES.lock();
    let same = |r: &Route| r.bus == bus && r.device == device && r.pin == pin;
    let slot = match routes.iter().position(|r| r.is_some_and(|r| same(&r))) {
        Some(i) => i,
        None => match routes.iter().position(Option::is_none) {
            Some(i) => i,
            None => return false,
        },
    };
    routes[slot] = Some(Route { bus, device, pin, gsi, flags });
    true
}

/// The bridge whose secondary bus is `bus`: its bus and device.
fn parent_bridge(bus: u8) -> Option<(u8, u8)> {
    crate::pci::functions().into_iter().find_map(|b| {
        // A PCI-to-PCI bridge (header type 1) and its secondary bus.
        let header = (crate::pci::cfg_read32(b.bus, b.slot, b.func, 0x0c) >> 16) & 0x7f;
        let secondary = (crate::pci::cfg_read32(b.bus, b.slot, b.func, 0x18) >> 8) as u8;
        (header == 1 && secondary == bus && b.bus != bus).then_some((b.bus, b.slot))
    })
}

/// The route of the function at `bus:slot` whose interrupt pin is `pin`
/// (0 INTA to 3 INTD): its own bus's entry, else its pin swizzled
/// through each bridge above it (the PCI-to-PCI Bridge Specification's
/// `(pin + device) mod 4`) to the first bus with an entry for it.
fn find(bus: u8, slot: u8, pin: u8) -> Option<Route> {
    let (mut bus, mut slot, mut pin) = (bus, slot, pin);
    for _ in 0..=MAX_DEPTH {
        let hit = ROUTES.lock().iter().flatten().copied().find(|r| r.bus == bus && r.device == slot && r.pin == pin);
        if hit.is_some() {
            return hit;
        }
        let (parent_bus, parent_slot) = parent_bridge(bus)?;
        pin = (pin + slot) % 4;
        (bus, slot) = (parent_bus, parent_slot);
    }
    None
}

/// The route of the function at `bus:slot.func`'s INTx pin, if it has a
/// pin and a route.
fn route_of(bus: u8, slot: u8, func: u8) -> Option<Route> {
    let pin = (crate::pci::cfg_read32(bus, slot, func, 0x3c) >> 8) & 0xff;
    if pin == 0 || pin > 4 {
        return None;
    }
    find(bus, slot, pin as u8 - 1)
}

/// Set or clear the function's INTx disable bit.
fn intx_off(bus: u8, slot: u8, func: u8, off: bool) {
    let cmd = crate::pci::cfg_read32(bus, slot, func, 0x04) & 0xffff;
    let new = if off { cmd | CMD_INTX_OFF } else { cmd & !CMD_INTX_OFF };
    if new != cmd {
        crate::pci::cfg_write32(bus, slot, func, 0x04, new);
    }
}

/// Turn off the INTx of every function on `gsi` but the [`USERS`].
fn quiet(gsi: u32) {
    let users = *USERS.lock();
    for f in crate::pci::functions() {
        if users.contains(&Some((f.bus, f.slot, f.func))) {
            continue;
        }
        if route_of(f.bus, f.slot, f.func).is_some_and(|r| r.gsi == gsi) {
            intx_off(f.bus, f.slot, f.func, true);
        }
    }
}

/// The function's INTx through the I/O APIC: its vector (the
/// `irq::dispatch` number), the line's other functions quietened
/// ([`quiet`]), the pin unmasked, the function's INTx enabled. `None`
/// without a pin, a route, an I/O APIC pin for it or room in [`USERS`].
pub fn setup(bus: u8, slot: u8, func: u8) -> Option<u32> {
    let route = route_of(bus, slot, func)?;
    let level = route.flags & PCI_INTX_LEVEL != 0;
    let active_low = route.flags & PCI_INTX_ACTIVE_LOW != 0;
    let vector = super::ioapic::route_gsi(route.gsi, level, active_low)?;
    {
        let mut users = USERS.lock();
        let me = Some((bus, slot, func));
        if !users.contains(&me) {
            *users.iter_mut().find(|u| u.is_none())? = me;
        }
    }
    quiet(route.gsi);
    intx_off(bus, slot, func, false);
    super::ioapic::unmask_gsi(route.gsi);
    Some(vector)
}

/// The function's INTx given back (`irq::pci_irq_disable`): turned off and
/// no longer one of the [`USERS`]. Whether it was one (its interrupt was
/// its INTx).
pub fn release(bus: u8, slot: u8, func: u8) -> bool {
    let me = Some((bus, slot, func));
    {
        let mut users = USERS.lock();
        let Some(u) = users.iter_mut().find(|u| **u == me) else {
            return false;
        };
        *u = None;
    }
    intx_off(bus, slot, func, true);
    true
}
