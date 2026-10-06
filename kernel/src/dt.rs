//! The flattened device tree (FDT) Limine hands over on aarch64 and riscv64:
//! the board's description. `fill` turns it into the kernel's platform
//! description (`crate::platform`), the second source after the ACPI
//! tables where the firmware has them; the accessors below serve what only
//! the tree describes afterwards: the virtio-mmio nodes modules look up
//! (`KernelApi::dt_mmio_find`) and the PCI INTx routing. x86_64 has no
//! tree (ACPI instead): `init` finds none and the accessors stay unused.
//!
//! Lookups are by `compatible` string. Interrupt specifiers are returned
//! raw (cells per the interrupt parent's `#interrupt-cells`) and decoded by
//! `arch::irq_from_dt`, since their meaning depends on the controller.
//! Everything here runs before the heap exists, so nothing allocates.

use fdt::Fdt;
use fdt::node::FdtNode;
use spin::Once;

use crate::platform::{Platform, Uart, UartKind};

static FDT: Once<Fdt<'static>> = Once::new();

/// Parse the tree Limine found. False without one (x86_64, or a loader
/// that passed none).
pub fn init() -> bool {
    let Some(resp) = crate::limine_boot::DTB.response() else {
        return false;
    };
    let ptr = resp.dtb_ptr as *const u8;
    if ptr.is_null() {
        return false;
    }
    match unsafe { Fdt::from_ptr(ptr) } {
        Ok(fdt) => {
            FDT.call_once(|| fdt);
            true
        }
        Err(_) => false,
    }
}

pub fn get() -> Option<&'static Fdt<'static>> {
    FDT.get()
}

/// The root `model` string (QEMU: `linux,dummy-virt` / `riscv-virtio,qemu`).
pub fn model() -> Option<&'static str> {
    Some(get()?.root().model())
}

/// A property value as big-endian 32-bit cells.
#[derive(Clone, Copy)]
struct Cells<'a>(&'a [u8]);

impl<'a> Cells<'a> {
    fn len(&self) -> usize {
        self.0.len() / 4
    }

    fn get(&self, i: usize) -> Option<u32> {
        let b = self.0.get(i * 4..i * 4 + 4)?;
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Cells `i..i + n` joined big-endian into one number.
    fn join(&self, i: usize, n: usize) -> Option<u64> {
        let mut v = 0u64;
        for k in 0..n {
            v = (v << 32) | self.get(i + k)? as u64;
        }
        Some(v)
    }
}

fn prop<'b>(node: FdtNode<'b, 'static>, name: &str) -> Option<Cells<'static>> {
    Some(Cells(node.property(name)?.value))
}

fn has_compatible(node: FdtNode, compat: &str) -> bool {
    node.compatible()
        .map(|c| c.all().any(|s| s == compat))
        .unwrap_or(false)
}

/// `reg` entry `index` of `node`: `(base, size)`.
fn node_reg(node: FdtNode, index: usize) -> Option<(u64, u64)> {
    let r = node.reg()?.nth(index)?;
    Some((r.starting_address as u64, r.size.unwrap_or(0) as u64))
}

/// `#address-cells` of the parent of `node`, as its `reg` is encoded.
fn parent_address_cells(node: FdtNode) -> Option<usize> {
    Some(node.raw_reg()?.next()?.address.len() / 4)
}

/// `reg` entry `index` of the first node compatible with any of `compat`.
pub fn reg(compat: &[&str], index: usize) -> Option<(u64, u64)> {
    node_reg(get()?.find_compatible(compat)?, index)
}

/// An interrupt specifier: `cells()`, per the interrupt parent.
#[derive(Clone, Copy)]
pub struct IrqSpec {
    cells: [u32; 4],
    n: usize,
}

impl IrqSpec {
    pub fn cells(&self) -> &[u32] {
        &self.cells[..self.n]
    }
}

/// The interrupt parent of `node`: its own `interrupt-parent`, else the
/// root's (the usual single-controller board).
fn interrupt_parent<'b>(
    fdt: &'b Fdt<'static>,
    node: FdtNode<'b, 'static>,
) -> Option<FdtNode<'b, 'static>> {
    let phandle = node
        .property("interrupt-parent")
        .or_else(|| fdt.root().property("interrupt-parent"))?;
    fdt.find_phandle(Cells(phandle.value).get(0)?)
}

/// First `interrupts` specifier of `node`.
fn interrupt_spec(fdt: &Fdt<'static>, node: FdtNode<'_, 'static>) -> Option<IrqSpec> {
    let n = interrupt_parent(fdt, node)?.interrupt_cells()?;
    if n == 0 || n > 4 {
        return None;
    }
    let ints = prop(node, "interrupts")?;
    let mut spec = IrqSpec { cells: [0; 4], n };
    for i in 0..n {
        spec.cells[i] = ints.get(i)?;
    }
    Some(spec)
}

