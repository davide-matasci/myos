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
//!
//! Behind the root, a secondary controller (a GPIO block's lines, behind
//! one of the root's) is an interrupt domain a module drives
//! ([`domain_register`], `KernelApi::irq_domain_register`): its lines are
//! interrupt numbers from [`DOMAIN_FIRST`] up, which [`route`] and
//! [`unmask`] take to the domain instead of the root, which a specifier
//! whose parent is the domain's node decodes to ([`xlate`]), and which the
//! module's handler of its own line dispatches (`irq::dispatch`).

use spin::{Mutex, Once};

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
    /// Deliver device interrupt `irq` (after [`IrqChip::route`]) to the BSP,
    /// level-triggered.
    fn unmask(&self, irq: u32);
    /// [`IrqChip::unmask`] for a line its device pulses (the SMMU's event
    /// queue): edge-triggered where the controller is told the trigger,
    /// as `unmask` elsewhere.
    #[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))]
    fn unmask_edge(&self, irq: u32) {
        self.unmask(irq);
    }
    /// Deliver device interrupt `irq` no more (its last handler went:
    /// `irq::disable`, `irq::pci_irq_disable`); [`IrqChip::unmask`]
    /// delivers it again.
    fn mask(&self, irq: u32);
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
    fn mask(&self, _irq: u32) {}
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
/// the domain or the driver of its interrupt parent: `None` when neither
/// is made of that node.
pub fn xlate(spec: &crate::dt::IrqSpec) -> Option<u32> {
    if let Some(d) = DOMAINS.lock().iter().flatten().find(|d| d.phandle != 0 && d.phandle == spec.parent()) {
        // The first cell is the line, as every GPIO controller has it.
        return spec.cells().first().filter(|&&line| line < d.count).map(|line| d.first + line);
    }
    crate::arch::IRQCHIPS.iter().find_map(|d| {
        let node = d.dt.iter().position(|n| crate::dt::phandle_compatible(spec.parent(), n.compatible))?;
        (d.xlate)(node, spec.cells())
    })
}

/// The `irq::dispatch` number of device interrupt `irq`, still masked
/// ([`IrqChip::route`]): a domain's line is its own number.
pub fn route(irq: u32) -> Option<u32> {
    if domain_of(irq).is_some() {
        return Some(irq);
    }
    root().route(irq)
}

/// Deliver device interrupt `irq` (after [`route`]): through its domain,
/// or the root ([`IrqChip::unmask`]).
pub fn unmask(irq: u32) {
    match domain_of(irq) {
        Some(d) => unsafe { (d.unmask)(d.ctx as *mut core::ffi::c_void, irq - d.first) },
        None => root().unmask(irq),
    }
}

/// Stop device interrupt `irq` (its last handler went): through its
/// domain, or the root ([`IrqChip::mask`]).
pub fn mask(irq: u32) {
    match domain_of(irq) {
        Some(d) => unsafe { (d.mask)(d.ctx as *mut core::ffi::c_void, irq - d.first) },
        None => root().mask(irq),
    }
}

/// The first interrupt number of the domains: above every root
/// controller's (an x86 vector, a GIC INTID, a PLIC source).
pub const DOMAIN_FIRST: u32 = 4096;
/// The domains, and the lines they have between them.
const MAX_DOMAINS: usize = 8;
const DOMAIN_LINES: u32 = 4096;

/// A secondary controller a module drives: its lines are interrupts
/// `first..first + count`.
#[derive(Clone, Copy)]
struct Domain {
    /// Its device tree node's phandle, 0 when it has none.
    phandle: u32,
    first: u32,
    count: u32,
    /// Deliver line `line` (`0..count`) of the controller, or no more.
    unmask: LineFn,
    mask: LineFn,
    ctx: usize,
}

/// A domain's `unmask` or `mask` of one of its lines.
pub type LineFn = unsafe extern "C" fn(ctx: *mut core::ffi::c_void, line: u32);

static DOMAINS: Mutex<[Option<Domain>; MAX_DOMAINS]> = Mutex::new([None; MAX_DOMAINS]);

fn domain_of(irq: u32) -> Option<Domain> {
    DOMAINS.lock().iter().flatten().find(|d| (d.first..d.first + d.count).contains(&irq)).copied()
}

/// A domain of `count` lines for the device tree node `phandle` (0 for a
/// controller without one), whose line `n` `unmask(ctx, n)` delivers and
/// `mask(ctx, n)` stops: its
/// first interrupt number, `None` when the domains are full or the node
/// has one already. A domain stays for as long as the kernel runs (its
/// module stays loaded: a registration).
pub fn domain_register(phandle: u32, count: u32, unmask: LineFn, mask: LineFn, ctx: usize) -> Option<u32> {
    let mut domains = DOMAINS.lock();
    if count == 0 || (phandle != 0 && domains.iter().flatten().any(|d| d.phandle == phandle)) {
        return None;
    }
    let first = domains.iter().flatten().map(|d| d.first + d.count).max().unwrap_or(DOMAIN_FIRST);
    if first.checked_add(count).is_none_or(|end| end > DOMAIN_FIRST + DOMAIN_LINES) {
        return None;
    }
    let slot = domains.iter_mut().find(|d| d.is_none())?;
    *slot = Some(Domain { phandle, first, count, unmask, mask, ctx });
    Some(first)
}
