//! The GIC: v2 (memory-mapped distributor and CPU interface) and v3
//! (distributor with affinity routing, one redistributor per CPU found by
//! MPIDR, the CPU interface through the `ICC_*` system registers). Both
//! take the SGIs (IPIs) and the timer PPIs on every CPU and deliver the
//! SPIs they enable to the BSP, level-triggered, priority 0x80.

use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use super::interrupts::{PPI_EL1_PHYS, PPI_EL1_VIRT, SGI_RESCHED, SGI_TLB};
use crate::irqchip::{DtNode, Driver, IrqChip, Ipi, Target};
use crate::platform::Intc;

/// The distributor, and either the GICv2 CPU interface or the GICv3
/// redistributor region (`GICR_SIZE` bytes of frames, one per CPU), mapped
/// (device window).
static GICD_BASE: AtomicUsize = AtomicUsize::new(0);
static GICC_BASE: AtomicUsize = AtomicUsize::new(0);
static GICR_BASE: AtomicUsize = AtomicUsize::new(0);
static GICR_SIZE: AtomicUsize = AtomicUsize::new(0);
static GICD_V3_DONE: AtomicBool = AtomicBool::new(false);
/// The BSP's affinity, where SPIs are routed (GICv3 `GICD_IROUTER`).
static BSP_AFFINITY: AtomicU64 = AtomicU64::new(0);

fn gicd() -> usize {
    GICD_BASE.load(Ordering::Relaxed)
}

fn gicc() -> usize {
    GICC_BASE.load(Ordering::Relaxed)
}

// Distributor registers (GICv3: with affinity routing).
const GICD_CTLR: usize = 0x0000;
const GICD_TYPER: usize = 0x0004;
const GICD_IGROUPR: usize = 0x0080;
const GICD_ISENABLER: usize = 0x0100;
const GICD_ICENABLER: usize = 0x0180;
const GICD_IPRIORITYR: usize = 0x0400;
const GICD_ITARGETSR: usize = 0x0800;
const GICD_ICFGR: usize = 0x0C00;
const GICD_SGIR: usize = 0x0F00;
const GICD_IROUTER: usize = 0x6000;
const GICD_CTLR_RWP: u32 = 1 << 31;
// GICv2 CPU interface registers.
const GICC_CTLR: usize = 0x00;
const GICC_PMR: usize = 0x04;
const GICC_IAR: usize = 0x0C;
const GICC_EOIR: usize = 0x10;
// Redistributor: the RD frame, then the SGI frame 64 KiB further.
const GICR_CTLR: usize = 0x0000;
const GICR_TYPER: usize = 0x0008;
const GICR_WAKER: usize = 0x0014;
const GICR_SGI_FRAME: usize = 0x1_0000;
const GICR_IGROUPR0: usize = 0x0080;
const GICR_ISENABLER0: usize = 0x0100;
const GICR_ICENABLER0: usize = 0x0180;
const GICR_IPRIORITYR: usize = 0x0400;
const GICR_TYPER_VLPIS: u64 = 1 << 1;
const GICR_TYPER_LAST: u64 = 1 << 4;
const GICR_WAKER_PROCESSOR_SLEEP: u32 = 1 << 1;
const GICR_WAKER_CHILDREN_ASLEEP: u32 = 1 << 2;

/// INTIDs from 1020 are special (1023: nothing pending).
const SPECIAL: u32 = 1020;

pub static GICV2: Driver = Driver {
    name: "gicv2",
    dt: &[DtNode { compatible: &["arm,cortex-a15-gic", "arm,gic-400", "arm,cortex-a9-gic"], regs: 2 }],
    xlate,
    start: start_v2,
};

pub static GICV3: Driver = Driver {
    name: "gicv3",
    dt: &[DtNode { compatible: &["arm,gic-v3"], regs: 2 }],
    xlate,
    start: start_v3,
};

/// A GIC interrupt specifier (`type, number, flags`): SPI `n` is INTID
/// 32 + n, PPI `n` is 16 + n.
fn xlate(_node: usize, cells: &[u32]) -> Option<u32> {
    match cells {
        [0, n, ..] if *n < 988 => Some(32 + n),
        [1, n, ..] if *n < 16 => Some(16 + n),
        _ => None,
    }
}

