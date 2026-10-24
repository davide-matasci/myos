//! The RISC-V AIA (QEMU `virt,aia=aplic-imsic`): an IMSIC per hart takes
//! message-signalled interrupts, each an identity (1 to `riscv,num-ids`)
//! of its supervisor-level interrupt file, and the APLIC turns the wired
//! interrupts (the UART, the virtio-mmio devices, the PCIe INTx lines)
//! into such messages. The supervisor level's: the machine level's nodes
//! are the firmware's (`dt::machine_level`).
//!
//! Every interrupt goes to the BSP's file. A wired source `n` is its
//! identity `n` (so the device tree's numbers stay the `irq::dispatch`
//! numbers, as the PLIC's); the identities above the APLIC's sources are
//! the PCI functions' MSIs ([`MsiChip`]: the BSP's page and the identity
//! are the message). The file's enable bits are the BSP's own CSRs, so
//! they are all on from its start: an interrupt is gated where it is
//! raised (the APLIC's enable bit, the device's MSI). The IPIs stay the
//! SBI's, as with the PLIC.

use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use spin::Mutex;

use crate::irqchip::{DtNode, Driver, IrqChip, Ipi, MsiChip, Target};
use crate::msi::Message;
use crate::platform::Intc;

pub static DRIVER: Driver = Driver {
    name: "aia",
    dt: &[DtNode { compatible: &["riscv,aplic"], regs: 1 }, DtNode { compatible: &["riscv,imsics"], regs: 1 }],
    xlate,
    start,
};

/// The most sources an APLIC has (1 to 1023).
const MAX_SOURCES: usize = 1024;
/// Each source's trigger from its specifier (`IRQ_TYPE_*`: 1 rising edge,
/// 2 falling, 4 high level, 8 low), for [`IrqChip::unmask`].
static TRIGGER: [AtomicU8; MAX_SOURCES] = [const { AtomicU8::new(0) }; MAX_SOURCES];

/// An APLIC interrupt specifier: the source and its trigger.
fn xlate(node: usize, cells: &[u32]) -> Option<u32> {
    match (node, cells) {
        (0, [src, flags, ..]) if *src != 0 && (*src as usize) < MAX_SOURCES => {
            TRIGGER[*src as usize].store(*flags as u8, Ordering::Relaxed);
            Some(*src)
        }
        (0, [src]) if *src != 0 && (*src as usize) < MAX_SOURCES => Some(*src),
        _ => None,
    }
}

static APLIC: AtomicUsize = AtomicUsize::new(0);
/// The APLIC's sources and the IMSIC's identities.
static SOURCES: AtomicU32 = AtomicU32::new(0);
static IDS: AtomicU32 = AtomicU32::new(0);
/// The BSP's IMSIC page (the MSIs' address) and its hart index (what the
/// APLIC's targets name).
static BSP_PAGE: AtomicU64 = AtomicU64::new(0);
static BSP_INDEX: AtomicU32 = AtomicU32::new(0);

// APLIC registers (the supervisor domain's, in MSI delivery mode).
const DOMAINCFG: usize = 0x0000;
const SOURCECFG: usize = 0x0004;
const SETIPNUM: usize = 0x1CDC;
const IN_CLRIP: usize = 0x1D00;
const SETIENUM: usize = 0x1EDC;
const TARGET: usize = 0x3004;
/// `domaincfg`: interrupts enabled (IE), MSI delivery mode (DM).
const DOMAINCFG_IE: u32 = 1 << 8;
const DOMAINCFG_DM: u32 = 1 << 2;
/// `sourcecfg` source modes.
const SM_EDGE_RISE: u32 = 4;
const SM_EDGE_FALL: u32 = 5;
const SM_LEVEL_HIGH: u32 = 6;
const SM_LEVEL_LOW: u32 = 7;

// The IMSIC's supervisor interrupt file's registers, through `siselect`
// and `sireg`.
const EIDELIVERY: u64 = 0x70;
const EITHRESHOLD: u64 = 0x72;
const EIE0: u64 = 0xC0;

fn aplic() -> usize {
    APLIC.load(Ordering::Relaxed)
}

fn w(off: usize, v: u32) {
    unsafe { core::ptr::write_volatile((aplic() + off) as *mut u32, v) }
}

fn r(off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((aplic() + off) as *const u32) }
}

/// Write `value` to the interrupt file's register `reg` (`siselect`, then
/// `sireg`: CSRs 0x150 and 0x151).
fn ifile_write(reg: u64, value: u64) {
    unsafe {
        core::arch::asm!("csrw 0x150, {s}", "csrw 0x151, {v}", s = in(reg) reg, v = in(reg) value, options(nostack));
    }
}

