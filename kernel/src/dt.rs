//! The flattened device tree (FDT) Limine hands over on aarch64 and riscv64:
//! the board's description. `fill` turns it into the kernel's platform
//! description (`crate::platform`), the second source after the ACPI
//! tables where the firmware has them; the accessors below serve what only
//! the tree describes afterwards: the virtio-mmio nodes modules look up
//! (`KernelApi::dt_mmio_find`) and the PCI INTx routing. x86_64 has no
//! tree (ACPI instead): `init` finds none and the accessors stay unused.
//!
//! Lookups are by `compatible` string, among the nodes not marked
//! `status = "disabled"` (a SoC's tree lists every block it has, a board
//! enables the ones it wires). A `reg` address is translated to the CPU's
//! through the `ranges` of the buses above the node (a Raspberry Pi 4's
//! peripherals sit at bus address 0x7e000000, CPU address 0xfe000000).
//! Interrupt specifiers are returned
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

/// The node is in use: no `status`, or `okay` (`ok` in older trees).
fn enabled(node: FdtNode) -> bool {
    match node.property("status").and_then(|p| p.as_str()) {
        None => true,
        Some(s) => s == "okay" || s == "ok",
    }
}

/// The first enabled node compatible with any of `compat`.
fn find_enabled<'b>(fdt: &'b Fdt<'static>, compat: &[&str]) -> Option<FdtNode<'b, 'static>> {
    fdt.all_nodes()
        .find(|n| enabled(*n) && compat.iter().any(|c| has_compatible(*n, c)))
}

/// Buses a node may be nested in, the root included.
const MAX_DEPTH: usize = 8;

/// The nodes from the root down to `target`'s parent, into `chain`: how
/// many. A node is told apart by its name's place in the tree's blob.
fn ancestors<'b>(
    node: FdtNode<'b, 'static>,
    target: *const u8,
    depth: usize,
    chain: &mut [Option<FdtNode<'b, 'static>>; MAX_DEPTH],
) -> Option<usize> {
    if depth >= MAX_DEPTH {
        return None;
    }
    chain[depth] = Some(node);
    for child in node.children() {
        if child.name.as_ptr() == target {
            return Some(depth + 1);
        }
        if let Some(n) = ancestors(child, target, depth + 1, chain) {
            return Some(n);
        }
    }
    None
}

/// Some bus of the tree moves the addresses under it: a node with children
/// and a non-empty `ranges`. Without one (QEMU `virt`) [`translate`] skips
/// its walk of the tree, which [`mmio_device`]'s ranking would otherwise
/// run for every pair of `virtio,mmio` nodes on every lookup.
fn moves_addresses(fdt: &Fdt<'static>) -> bool {
    static MOVES: Once<bool> = Once::new();
    *MOVES.call_once(|| {
        fdt.all_nodes()
            .any(|n| n.property("ranges").is_some_and(|r| !r.value.is_empty()) && n.children().next().is_some())
    })
}

/// `addr`, a `reg` address of `node`, as the CPU sees it: through the
/// `ranges` of each bus above it, the innermost first. A bus without
/// `ranges`, an empty one, or an address no entry covers is taken as
/// one-to-one (QEMU `virt`'s nodes are all at the root).
fn translate(node: FdtNode<'_, 'static>, mut addr: u64) -> u64 {
    let Some(fdt) = get().filter(|fdt| moves_addresses(fdt)) else {
        return addr;
    };
    let mut chain = [None; MAX_DEPTH];
    let Some(n) = fdt.find_node("/").and_then(|root| ancestors(root, node.name.as_ptr(), 0, &mut chain)) else {
        return addr;
    };
    for k in (1..n).rev() {
        let (Some(bus), Some(up)) = (chain[k], chain[k - 1]) else {
            continue;
        };
        let Some(ranges) = prop(bus, "ranges") else {
            continue;
        };
        let child = bus.cell_sizes().address_cells;
        let size = bus.cell_sizes().size_cells;
        let parent = up.cell_sizes().address_cells;
        let entry = child + parent + size;
        if entry == 0 || child > 2 || parent > 2 || size > 2 {
            continue;
        }
        let mut i = 0;
        while i + entry <= ranges.len() {
            let (Some(c), Some(p), Some(len)) =
                (ranges.join(i, child), ranges.join(i + child, parent), ranges.join(i + child + parent, size))
            else {
                break;
            };
            if addr >= c && addr - c < len {
                addr = addr - c + p;
                break;
            }
            i += entry;
        }
    }
    addr
}

/// `reg` entry `index` of `node`: `(base, size)`, the base as the CPU sees
/// it.
fn node_reg(node: FdtNode<'_, 'static>, index: usize) -> Option<(u64, u64)> {
    let r = node.reg()?.nth(index)?;
    Some((translate(node, r.starting_address as u64), r.size.unwrap_or(0) as u64))
}