fn map(phys: u64, size: u64, what: &'static str) -> Result<usize, &'static str> {
    super::paging::map_mmio(phys, size).ok_or(what)
}

fn start_v2(intc: &Intc) -> Result<&'static dyn IrqChip, &'static str> {
    let (Some(d), Some(c)) = (intc.reg(0), intc.reg(1)) else {
        return Err("platform: GICv2 without its distributor and CPU interface");
    };
    GICD_BASE.store(map(d.base, 0x1000, "platform: cannot map the GIC distributor")?, Ordering::SeqCst);
    GICC_BASE.store(map(c.base, 0x2000, "platform: cannot map the GIC CPU interface")?, Ordering::SeqCst);
    Ok(&GicV2)
}

fn start_v3(intc: &Intc) -> Result<&'static dyn IrqChip, &'static str> {
    let Some(r) = intc.reg(1).filter(|r| r.base != 0) else {
        return Err("platform: GICv3 without a redistributor region");
    };
    let d = intc.reg(0).ok_or("platform: GICv3 without a distributor")?;
    GICD_BASE.store(map(d.base, 0x1_0000, "platform: cannot map the GIC distributor")?, Ordering::SeqCst);
    GICR_BASE.store(map(r.base, r.size, "platform: cannot map the GIC redistributors")?, Ordering::SeqCst);
    GICR_SIZE.store(r.size as usize, Ordering::SeqCst);
    Ok(&GicV3)
}

/// The interrupts every CPU takes: the two SGIs (IPIs) and the timer PPIs.
const PER_CPU_INTS: u32 = (1 << SGI_TLB) | (1 << SGI_RESCHED) | (1 << PPI_EL1_VIRT) | (1 << PPI_EL1_PHYS);

/// The SGI an IPI is.
fn sgi(ipi: Ipi) -> u32 {
    match ipi {
        Ipi::Tlb => SGI_TLB,
        Ipi::Resched => SGI_RESCHED,
    }
}

/// Enable SPI `id` in the distributor, level-triggered or edge-triggered,
/// priority 0x80, delivered to the BSP (GICv2: CPU interface 0; GICv3: the
/// BSP's affinity).
fn enable_spi(id: u32, v3: bool, edge: bool) {
    if !(32..SPECIAL).contains(&id) {
        return;
    }
    // ICFGR: 2 bits per interrupt, 0b00 = level-sensitive, 0b10 = edge.
    let cfg = gicd() + GICD_ICFGR + (id as usize / 16) * 4;
    let shift = (id % 16) * 2;
    write32(cfg, (read32(cfg) & !(0b11 << shift)) | if edge { 0b10 << shift } else { 0 });
    write8(gicd() + GICD_IPRIORITYR + id as usize, 0x80);
    if v3 {
        let group = gicd() + GICD_IGROUPR + (id as usize / 32) * 4;
        write32(group, read32(group) | (1 << (id % 32)));
        write64(gicd() + GICD_IROUTER + id as usize * 8, BSP_AFFINITY.load(Ordering::Relaxed));
    } else {
        write8(gicd() + GICD_ITARGETSR + id as usize, 0x01);
    }
    write32(gicd() + GICD_ISENABLER + (id as usize / 32) * 4, 1 << (id % 32));
    unsafe {
        asm!("dsb sy", options(nostack));
    }
}

struct GicV2;

impl IrqChip for GicV2 {
    fn name(&self) -> &'static str {
        "gicv2"
    }

    /// The CPU interface and the distributor registers are banked per CPU:
    /// each CPU enables its own SGIs and timer PPIs. With only the
    /// interface enabled an AP's distributor bank stays reset-disabled: no
    /// timer PPI, so an idle AP sleeps in WFI forever, never soft-ACKs TLB
    /// shootdown epochs, and every shootdown burns the full 2M-spin bound
    /// (under `-smp 4` interactive stages crawl; PR #164 root cause).
    fn init_cpu(&self) {
        write32(gicd() + GICD_CTLR, 3); // enable group 0+1
        write32(gicc() + GICC_CTLR, 3); // enable group 0+1
        write32(gicc() + GICC_PMR, 0xFF); // accept all
        write32(gicd() + GICD_ISENABLER, PER_CPU_INTS);
        for id in [SGI_TLB, SGI_RESCHED, PPI_EL1_VIRT, PPI_EL1_PHYS] {
            write8(gicd() + GICD_IPRIORITYR + id as usize, 0x80);
        }
    }

    fn route(&self, irq: u32) -> Option<u32> {
        (32..SPECIAL).contains(&irq).then_some(irq)
    }

    fn unmask(&self, irq: u32) {
        enable_spi(irq, false, false);
    }

    fn unmask_edge(&self, irq: u32) {
        enable_spi(irq, false, true);
    }

    fn claim(&self) -> Option<u32> {
        let id = read32(gicc() + GICC_IAR) & 0x3FF;
        (id < SPECIAL).then_some(id)
    }

    fn complete(&self, id: u32) {
        write32(gicc() + GICC_EOIR, id);
    }

    /// `GICD_SGIR`: to every other CPU (target filter 0b01), or to CPU
    /// interface `n`, the CPU with MPIDR Aff0 = `n` on QEMU `virt`
    /// (CPUTargetList bit `n`, 8 of them).
    fn send_ipi(&self, to: Target, ipi: Ipi) {
        let id = sgi(ipi);
        let target = match to {
            Target::Cpu(cpu) => crate::smp::cpu_hw_id(cpu) & 0xff,
            Target::Others => 8,
        };
        unsafe {
            asm!("dsb ishst", options(nostack));
        }
        if target >= 8 {
            write32(gicd() + GICD_SGIR, (0b01 << 24) | id);
        } else {
            write32(gicd() + GICD_SGIR, (1 << (16 + target)) | id);
        }
    }
}