/// A memory-mapped device node.
#[derive(Clone, Copy)]
pub struct MmioDevice {
    pub base: u64,
    pub size: u64,
    pub irq: Option<IrqSpec>,
}

/// The `index`-th node compatible with `compat`, in ascending `reg` order
/// (device names such as `/dev/vda` follow it, whatever the tree's order).
pub fn mmio_device(compat: &str, index: usize) -> Option<MmioDevice> {
    let fdt = get()?;
    let matching = || {
        fdt.all_nodes()
            .filter(|n| has_compatible(*n, compat))
            .filter_map(|n| node_reg(n, 0).map(|(base, size)| (base, size, n)))
    };
    // Selection by rank: the match with exactly `index` smaller bases.
    let (base, size, node) =
        matching().find(|(b, _, _)| matching().filter(|(o, _, _)| o < b).count() == index)?;
    Some(MmioDevice {
        base,
        size,
        irq: interrupt_spec(fdt, node),
    })
}

/// The PCIe host bridge (`pci-host-ecam-generic`).
pub struct PciHost {
    pub ecam_base: u64,
    pub ecam_size: u64,
    /// Last bus of `bus-range` (255 when absent).
    pub bus_end: u8,
}

pub fn pci_host() -> Option<PciHost> {
    let host = get()?.find_compatible(&["pci-host-ecam-generic"])?;
    let (ecam_base, ecam_size) = node_reg(host, 0)?;
    let bus_end = match prop(host, "bus-range") {
        Some(r) => r.get(1)? as u8,
        None => 255,
    };
    Some(PciHost {
        ecam_base,
        ecam_size,
        bus_end,
    })
}

/// PCI address-space flags of a `ranges` entry (its first child cell).
pub const PCI_SPACE_MASK: u32 = 0x0300_0000;
pub const PCI_SPACE_MEM32: u32 = 0x0200_0000;
pub const PCI_SPACE_MEM64: u32 = 0x0300_0000;

/// The first MMIO window of the host bridge whose space flags equal
/// `space` (`PCI_SPACE_MEM32` / `PCI_SPACE_MEM64`): `(cpu_base, size)`.
pub fn pci_mmio_window(space: u32) -> Option<(u64, u64)> {
    let host = get()?.find_compatible(&["pci-host-ecam-generic"])?;
    let ranges = prop(host, "ranges")?;
    let child = host.cell_sizes().address_cells; // 3: flags, address hi, lo
    let size = host.cell_sizes().size_cells;
    let parent = parent_address_cells(host)?;
    let entry = child + parent + size;
    if child == 0 || entry == 0 {
        return None;
    }
    let mut i = 0;
    while i + entry <= ranges.len() {
        if ranges.get(i)? & PCI_SPACE_MASK == space {
            return Some((ranges.join(i + child, parent)?, ranges.join(i + child + parent, size)?));
        }
        i += entry;
    }
    None
}

/// Interrupt specifier for INTx `pin` (1..=4) of the PCI function
/// `bus:slot.func`, from the host bridge's `interrupt-map` and
/// `interrupt-map-mask` (the slot swizzle is whatever the board says).
pub fn pci_intx(bus: u8, slot: u8, func: u8, pin: u8) -> Option<IrqSpec> {
    let fdt = get()?;
    let host = fdt.find_compatible(&["pci-host-ecam-generic"])?;
    let addr_cells = host.cell_sizes().address_cells; // 3
    if addr_cells == 0 || addr_cells > 3 || host.interrupt_cells()? != 1 {
        return None;
    }
    let mask = prop(host, "interrupt-map-mask")?;
    let map = prop(host, "interrupt-map")?;
    // Child unit address: the config-space address of the function.
    let unit = ((bus as u32) << 16) | ((slot as u32) << 11) | ((func as u32) << 8);
    let want_pin = pin as u32 & mask.get(addr_cells)?;
    let mut i = 0;
    // Entry: child address, child interrupt, parent phandle, parent unit
    // address (parent `#address-cells`), parent interrupt specifier.
    while i + addr_cells + 2 <= map.len() {
        let parent = fdt.find_phandle(map.get(i + addr_cells + 1)?)?;
        let pa = prop(parent, "#address-cells").and_then(|c| c.get(0)).unwrap_or(0) as usize;
        let pi = parent.interrupt_cells()?;
        let spec_at = i + addr_cells + 2 + pa;
        if pi == 0 || pi > 4 || spec_at + pi > map.len() {
            return None;
        }
        let mut matches = map.get(i)? == (unit & mask.get(0)?);
        for k in 1..addr_cells {
            matches &= map.get(i + k)? == 0;
        }
        matches &= map.get(i + addr_cells)? == want_pin;
        if matches {
            let mut spec = IrqSpec { cells: [0; 4], n: pi };
            for k in 0..pi {
                spec.cells[k] = map.get(spec_at + k)?;
            }
            return Some(spec);
        }
        i = spec_at + pi;
    }
    None
}

