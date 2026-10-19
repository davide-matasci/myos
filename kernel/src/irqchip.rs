//! Interrupt controllers: one interface for every controller the kernel
//! drives as the board's root (x86_64's local APIC, the GICv2 and the
//! GICv3, the Raspberry Pi 3's two, the RISC-V PLIC), and one table per
//! arch of the drivers it has (`arch::IRQCHIPS`). Adding a controller is a
//! file that implements [`IrqChip`] and a [`Driver`] entry in its arch's
//! table: the platform description, the device tree's interrupt
//! specifiers and every caller find it through these.
//!
//! A [`Driver`] is what is known before the controller runs: its name
//! (`/proc/platform`, the ACPI tables' choice), the device tree nodes it
//! is made of, how it reads their interrupt specifiers ([`Driver::xlate`])
//! and how it starts. [`start`] picks the one the platform description
//! names (`arch::apply_platform`); the running controller is [`root`].
//!
//! The interrupt numbers are each arch's `irq::dispatch` numbers: an x86
//! LAPIC vector; on aarch64 the GIC's INTIDs, which every aarch64
//! controller here uses (SGI 0 and 1 the IPIs, PPI 27 and 30 the timers,
//! 32 and up the devices: the arch's interrupt entry reads them so); a
//! PLIC source on riscv64.

use spin::Once;

use crate::platform::Intc;

/// The IPIs the kernel sends (`smp`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Ipi {
    /// Flush the TLB (`smp::tlb_shootdown`).
    Tlb,
    /// Look for a task to run (`smp::kick_cpu`, `smp::kick_cpus`).
    Resched,
}

/// Where an IPI goes: one logical CPU, or every online CPU but this one.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Cpu(usize),
    Others,
}

/// A running interrupt controller.
pub trait IrqChip: Sync {
    /// For the boot log.
    fn name(&self) -> &'static str;
    /// This CPU's side of the controller on: the BSP's from the arch's
    /// interrupt setup, then each AP's from its start-up.
    fn init_cpu(&self);
    /// The `irq::dispatch` number of device interrupt `irq` (the number
    /// [`Driver::xlate`], the SPCR or the arch gives it), still masked:
    /// `None` when the controller cannot deliver it. The number itself on
    /// most; x86_64 gives an ISA interrupt a LAPIC vector.
    fn route(&self, irq: u32) -> Option<u32>;
    /// Deliver device interrupt `irq` (after [`IrqChip::route`]) to the BSP.
    fn unmask(&self, irq: u32);
    /// The pending interrupt this CPU takes, acknowledged: `None` when
    /// nothing is pending. The controllers the CPU asks (the GIC, the
    /// PLIC, the Pi's); x86_64's vectors say which one it is.
    #[cfg_attr(target_arch = "x86_64", allow(dead_code))]
    fn claim(&self) -> Option<u32>;
    /// The end of interrupt `id` (from [`IrqChip::claim`]), after its
    /// handlers ran.
    fn complete(&self, id: u32);
    /// Send `ipi`.
    fn send_ipi(&self, to: Target, ipi: Ipi);
}

/// One device tree node of a controller: its `compatible` strings, and
/// the `reg` entries the description takes from it, in order.
pub struct DtNode {
    pub compatible: &'static [&'static str],
    pub regs: usize,
}

/// A controller the kernel can drive.
pub struct Driver {
    /// Its name in the platform description (`Intc::driver`).
    pub name: &'static str,
    /// The device tree nodes it is made of, every one required; empty for
    /// a controller the tree does not describe (x86_64's).
    pub dt: &'static [DtNode],
    /// Decode an interrupt specifier whose `interrupt-parent` is its node
    /// `node` (an index into [`Driver::dt`]): the interrupt number, `None`
    /// for one it does not deliver.
    pub xlate: fn(node: usize, cells: &[u32]) -> Option<u32>,
    /// Map its registers from the description and return it, running
    /// (its CPU side waits for [`IrqChip::init_cpu`]).
    pub start: fn(&Intc) -> Result<&'static dyn IrqChip, &'static str>,
}

/// The driver called `name`, of this arch.
pub fn driver(name: &str) -> Option<&'static Driver> {
    crate::arch::IRQCHIPS.iter().copied().find(|d| d.name == name)
}

/// Before [`start`]: nothing delivered, nothing sent.
struct NoChip;

impl IrqChip for NoChip {
    fn name(&self) -> &'static str {
        "none"
    }
    fn init_cpu(&self) {}
    fn route(&self, _irq: u32) -> Option<u32> {
        None
    }
    fn unmask(&self, _irq: u32) {}
    fn claim(&self) -> Option<u32> {
        None
    }
    fn complete(&self, _id: u32) {}
    fn send_ipi(&self, _to: Target, _ipi: Ipi) {}
}

static ROOT: Once<&'static dyn IrqChip> = Once::new();

/// Start the controller the platform description names, or the arch's own
/// when it names none (`arch::IRQCHIP_DEFAULT`: a PC without ACPI tables).
pub fn start(p: &crate::platform::Platform) -> Result<(), &'static str> {
    let (driver, intc) = match p.intc.map(|c| c.value) {
        Some(intc) => (driver(intc.driver).ok_or("platform: no driver for the interrupt controller")?, intc),
        None => {
            let driver = crate::arch::IRQCHIP_DEFAULT.ok_or("platform: no interrupt controller")?;
            (driver, Intc::new(driver.name, &[]))
        }
    };
    let chip = (driver.start)(&intc)?;
    ROOT.call_once(|| chip);
    Ok(())
}

/// The running controller ([`start`]).
pub fn root() -> &'static dyn IrqChip {
    ROOT.get().copied().unwrap_or(&NoChip)
}

/// The interrupt number of a device tree interrupt specifier, decoded by
/// the driver of its interrupt parent: `None` when no driver of this arch
/// is made of that node.
pub fn xlate(spec: &crate::dt::IrqSpec) -> Option<u32> {
    crate::arch::IRQCHIPS.iter().find_map(|d| {
        let node = d.dt.iter().position(|n| crate::dt::phandle_compatible(spec.parent(), n.compatible))?;
        (d.xlate)(node, spec.cells())
    })
}