/// `#address-cells` of the parent of `node`, as its `reg` is encoded.
fn parent_address_cells(node: FdtNode) -> Option<usize> {
    Some(node.raw_reg()?.next()?.address.len() / 4)
}

/// `reg` entry `index` of the first enabled node compatible with any of
/// `compat`.
pub fn reg(compat: &[&str], index: usize) -> Option<(u64, u64)> {
    node_reg(find_enabled(get()?, compat)?, index)
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
/// (device names such as `/dev/vda/` follow it, whatever the tree's order).
pub fn mmio_device(compat: &str, index: usize) -> Option<MmioDevice> {
    let fdt = get()?;
    let matching = || {
        fdt.all_nodes()
            .filter(|n| enabled(*n) && has_compatible(*n, compat))
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

/// PCIe host bridges: the generic ECAM one (QEMU `virt`), and the BCM2711's
/// (Raspberry Pi 4), whose configuration space is reached one function at
/// a time through a window of its registers (`arch::pci`).
const PCI_HOST_COMPAT: &[&str] = &["pci-host-ecam-generic", "brcm,bcm2711-pcie"];

fn pci_host_node<'b>(fdt: &'b Fdt<'static>) -> Option<FdtNode<'b, 'static>> {
    find_enabled(fdt, PCI_HOST_COMPAT)
}

/// How a host bridge's configuration space is reached.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PciHostKind {
    /// `reg` is the ECAM window.
    Ecam,
    /// `reg` is the BCM2711's registers.
    Bcm2711,
}

/// The PCIe host bridge.
pub struct PciHost {
    pub kind: PciHostKind,
    /// `reg` 0: the ECAM window, or the bridge's registers.
    pub base: u64,
    pub size: u64,
    /// Last bus of `bus-range` (255 when absent).
    pub bus_end: u8,
}

pub fn pci_host() -> Option<PciHost> {
    let host = pci_host_node(get()?)?;
    let kind = if has_compatible(host, "brcm,bcm2711-pcie") { PciHostKind::Bcm2711 } else { PciHostKind::Ecam };
    let (base, size) = node_reg(host, 0)?;
    let bus_end = match prop(host, "bus-range") {
        Some(r) => r.get(1)? as u8,
        None => 255,
    };
    Some(PciHost {
        kind,
        base,
        size,
        bus_end,
    })
}

/// PCI address-space flags of a `ranges` entry (its first child cell).
pub const PCI_SPACE_MASK: u32 = 0x0300_0000;
pub const PCI_SPACE_MEM32: u32 = 0x0200_0000;
pub const PCI_SPACE_MEM64: u32 = 0x0300_0000;

/// A window of a PCI bus: CPU addresses `[cpu, cpu + size)` are bus
/// addresses `[pci, pci + size)` (equal on QEMU `virt`; a Raspberry Pi 4's
/// 32-bit window is CPU 0x6_0000_0000, bus 0xc0000000).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PciWindow {
    pub cpu: u64,
    pub pci: u64,
    pub size: u64,
}

/// The first MMIO window of the host bridge whose space flags equal
/// `space` (`PCI_SPACE_MEM32` / `PCI_SPACE_MEM64`).
pub fn pci_mmio_window(space: u32) -> Option<PciWindow> {
    let host = pci_host_node(get()?)?;
    let ranges = prop(host, "ranges")?;
    let child = host.cell_sizes().address_cells; // 3: flags, address hi, lo
    let size = host.cell_sizes().size_cells;
    let parent = parent_address_cells(host)?;
    let entry = child + parent + size;
    if child != 3 || entry == 0 {
        return None;
    }
    let mut i = 0;
    while i + entry <= ranges.len() {
        if ranges.get(i)? & PCI_SPACE_MASK == space {
            return Some(PciWindow {
                cpu: translate(host, ranges.join(i + child, parent)?),
                pci: ranges.join(i + 1, 2)?,
                size: ranges.join(i + child + parent, size)?,
            });
        }
        i += entry;
    }
    None
}

/// How many windows of a device's DMA are kept (`dma-ranges` entries).
pub const DMA_WINDOWS: usize = 8;

/// A window of a device's DMA: CPU addresses `[cpu, cpu + size)`, which
/// the device addresses from `bus` up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DmaWindow {
    pub bus: u64,
    pub cpu: u64,
    pub size: u64,
}

/// All of memory, one-to-one: the DMA of a device no `dma-ranges` limit.
const DMA_ANYWHERE: DmaWindow = DmaWindow { bus: 0, cpu: 0, size: u64::MAX };