struct GicV3;

/// This CPU's MPIDR affinity fields packed as the GIC uses them:
/// `Aff3:Aff2:Aff1:Aff0` in 32 bits (`GICR_TYPER`), and as a routing value
/// (`GICD_IROUTER`: Aff3 at bit 32).
fn affinity() -> (u32, u64) {
    let mpidr: u64;
    unsafe {
        asm!("mrs {m}, mpidr_el1", m = out(reg) mpidr, options(nomem, nostack, preserves_flags));
    }
    let aff3 = (mpidr >> 32) & 0xFF;
    let low = mpidr & 0x00FF_FFFF;
    (((aff3 << 24) | low) as u32, (aff3 << 32) | low)
}

/// Wait for a distributor or redistributor register write to land
/// (`CTLR.RWP`).
fn wait_rwp(ctlr: usize) {
    while read32(ctlr) & GICD_CTLR_RWP != 0 {
        core::hint::spin_loop();
    }
}

/// GICv3 distributor, once: affinity routing on, every SPI in group 1,
/// disabled, routed to the BSP when enabled (`enable_spi`).
fn init_gicd_v3() {
    BSP_AFFINITY.store(affinity().1, Ordering::SeqCst);
    let lines = ((read32(gicd() + GICD_TYPER) & 0x1F) as usize + 1) * 32;
    for i in 1..lines / 32 {
        write32(gicd() + GICD_ICENABLER + i * 4, 0xFFFF_FFFF);
        write32(gicd() + GICD_IGROUPR + i * 4, 0xFFFF_FFFF);
    }
    wait_rwp(gicd() + GICD_CTLR);
    // ARE_NS, EnableGrp1NS, EnableGrp1 (the view differs with GICD_CTLR.DS;
    // these bits are right under both).
    write32(gicd() + GICD_CTLR, 0x13);
    wait_rwp(gicd() + GICD_CTLR);
}

/// This CPU's redistributor frame: the one whose `GICR_TYPER` names this
/// CPU's affinity. Frames are 128 KiB (256 KiB with the GICv4 VLPI frames),
/// up to the one marked `Last`.
fn this_redistributor() -> Option<usize> {
    let (aff, _) = affinity();
    let base = GICR_BASE.load(Ordering::Relaxed);
    let end = base + GICR_SIZE.load(Ordering::Relaxed);
    let mut frame = base;
    while frame + GICR_SGI_FRAME * 2 <= end {
        let typer = read64(frame + GICR_TYPER);
        if (typer >> 32) as u32 == aff {
            return Some(frame);
        }
        if typer & GICR_TYPER_LAST != 0 {
            break;
        }
        frame += if typer & GICR_TYPER_VLPIS != 0 { GICR_SGI_FRAME * 4 } else { GICR_SGI_FRAME * 2 };
    }
    None
}

/// `ICC_SGI1R_EL1`.
fn sgi1r(value: u64) {
    unsafe {
        asm!("dsb ishst", "msr S3_0_C12_C11_5, {v}", "isb", v = in(reg) value, options(nostack));
    }
}