/// `timebase-frequency` of the CPUs (riscv64: the `time` CSR rate).
pub fn timebase_frequency() -> Option<u64> {
    let cpus = get()?.find_node("/cpus")?;
    cpus.property("timebase-frequency")?.as_usize().map(|v| v as u64)
}

/// GICv2 `compatible` strings (QEMU `virt`: `arm,cortex-a15-gic`).
const GICV2_COMPAT: &[&str] = &["arm,cortex-a15-gic", "arm,gic-400", "arm,cortex-a9-gic"];

/// The first node compatible with `compat`, as a UART of `kind`: base,
/// first interrupt, and the `reg-shift` / `reg-io-width` of a 16550.
fn uart(compat: &[&str], kind: UartKind) -> Option<Uart> {
    let fdt = get()?;
    let node = fdt.find_compatible(compat)?;
    let (base, _) = node_reg(node, 0)?;
    let irq = interrupt_spec(fdt, node).and_then(|s| crate::arch::irq_from_dt(s.cells()));
    let cell = |name: &str, default: u32| match kind {
        UartKind::Ns16550 => prop(node, name).and_then(|c| c.get(0)).unwrap_or(default),
        UartKind::Pl011 => default,
    };
    Some(Uart {
        kind,
        base,
        io: false,
        irq,
        reg_shift: cell("reg-shift", 0) as u8,
        reg_width: cell("reg-io-width", 1) as u8,
    })
}

/// Describe the board from the tree (`crate::platform::init`): the model,
/// the CPUs, the interrupt controller (a GICv2, a GICv3 or a PLIC), the
/// console UART (a PL011 or a 16550), the RTC (a PL031 or a goldfish), the
/// `time` CSR rate and the PCIe host bridge with its MMIO windows.
pub fn fill(p: &mut Platform) {
    use crate::platform::*;
    let Some(fdt) = FDT.get() else {
        return;
    };
    let from = Source::DeviceTree;
    offer_model(p, fdt.root().model());
    let cpus = fdt.cpus().count();
    if cpus != 0 {
        offer_cpus(p, cpus, from);
    }
    if let (Some((gicd, _)), Some((gicc, _))) = (reg(GICV2_COMPAT, 0), reg(GICV2_COMPAT, 1)) {
        offer_intc(p, Intc::GicV2 { gicd, gicc }, from);
    } else if let (Some((gicd, _)), Some((gicr, gicr_size))) =
        (reg(&["arm,gic-v3"], 0), reg(&["arm,gic-v3"], 1))
    {
        offer_intc(p, Intc::GicV3 { gicd, gicr, gicr_size }, from);
    } else if let Some((base, _)) = reg(&["sifive,plic-1.0.0", "riscv,plic0"], 0) {
        offer_intc(p, Intc::Plic { base }, from);
    }
    if let Some(uart) = uart(&["arm,pl011"], UartKind::Pl011) {
        offer_uart(p, uart, from);
    } else if let Some(uart) = uart(&["ns16550a", "ns16550", "snps,dw-apb-uart"], UartKind::Ns16550) {
        offer_uart(p, uart, from);
    }
    if let Some((base, _)) = reg(&["arm,pl031"], 0) {
        offer_rtc(p, Rtc { kind: RtcKind::Pl031, base }, from);
    } else if let Some((base, _)) = reg(&["google,goldfish-rtc"], 0) {
        offer_rtc(p, Rtc { kind: RtcKind::Goldfish, base }, from);
    }
    if let Some(hz) = timebase_frequency() {
        offer_timer_hz(p, hz, from);
    }
    if let Some(host) = pci_host() {
        let bus_start = fdt
            .find_compatible(&["pci-host-ecam-generic"])
            .and_then(|h| prop(h, "bus-range"))
            .and_then(|r| r.get(0))
            .unwrap_or(0) as u8;
        offer_pci(
            p,
            PciHost { ecam: host.ecam_base, ecam_size: host.ecam_size, bus_start, bus_end: host.bus_end },
            from,
        );
        let windows = PciWindows {
            mmio32: pci_mmio_window(PCI_SPACE_MEM32),
            mmio64: pci_mmio_window(PCI_SPACE_MEM64),
        };
        if windows.mmio32.is_some() || windows.mmio64.is_some() {
            offer_pci_windows(p, windows, from);
        }
    }
    let psci = fdt.find_compatible(&["arm,psci-1.0", "arm,psci-0.2", "arm,psci"]);
    match psci.and_then(|n| n.property("method")).and_then(|m| m.as_str()) {
        Some("hvc") => offer_psci(p, PsciConduit::Hvc, from),
        Some("smc") => offer_psci(p, PsciConduit::Smc, from),
        _ => {}
    }
}