/// What a device's DMA reaches (each `dma-ranges` entry a window, in the
/// tree's order), and whether the tree says its DMA is coherent with the
/// caches (`dma-coherent`, `dma-noncoherent` on the node or a bus above
/// it), `None` when it says nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DmaWindows {
    pub windows: [Option<DmaWindow>; DMA_WINDOWS],
    pub coherent: Option<bool>,
}

/// DMA of the functions behind the PCIe host bridge: its `dma-ranges`
/// (bus addresses of the functions' DMA), then the buses' above it.
pub fn pci_dma() -> Option<DmaWindows> {
    let host = pci_host_node(get()?)?;
    let mut windows = [None; DMA_WINDOWS];
    match prop(host, "dma-ranges").filter(|r| r.len() > 0) {
        None => windows[0] = Some(DMA_ANYWHERE),
        Some(r) => {
            let child = host.cell_sizes().address_cells;
            let size = host.cell_sizes().size_cells;
            let parent = parent_address_cells(host)?;
            let entry = child + parent + size;
            if child != 3 {
                return None;
            }
            for (k, w) in windows.iter_mut().enumerate() {
                let i = k * entry;
                if i + entry > r.len() {
                    break;
                }
                *w = Some(DmaWindow {
                    bus: r.join(i + 1, 2)?,
                    cpu: r.join(i + child, parent)?,
                    size: r.join(i + child + parent, size)?,
                });
            }
        }
    }
    Some(dma_up(host, windows))
}

/// DMA of the memory-mapped device whose registers are at `phys`: the
/// `dma-ranges` of the buses above it.
pub fn mmio_dma(phys: u64) -> Option<DmaWindows> {
    let node = get()?
        .all_nodes()
        .find(|n| enabled(*n) && n.reg().is_some() && node_reg(*n, 0).is_some_and(|(b, _)| b == phys))?;
    let mut windows = [None; DMA_WINDOWS];
    windows[0] = Some(DMA_ANYWHERE);
    Some(dma_up(node, windows))
}

/// `windows`, the DMA of `node` in its parent's addresses, through the
/// `dma-ranges` of each bus above (innermost first; none, or an empty one,
/// is one-to-one): each window cut to the entries it overlaps, and moved
/// by them. With the coherence the nearest of them states.
fn dma_up(node: FdtNode<'_, 'static>, mut windows: [Option<DmaWindow>; DMA_WINDOWS]) -> DmaWindows {
    let said = |n: FdtNode| {
        if n.property("dma-coherent").is_some() {
            Some(true)
        } else if n.property("dma-noncoherent").is_some() {
            Some(false)
        } else {
            None
        }
    };
    let mut coherent = said(node);
    let chain = get().and_then(|fdt| {
        let mut chain = [None; MAX_DEPTH];
        let n = fdt.find_node("/").and_then(|root| ancestors(root, node.name.as_ptr(), 0, &mut chain))?;
        Some((chain, n))
    });
    let Some((chain, n)) = chain else {
        return DmaWindows { windows, coherent };
    };
    for k in (0..n).rev() {
        let Some(bus) = chain[k] else {
            continue;
        };
        coherent = coherent.or_else(|| said(bus));
        let (Some(up), Some(r)) = (k.checked_sub(1).and_then(|u| chain[u]), prop(bus, "dma-ranges")) else {
            continue;
        };
        let child = bus.cell_sizes().address_cells;
        let size = bus.cell_sizes().size_cells;
        let parent = up.cell_sizes().address_cells;
        let entry = child + parent + size;
        if r.len() == 0 || child > 2 || parent > 2 || size > 2 || entry == 0 {
            continue;
        }
        let mut moved = [None; DMA_WINDOWS];
        let mut m = 0;
        for w in windows.iter().flatten() {
            let mut i = 0;
            while i + entry <= r.len() && m < DMA_WINDOWS {
                let (Some(c), Some(p), Some(len)) =
                    (r.join(i, child), r.join(i + child, parent), r.join(i + child + parent, size))
                else {
                    break;
                };
                let start = w.cpu.max(c);
                let end = w.cpu.saturating_add(w.size).min(c.saturating_add(len));
                if start < end {
                    moved[m] = Some(DmaWindow { bus: w.bus + (start - w.cpu), cpu: p + (start - c), size: end - start });
                    m += 1;
                }
                i += entry;
            }
        }
        windows = moved;
    }
    DmaWindows { windows, coherent }
}