/// The APLIC's registers, where Limine's map has the low device range
/// (`paging::map_devices`), as the PLIC's; the BSP's place among the
/// IMSICs from the tree.
fn start(intc: &Intc) -> Result<&'static dyn IrqChip, &'static str> {
    let aplic = intc.reg(0).ok_or("platform: AIA without its APLIC")?;
    let imsic = intc.reg(1).ok_or("platform: AIA without its IMSICs")?;
    APLIC.store(aplic.base as usize, Ordering::SeqCst);
    let sources = crate::dt::aia_count("riscv,aplic", "riscv,num-sources").unwrap_or(0).min(MAX_SOURCES as u32 - 1);
    let ids = crate::dt::aia_count("riscv,imsics", "riscv,num-ids").unwrap_or(0).min(2047);
    if sources == 0 || ids <= sources {
        return Err("platform: AIA without sources or identities for them");
    }
    SOURCES.store(sources, Ordering::SeqCst);
    IDS.store(ids, Ordering::SeqCst);
    let page = crate::dt::imsic_hart_page(crate::smp::cpu_hw_id(0)).ok_or("platform: no IMSIC page for the BSP")?;
    BSP_PAGE.store(page, Ordering::SeqCst);
    BSP_INDEX.store(((page - imsic.base) >> 12) as u32, Ordering::SeqCst);
    // Every source inactive and disabled until unmasked, then messages on.
    w(DOMAINCFG, 0);
    for src in 1..=sources as usize {
        w(SOURCECFG + 4 * (src - 1), 0);
    }
    w(DOMAINCFG, DOMAINCFG_IE | DOMAINCFG_DM);
    crate::irqchip::msi_register(&AIA);
    Ok(&AIA)
}

/// The boot log's line for the MSIs.
pub fn report() {
    let (first, ids) = (SOURCES.load(Ordering::Relaxed) + 1, IDS.load(Ordering::Relaxed));
    crate::console::status_ok(&alloc::format!("msi: imsic, identities {first}-{ids}"));
}

struct Aia {
    /// The MSI identities given out, a bit each above the sources'.
    msis: Mutex<[u64; 32]>,
}

static AIA: Aia = Aia { msis: Mutex::new([0; 32]) };

impl IrqChip for Aia {
    fn name(&self) -> &'static str {
        "aia"
    }

    /// The BSP's interrupt file: delivery on, every identity enabled
    /// (each gated where it is raised), no threshold; supervisor external
    /// interrupts on. The other harts' files stay off: nothing is sent
    /// to them.
    fn init_cpu(&self) {
        if crate::smp::cpu_id() != 0 {
            return;
        }
        ifile_write(EITHRESHOLD, 0);
        let ids = IDS.load(Ordering::Relaxed) as u64;
        for word in 0..=ids / 64 {
            // RV64: the even registers, 64 identities each.
            ifile_write(EIE0 + 2 * word, u64::MAX);
        }
        ifile_write(EIDELIVERY, 1);
        unsafe {
            core::arch::asm!("csrs sie, {}", in(reg) 1u64 << 9, options(nostack)); // SEIE
        }
    }

    fn route(&self, irq: u32) -> Option<u32> {
        (1..=SOURCES.load(Ordering::Relaxed)).contains(&irq).then_some(irq)
    }

    /// Source `src` as its specifier's trigger (high level without one),
    /// sent to the BSP as identity `src`, enabled.
    fn unmask(&self, src: u32) {
        if !(1..=SOURCES.load(Ordering::Relaxed)).contains(&src) {
            return;
        }
        let mode = match TRIGGER[src as usize].load(Ordering::Relaxed) {
            1 => SM_EDGE_RISE,
            2 => SM_EDGE_FALL,
            8 => SM_LEVEL_LOW,
            _ => SM_LEVEL_HIGH,
        };
        let i = src as usize - 1;
        w(SOURCECFG + 4 * i, mode);
        w(TARGET + 4 * i, BSP_INDEX.load(Ordering::Relaxed) << 18 | src);
        w(SETIENUM, src);
    }

    /// The BSP's highest pending identity, claimed (`stopei`, CSR 0x15C,
    /// read and cleared).
    fn claim(&self) -> Option<u32> {
        let top: u64;
        unsafe {
            core::arch::asm!("csrrw {t}, 0x15C, zero", t = out(reg) top, options(nostack));
        }
        let id = (top >> 16) as u32 & 0x7FF;
        (id != 0).then_some(id)
    }

    /// A level-triggered source still asserted after its handlers ran
    /// (they quieten their device, but a shared line may have another)
    /// is sent again: in MSI mode the APLIC sends one message per
    /// assertion.
    fn complete(&self, id: u32) {
        if !(1..=SOURCES.load(Ordering::Relaxed)).contains(&id) {
            return;
        }
        let level = !matches!(TRIGGER[id as usize].load(Ordering::Relaxed), 1 | 2);
        if level && r(IN_CLRIP + 4 * (id as usize / 32)) & 1 << (id % 32) != 0 {
            w(SETIPNUM, id);
        }
    }

    fn send_ipi(&self, to: Target, _ipi: Ipi) {
        super::plic::send_ipi(to);
    }
}

impl MsiChip for Aia {
    /// A free identity above the sources', its message the BSP's page.
    fn alloc(&self, _rid: u16) -> Option<(u32, Message)> {
        let (first, ids) = (SOURCES.load(Ordering::Relaxed) + 1, IDS.load(Ordering::Relaxed));
        let mut used = self.msis.lock();
        let id = (first..=ids).find(|&n| used[n as usize / 64] & 1 << (n % 64) == 0)?;
        used[id as usize / 64] |= 1 << (id % 64);
        Some((id, Message { address: BSP_PAGE.load(Ordering::Relaxed), data: id }))
    }

    fn free(&self, irq: u32) {
        if (SOURCES.load(Ordering::Relaxed) + 1..=IDS.load(Ordering::Relaxed)).contains(&irq) {
            self.msis.lock()[irq as usize / 64] &= !(1 << (irq % 64));
        }
    }
}
