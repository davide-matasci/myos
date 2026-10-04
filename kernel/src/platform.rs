//! The board the kernel runs on, described once at boot, before the heap
//! exists: the interrupt controller, the console UART, the RTC, the clocks
//! and the PCIe host bridge, plus the CPU count. The arch code takes its
//! device bases from here and from nowhere else (`arch::apply_platform`),
//! so nothing about a board is assumed outside the two fillers.
//!
//! The ACPI static tables fill it when the firmware has them and the arch
//! prefers them (x86_64 always, aarch64 under EDK2: `crate::acpi`); the
//! device tree fills it otherwise (riscv64, boards without ACPI:
//! `crate::dt::fill`). When both exist the tree adds what the tables do not
//! describe (the RTC, the PCIe MMIO windows) and the components both
//! describe are compared, so a disagreement is visible instead of silent.
//! Every component says which source it came from; `/proc/platform` shows
//! the whole description, and the Limine command line can force a source
//! (`platform=acpi`, `platform=dt`) when a board's firmware gets one wrong.

use spin::Once;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Acpi,
    DeviceTree,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Acpi => "acpi",
            Source::DeviceTree => "dt",
        }
    }
}

/// A component and where its description came from.
#[derive(Clone, Copy)]
pub struct Component<T: Copy> {
    pub value: T,
    pub from: Source,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Intc {
    /// x86_64: the local APIC's physical address.
    Apic { lapic: u64 },
    /// aarch64: the GICv2 distributor and CPU interface.
    GicV2 { gicd: u64, gicc: u64 },
    /// aarch64: the GICv3 distributor and the redistributor range.
    GicV3 { gicd: u64, gicr: u64, gicr_size: u64 },
    /// riscv64: the PLIC.
    Plic { base: u64 },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UartKind {
    Pl011,
    Ns16550,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Uart {
    pub kind: UartKind,
    pub base: u64,
    /// `base` is an I/O port, not a memory address (x86_64's COM ports).
    pub io: bool,
    /// Its interrupt, as the interrupt controller numbers it.
    pub irq: Option<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RtcKind {
    Pl031,
    Goldfish,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Rtc {
    pub kind: RtcKind,
    pub base: u64,
}

/// The PCIe host bridge's configuration space.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PciHost {
    pub ecam: u64,
    pub ecam_size: u64,
    pub bus_start: u8,
    pub bus_end: u8,
}

/// Where the host bridge maps BARs: the 32-bit and the 64-bit MMIO window
/// (`base, size`), from the tree's `ranges`; ACPI keeps them in AML.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PciWindows {
    pub mmio32: Option<(u64, u64)>,
    pub mmio64: Option<(u64, u64)>,
}

/// A component both sources described differently.
pub const DIFFERS_CPUS: u8 = 1;
pub const DIFFERS_INTC: u8 = 2;
pub const DIFFERS_UART: u8 = 4;
pub const DIFFERS_PCI: u8 = 8;

#[derive(Clone, Copy)]
pub struct Platform {
    /// The source that filled first (the preferred one that was present).
    pub source: Option<Source>,
    /// The tree's root `model` (`linux,dummy-virt`, `riscv-virtio,qemu`).
    pub model: Option<Component<&'static str>>,
    /// The ACPI root table's OEM ID.
    pub oem: Option<([u8; 6], usize)>,
    pub cpus: Option<Component<usize>>,
    pub intc: Option<Component<Intc>>,
    pub uart: Option<Component<Uart>>,
    pub rtc: Option<Component<Rtc>>,
    /// riscv64: the `time` CSR rate.
    pub timer_hz: Option<Component<u64>>,
    /// aarch64: the EL1 physical and virtual timer INTIDs.
    pub timer_irqs: Option<Component<(u32, u32)>>,
    pub pci: Option<Component<PciHost>>,
    pub pci_windows: Option<Component<PciWindows>>,
    /// `DIFFERS_*` bits: the second source disagreed with the first there.
    pub differs: u8,
}

impl Platform {
    const EMPTY: Platform = Platform {
        source: None,
        model: None,
        oem: None,
        cpus: None,
        intc: None,
        uart: None,
        rtc: None,
        timer_hz: None,
        timer_irqs: None,
        pci: None,
        pci_windows: None,
        differs: 0,
    };
}

static PLATFORM: Once<Platform> = Once::new();

/// Offer `value` from `from` for a component: taken when nothing described
/// it yet, compared otherwise (`bit` into `differs` when it disagrees).
fn offer<T: Copy + PartialEq>(
    slot: &mut Option<Component<T>>,
    value: T,
    from: Source,
    differs: &mut u8,
    bit: u8,
) {
    match slot {
        None => *slot = Some(Component { value, from }),
        Some(c) if c.value != value => *differs |= bit,
        Some(_) => {}
    }
}

/// What the Limine command line asks for: `platform=acpi` or `platform=dt`
/// keeps the other source out of the description.
fn forced() -> Option<Source> {
    for word in crate::limine_boot::cmdline().split_whitespace() {
        match word {
            "platform=acpi" => return Some(Source::Acpi),
            "platform=dt" => return Some(Source::DeviceTree),
            _ => {}
        }
    }
    None
}

/// Describe the board: ACPI first where the arch prefers it and the
/// firmware has tables, then the device tree. Before the heap.
pub fn init() {
    let forced = forced();
    let mut p = Platform::EMPTY;
    let acpi_wanted = forced != Some(Source::DeviceTree)
        && (crate::arch::PREFER_ACPI || forced == Some(Source::Acpi));
    if acpi_wanted && crate::acpi::present() {
        fill_acpi(&mut p);
        p.source = Some(Source::Acpi);
    }
    if forced != Some(Source::Acpi) && crate::dt::get().is_some() {
        crate::dt::fill(&mut p);
        if p.source.is_none() {
            p.source = Some(Source::DeviceTree);
        }
    }
    PLATFORM.call_once(|| p);
}

pub fn get() -> &'static Platform {
    PLATFORM.get().unwrap_or(&Platform::EMPTY)
}

/// The ACPI static tables' view (`crate::acpi`).
fn fill_acpi(p: &mut Platform) {
    use crate::acpi;
    let from = Source::Acpi;
    p.oem = acpi::oem_id();
    if let Some(m) = acpi::madt() {
        if m.cpus != 0 {
            offer(&mut p.cpus, m.cpus, from, &mut p.differs, DIFFERS_CPUS);
        }
        let intc = match (m.gicd, m.gicc, m.gicr) {
            // A GICD entry names the version; an unknown one with
            // redistributors is a v3.
            (Some((gicd, v)), _, Some((gicr, gicr_size))) if v >= 3 || v == 0 => {
                Some(Intc::GicV3 { gicd, gicr, gicr_size })
            }
            (Some((gicd, _)), Some(gicc), _) => Some(Intc::GicV2 { gicd, gicc }),
            (Some((gicd, v)), None, None) if v >= 3 => {
                Some(Intc::GicV3 { gicd, gicr: 0, gicr_size: 0 })
            }
            (None, _, _) if m.lapic != 0 => Some(Intc::Apic { lapic: m.lapic }),
            _ => None,
        };
        if let Some(intc) = intc {
            offer(&mut p.intc, intc, from, &mut p.differs, DIFFERS_INTC);
        }
    }
    if let Some(s) = acpi::spcr() {
        let kind = match s.interface {
            0 | 1 | 0x12 => Some(UartKind::Ns16550),
            3 | 0xD | 0xE => Some(UartKind::Pl011),
            _ => None,
        };
        if let Some(kind) = kind {
            let uart = Uart {
                kind,
                base: s.base,
                io: s.io,
                irq: (s.gsiv != 0).then_some(s.gsiv),
            };
            offer(&mut p.uart, uart, from, &mut p.differs, DIFFERS_UART);
        }
    }
    if let Some(m) = acpi::mcfg() {
        let host = PciHost {
            ecam: m.base,
            ecam_size: (u64::from(m.bus_end) - u64::from(m.bus_start) + 1) << 20,
            bus_start: m.bus_start,
            bus_end: m.bus_end,
        };
        offer(&mut p.pci, host, from, &mut p.differs, DIFFERS_PCI);
    }
    if let Some(g) = acpi::gtdt() {
        p.timer_irqs = Some(Component { value: (g.el1_phys, g.el1_virt), from });
    }
}

/// The device tree's view: what it describes that nothing filled yet, and
/// a comparison for what the tables already did (`crate::dt::fill`).
pub fn offer_model(p: &mut Platform, model: &'static str) {
    if p.model.is_none() {
        p.model = Some(Component { value: model, from: Source::DeviceTree });
    }
}

pub fn offer_cpus(p: &mut Platform, cpus: usize, from: Source) {
    offer(&mut p.cpus, cpus, from, &mut p.differs, DIFFERS_CPUS);
}

pub fn offer_intc(p: &mut Platform, intc: Intc, from: Source) {
    offer(&mut p.intc, intc, from, &mut p.differs, DIFFERS_INTC);
}

pub fn offer_uart(p: &mut Platform, uart: Uart, from: Source) {
    offer(&mut p.uart, uart, from, &mut p.differs, DIFFERS_UART);
}

pub fn offer_rtc(p: &mut Platform, rtc: Rtc, from: Source) {
    if p.rtc.is_none() {
        p.rtc = Some(Component { value: rtc, from });
    }
}

pub fn offer_timer_hz(p: &mut Platform, hz: u64, from: Source) {
    if p.timer_hz.is_none() {
        p.timer_hz = Some(Component { value: hz, from });
    }
}

pub fn offer_pci(p: &mut Platform, host: PciHost, from: Source) {
    offer(&mut p.pci, host, from, &mut p.differs, DIFFERS_PCI);
}

pub fn offer_pci_windows(p: &mut Platform, windows: PciWindows, from: Source) {
    if p.pci_windows.is_none() {
        p.pci_windows = Some(Component { value: windows, from });
    }
}

/// `source acpi+dt`, `source dt`, ...: the sources that filled, for the
/// boot log and `/proc/platform`.
pub fn sources_text(p: &Platform) -> &'static str {
    let dt = crate::dt::get().is_some() && forced() != Some(Source::Acpi);
    match (p.source, dt) {
        (Some(Source::Acpi), true) => "acpi+dt",
        (Some(Source::Acpi), false) => "acpi",
        (Some(Source::DeviceTree), _) => "dt",
        (None, _) => "none",
    }
}

/// The names of the components the two sources disagree on.
pub fn differs_text(p: &Platform) -> alloc::string::String {
    let mut s = alloc::string::String::new();
    for (bit, name) in [
        (DIFFERS_CPUS, "cpus"),
        (DIFFERS_INTC, "intc"),
        (DIFFERS_UART, "uart"),
        (DIFFERS_PCI, "pci"),
    ] {
        if p.differs & bit != 0 {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(name);
        }
    }
    s
}

/// The text of `/proc/platform`: one line per component, ending in the
/// source it came from, `(acpi, dt differs)` when the tree disagreed.
pub fn text() -> alloc::vec::Vec<u8> {
    use alloc::format;
    let p = get();
    let tag = |from: Source, bit: u8| {
        if p.differs & bit != 0 {
            format!("({}, {} differs)", from.name(), other(from).name())
        } else {
            format!("({})", from.name())
        }
    };
    let mut s = format!("source {}\n", sources_text(p));
    if let Some(m) = p.model {
        s += &format!("model {} ({})\n", m.value, m.from.name());
    }
    if let Some((oem, n)) = p.oem {
        if let Ok(oem) = core::str::from_utf8(&oem[..n]) {
            s += &format!("oem {oem} (acpi)\n");
        }
    }
    if let Some(c) = p.cpus {
        s += &format!("cpus {} {}\n", c.value, tag(c.from, DIFFERS_CPUS));
    }
    if let Some(c) = p.intc {
        let what = match c.value {
            Intc::Apic { lapic } => format!("apic 0x{lapic:x}"),
            Intc::GicV2 { gicd, gicc } => format!("gicv2 0x{gicd:x} 0x{gicc:x}"),
            Intc::GicV3 { gicd, gicr, gicr_size } => {
                format!("gicv3 0x{gicd:x} 0x{gicr:x} 0x{gicr_size:x}")
            }
            Intc::Plic { base } => format!("plic 0x{base:x}"),
        };
        s += &format!("intc {what} {}\n", tag(c.from, DIFFERS_INTC));
    }
    if let Some(c) = p.uart {
        let u = c.value;
        let kind = match u.kind {
            UartKind::Pl011 => "pl011",
            UartKind::Ns16550 => "ns16550",
        };
        s += &format!("uart {kind} {}0x{:x}", if u.io { "io " } else { "" }, u.base);
        if let Some(irq) = u.irq {
            s += &format!(" irq {irq}");
        }
        s += &format!(" {}\n", tag(c.from, DIFFERS_UART));
    }
    if let Some(c) = p.rtc {
        let kind = match c.value.kind {
            RtcKind::Pl031 => "pl031",
            RtcKind::Goldfish => "goldfish",
        };
        s += &format!("rtc {kind} 0x{:x} ({})\n", c.value.base, c.from.name());
    }
    if let Some(c) = p.timer_hz {
        s += &format!("timer hz {} ({})\n", c.value, c.from.name());
    }
    if let Some(c) = p.timer_irqs {
        s += &format!("timer irq {} {} ({})\n", c.value.0, c.value.1, c.from.name());
    }
    if let Some(c) = p.pci {
        let h = c.value;
        s += &format!(
            "pci ecam 0x{:x} 0x{:x} bus {}-{} {}\n",
            h.ecam, h.ecam_size, h.bus_start, h.bus_end, tag(c.from, DIFFERS_PCI)
        );
    }
    if let Some(c) = p.pci_windows {
        if let Some((base, size)) = c.value.mmio32 {
            s += &format!("pci mmio32 0x{base:x} 0x{size:x} ({})\n", c.from.name());
        }
        if let Some((base, size)) = c.value.mmio64 {
            s += &format!("pci mmio64 0x{base:x} 0x{size:x} ({})\n", c.from.name());
        }
    }
    s.into_bytes()
}

fn other(from: Source) -> Source {
    match from {
        Source::Acpi => Source::DeviceTree,
        Source::DeviceTree => Source::Acpi,
    }
}