/// Interrupt specifier for INTx `pin` (1..=4) of the PCI function
/// `bus:slot.func`, from the host bridge's `interrupt-map` and
/// `interrupt-map-mask` (the slot swizzle is whatever the board says).
pub fn pci_intx(bus: u8, slot: u8, func: u8, pin: u8) -> Option<IrqSpec> {
    let fdt = get()?;
    let host = pci_host_node(fdt)?;
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

/// The tree describes a Raspberry Pi 3's interrupt controllers (BCM2836
/// local, BCM2835 peripherals): its two-cell interrupt specifiers are
/// theirs.
pub fn has_bcm2836_intc() -> bool {
    static HAS: Once<bool> = Once::new();
    *HAS.call_once(|| get().is_some_and(|fdt| find_enabled(fdt, &["brcm,bcm2836-armctrl-ic"]).is_some()))
}

/// `timebase-frequency` of the CPUs (riscv64: the `time` CSR rate).
pub fn timebase_frequency() -> Option<u64> {
    let cpus = get()?.find_node("/cpus")?;
    cpus.property("timebase-frequency")?.as_usize().map(|v| v as u64)
}

/// GICv2 `compatible` strings (QEMU `virt`: `arm,cortex-a15-gic`).
const GICV2_COMPAT: &[&str] = &["arm,cortex-a15-gic", "arm,gic-400", "arm,cortex-a9-gic"];

/// UARTs the console can drive: PL011s, and 16550s (most SoCs; the
/// Raspberry Pi's mini UART, a 16550 at a 4-byte stride).
const PL011_COMPAT: &[&str] = &["arm,pl011"];
const NS16550_COMPAT: &[&str] = &["ns16550a", "ns16550", "snps,dw-apb-uart", "brcm,bcm2835-aux-uart"];

/// `node` as a UART the console can drive: base, first interrupt, and the
/// `reg-shift` / `reg-io-width` of a 16550 (the Pi's mini UART has none:
/// a 4-byte stride and 32-bit access, as its Linux driver programs it).
fn uart_of(fdt: &Fdt<'static>, node: FdtNode<'_, 'static>) -> Option<Uart> {
    let kind = if PL011_COMPAT.iter().any(|c| has_compatible(node, c)) {
        UartKind::Pl011
    } else if NS16550_COMPAT.iter().any(|c| has_compatible(node, c)) {
        UartKind::Ns16550
    } else {
        return None;
    };
    let (base, _) = node_reg(node, 0)?;
    let irq = interrupt_spec(fdt, node).and_then(|s| crate::arch::irq_from_dt(s.cells()));
    let mini = has_compatible(node, "brcm,bcm2835-aux-uart");
    let cell = |name: &str, default: u32| match kind {
        UartKind::Ns16550 => prop(node, name).and_then(|c| c.get(0)).unwrap_or(default),
        UartKind::Pl011 => default,
    };
    Some(Uart {
        kind,
        base,
        io: false,
        irq,
        reg_shift: cell("reg-shift", if mini { 2 } else { 0 }) as u8,
        reg_width: cell("reg-io-width", if mini { 4 } else { 1 }) as u8,
    })
}

/// The console UART: the one `/chosen`'s `stdout-path` names (a path or
/// an alias, before its `:options`), if it is one the console can drive;
/// else the first enabled PL011, else the first enabled 16550.
fn console_uart(fdt: &Fdt<'static>) -> Option<Uart> {
    let named = fdt
        .find_node("/chosen")
        .and_then(|c| c.property("stdout-path"))
        .and_then(|p| p.as_str())
        .and_then(|path| fdt.find_node(path.split(':').next()?))
        .filter(|n| enabled(*n))
        .and_then(|n| uart_of(fdt, n));
    named
        .or_else(|| uart_of(fdt, find_enabled(fdt, PL011_COMPAT)?))
        .or_else(|| uart_of(fdt, find_enabled(fdt, NS16550_COMPAT)?))
}

/// Describe the board from the tree (`crate::platform::init`): the model,
/// the CPUs, the interrupt controller (a GICv2, a GICv3 or a PLIC), the
/// console UART (`console_uart`: a PL011 or a 16550), the RTC (a PL031 or a goldfish), the
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
    } else if let (Some((local, _)), Some((armctrl, _))) =
        (reg(&["brcm,bcm2836-l1-intc"], 0), reg(&["brcm,bcm2836-armctrl-ic"], 0))
    {
        offer_intc(p, Intc::Bcm2836 { local, armctrl }, from);
    }
    if let Some(uart) = console_uart(fdt) {
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
        let bus_start = pci_host_node(fdt)
            .and_then(|h| prop(h, "bus-range"))
            .and_then(|r| r.get(0))
            .unwrap_or(0) as u8;
        offer_pci(
            p,
            crate::platform::PciHost {
                kind: host.kind,
                base: host.base,
                size: host.size,
                bus_start,
                bus_end: host.bus_end,
            },
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
    let psci = find_enabled(fdt, &["arm,psci-1.0", "arm,psci-0.2", "arm,psci"]);
    match psci.and_then(|n| n.property("method")).and_then(|m| m.as_str()) {
        Some("hvc") => offer_psci(p, PsciConduit::Hvc, from),
        Some("smc") => offer_psci(p, PsciConduit::Smc, from),
        _ => {}
    }
}