impl IrqChip for GicV3 {
    fn name(&self) -> &'static str {
        "gicv3"
    }

    /// The distributor once (the BSP's runs before any AP starts), then
    /// this CPU's redistributor: woken, its SGIs and timer PPIs enabled
    /// (group 1, priority 0x80); then the system-register CPU interface:
    /// `ICC_SRE` (at EL2 as well when running there), the priority mask,
    /// group 1 delivery.
    fn init_cpu(&self) {
        if !GICD_V3_DONE.swap(true, Ordering::SeqCst) {
            init_gicd_v3();
        }
        let rd = this_redistributor().expect("gicv3: no redistributor for this CPU");
        let waker = read32(rd + GICR_WAKER) & !GICR_WAKER_PROCESSOR_SLEEP;
        write32(rd + GICR_WAKER, waker);
        while read32(rd + GICR_WAKER) & GICR_WAKER_CHILDREN_ASLEEP != 0 {
            core::hint::spin_loop();
        }
        let sgi = rd + GICR_SGI_FRAME;
        write32(sgi + GICR_ICENABLER0, 0xFFFF_FFFF);
        wait_rwp(rd + GICR_CTLR);
        write32(sgi + GICR_IGROUPR0, 0xFFFF_FFFF);
        for id in [SGI_TLB, SGI_RESCHED, PPI_EL1_VIRT, PPI_EL1_PHYS] {
            write8(sgi + GICR_IPRIORITYR + id as usize, 0x80);
        }
        write32(sgi + GICR_ISENABLER0, PER_CPU_INTS);
        unsafe {
            // ICC_SRE_EL1: SRE, DFB, DIB.
            asm!("msr S3_0_C12_C12_5, {v}", "isb", v = in(reg) 0x7u64, options(nostack));
            if super::current_el() >= 2 {
                // ICC_SRE_EL2: SRE, DFB, DIB, Enable (EL1 may use its own).
                asm!("msr S3_4_C12_C9_5, {v}", "isb", v = in(reg) 0xFu64, options(nostack));
            }
            // ICC_PMR_EL1: accept every priority.
            asm!("msr S3_0_C4_C6_0, {v}", v = in(reg) 0xFFu64, options(nostack));
            // ICC_BPR1_EL1: no preemption sub-groups.
            asm!("msr S3_0_C12_C12_3, {v}", v = in(reg) 0u64, options(nostack));
            // ICC_IGRPEN1_EL1: group 1 on.
            asm!("msr S3_0_C12_C12_7, {v}", "isb", v = in(reg) 1u64, options(nostack));
        }
    }

    fn route(&self, irq: u32) -> Option<u32> {
        (32..SPECIAL).contains(&irq).then_some(irq)
    }

    fn unmask(&self, irq: u32) {
        enable_spi(irq, true, false);
    }

    fn unmask_edge(&self, irq: u32) {
        enable_spi(irq, true, true);
    }

    fn claim(&self) -> Option<u32> {
        let iar: u64;
        unsafe {
            asm!("mrs {i}, S3_0_C12_C12_0", i = out(reg) iar, options(nomem, nostack)); // ICC_IAR1_EL1
        }
        let id = (iar & 0xFF_FFFF) as u32;
        (id < SPECIAL).then_some(id)
    }

    fn complete(&self, id: u32) {
        unsafe {
            asm!("msr S3_0_C12_C12_1, {i}", i = in(reg) u64::from(id), options(nostack)); // ICC_EOIR1_EL1
        }
    }

    /// `ICC_SGI1R_EL1`: to every other CPU (IRM), or to one by its
    /// affinity (Aff0 as a target list bit, 16 of them).
    fn send_ipi(&self, to: Target, ipi: Ipi) {
        let id = u64::from(sgi(ipi));
        let mpidr = match to {
            Target::Cpu(cpu) => Some(crate::smp::cpu_hw_id(cpu)).filter(|m| m & 0xff < 16),
            Target::Others => None,
        };
        match mpidr {
            Some(mpidr) => sgi1r(
                (id << 24)
                    | (1 << (mpidr & 0xff))
                    | (((mpidr >> 8) & 0xff) << 16)
                    | (((mpidr >> 16) & 0xff) << 32)
                    | (((mpidr >> 32) & 0xff) << 48),
            ),
            None => sgi1r((1 << 40) | (id << 24)),
        }
    }
}

fn read32(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

fn read64(addr: usize) -> u64 {
    unsafe { core::ptr::read_volatile(addr as *const u64) }
}

fn write8(addr: usize, value: u8) {
    unsafe { core::ptr::write_volatile(addr as *mut u8, value) }
}

fn write32(addr: usize, value: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, value) }
}

fn write64(addr: usize, value: u64) {
    unsafe { core::ptr::write_volatile(addr as *mut u64, value) }
}
