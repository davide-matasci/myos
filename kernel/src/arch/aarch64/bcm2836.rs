//! The interrupt controllers of a Raspberry Pi 3 (BCM2836/2837), where an
//! aarch64 board has a GIC: a per-core local controller (the ARM's own
//! interrupts: the generic timers, four mailboxes per core, and the GPU's
//! interrupt, routed to one core) in front of the BCM2835 controller of the
//! peripherals. Register layouts: Broadcom's "BCM2836 ARM-local
//! peripherals" (QA7) and "BCM2835 ARM Peripherals" documents.
//!
//! The interrupt numbers are the GIC's, so that the rest of the kernel
//! does not tell the two apart: mailbox 0 bit 0 and 1 are the IPIs 0 (TLB
//! shootdown) and 1 (reschedule), the virtual and physical timers 27 and
//! 30, and a peripheral's interrupt is 32 + n for GPU interrupt n (0..63)
//! and 96 + n for basic interrupt n (0..7), the `irq::dispatch` number.

use core::sync::atomic::{AtomicUsize, Ordering};

use super::interrupts::{PPI_EL1_PHYS, PPI_EL1_VIRT, SGI_RESCHED, SGI_TLB};
use crate::irqchip::{DtNode, Driver, IrqChip, Ipi, Target};
use crate::platform::Intc;

/// The local controller (kernel virtual).
static LOCAL: AtomicUsize = AtomicUsize::new(0);
/// The BCM2835 controller of the peripherals (kernel virtual).
static ARMCTRL: AtomicUsize = AtomicUsize::new(0);

/// Local: the core that takes the GPU's (peripherals') interrupt.
const GPU_ROUTING: usize = 0x0c;
/// Local, per core: the timers' interrupt enables (IRQ: bit 1 the
/// non-secure physical timer, bit 3 the virtual one).
const TIMER_CTL: usize = 0x40;
const TIMER_IRQ_PHYS: u32 = 1 << 1;
const TIMER_IRQ_VIRT: u32 = 1 << 3;
/// Local, per core: the mailboxes' interrupt enables (bit `n`: mailbox `n`).
const MAILBOX_CTL: usize = 0x50;
/// Local, per core: what interrupts it.
const SOURCE: usize = 0x60;
const SOURCE_PHYS: u32 = 1 << 1;
const SOURCE_VIRT: u32 = 1 << 3;
const SOURCE_MAILBOX0: u32 = 1 << 4;
const SOURCE_GPU: u32 = 1 << 8;
/// Local, per core: mailbox 0, write-1-to-set and read / write-1-to-clear.
const MAILBOX0_SET: usize = 0x80;
const MAILBOX0_CLEAR: usize = 0xc0;

/// BCM2835: pending, enable (write 1 to set) of the basic interrupts and of
/// GPU interrupts 0..31, 32..63.
const BASIC_PENDING: usize = 0x00;
const PENDING1: usize = 0x04;
const PENDING2: usize = 0x08;
const ENABLE1: usize = 0x10;
const ENABLE2: usize = 0x14;
const ENABLE_BASIC: usize = 0x18;
/// And the disable registers, write 1 to clear.
const DISABLE1: usize = 0x1c;
const DISABLE2: usize = 0x20;
const DISABLE_BASIC: usize = 0x24;

/// The interrupt numbers this module hands out for the peripherals.
const GPU_FIRST: u32 = 32;
const BASIC_FIRST: u32 = 96;
const BASIC_COUNT: u32 = 8;

pub static DRIVER: Driver = Driver {
    name: "bcm2836",
    dt: &[
        DtNode { compatible: &["brcm,bcm2836-l1-intc"], regs: 1 },
        DtNode { compatible: &["brcm,bcm2836-armctrl-ic"], regs: 1 },
    ],
    xlate,
    start,
};

/// A device tree interrupt specifier: of the BCM2835 controller (node 1),
/// `bank, number` (bank 0 the basic interrupts, 1 and 2 GPU interrupts
/// 0..31 and 32..63) as this module numbers them. The local controller's
/// own (node 0: the timers, the mailboxes) are not devices' interrupts.
fn xlate(node: usize, cells: &[u32]) -> Option<u32> {
    match (node, cells) {
        (1, [0, n, ..]) if *n < BASIC_COUNT => Some(BASIC_FIRST + n),
        (1, [bank @ (1 | 2), n, ..]) if *n < 32 => Some(GPU_FIRST + (bank - 1) * 32 + n),
        _ => None,
    }
}

/// Both controllers' registers mapped.
fn start(intc: &Intc) -> Result<&'static dyn IrqChip, &'static str> {
    let (Some(local), Some(armctrl)) = (intc.reg(0), intc.reg(1)) else {
        return Err("platform: bcm2836 without its two controllers");
    };
    let map = |phys: u64, size: u64, what| super::paging::map_mmio(phys, size).ok_or(what);
    LOCAL.store(map(local.base, 0x100, "platform: cannot map the local interrupt controller")?, Ordering::SeqCst);
    ARMCTRL.store(map(armctrl.base, 0x200, "platform: cannot map the interrupt controller")?, Ordering::SeqCst);
    Ok(&Bcm2836)
}

fn read32(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

fn write32(addr: usize, value: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, value) }
}

/// This core's number (MPIDR Aff0: 0..3).
fn core() -> usize {
    let mpidr: u64;
    unsafe {
        core::arch::asm!("mrs {m}, mpidr_el1", m = out(reg) mpidr, options(nomem, nostack, preserves_flags));
    }
    (mpidr & 3) as usize
}

struct Bcm2836;

impl IrqChip for Bcm2836 {
    fn name(&self) -> &'static str {
        "bcm2836"
    }

    /// This core's interrupts on: its timers and its mailbox 0; the BSP
    /// also takes the peripherals' interrupt.
    fn init_cpu(&self) {
        let local = LOCAL.load(Ordering::Relaxed);
        let core = core();
        write32(local + MAILBOX0_CLEAR + core * 16, u32::MAX);
        write32(local + TIMER_CTL + core * 4, TIMER_IRQ_PHYS | TIMER_IRQ_VIRT);
        write32(local + MAILBOX_CTL + core * 4, 1);
        if core == 0 {
            write32(local + GPU_ROUTING, 0);
        }
    }

    fn route(&self, irq: u32) -> Option<u32> {
        (GPU_FIRST..BASIC_FIRST + BASIC_COUNT).contains(&irq).then_some(irq)
    }

    /// Enable peripheral interrupt `irq` (delivered to the BSP).
    fn unmask(&self, irq: u32) {
        let armctrl = ARMCTRL.load(Ordering::Relaxed);
        match irq {
            GPU_FIRST..BASIC_FIRST => {
                let n = irq - GPU_FIRST;
                let reg = if n < 32 { ENABLE1 } else { ENABLE2 };
                write32(armctrl + reg, 1 << (n % 32));
            }
            BASIC_FIRST.. if irq < BASIC_FIRST + BASIC_COUNT => {
                write32(armctrl + ENABLE_BASIC, 1 << (irq - BASIC_FIRST))
            }
            _ => {}
        }
    }

    fn mask(&self, irq: u32) {
        let armctrl = ARMCTRL.load(Ordering::Relaxed);
        match irq {
            GPU_FIRST..BASIC_FIRST => {
                let n = irq - GPU_FIRST;
                let reg = if n < 32 { DISABLE1 } else { DISABLE2 };
                write32(armctrl + reg, 1 << (n % 32));
            }
            BASIC_FIRST.. if irq < BASIC_FIRST + BASIC_COUNT => {
                write32(armctrl + DISABLE_BASIC, 1 << (irq - BASIC_FIRST))
            }
            _ => {}
        }
    }

    /// The interrupt pending on this core, taken: a mailbox IPI is cleared
    /// here; a timer or a peripheral stays pending until its source is
    /// serviced.
    fn claim(&self) -> Option<u32> {
        let local = LOCAL.load(Ordering::Relaxed);
        let core = core();
        let source = read32(local + SOURCE + core * 4);
        if source & SOURCE_VIRT != 0 {
            return Some(PPI_EL1_VIRT);
        }
        if source & SOURCE_PHYS != 0 {
            return Some(PPI_EL1_PHYS);
        }
        if source & SOURCE_MAILBOX0 != 0 {
            let mailbox = local + MAILBOX0_CLEAR + core * 16;
            let bits = read32(mailbox);
            if bits != 0 {
                let ipi = bits.trailing_zeros();
                write32(mailbox, 1 << ipi);
                return Some(ipi);
            }
        }
        if source & SOURCE_GPU != 0 {
            let armctrl = ARMCTRL.load(Ordering::Relaxed);
            let pending1 = read32(armctrl + PENDING1) & read32(armctrl + ENABLE1);
            if pending1 != 0 {
                return Some(GPU_FIRST + pending1.trailing_zeros());
            }
            let pending2 = read32(armctrl + PENDING2) & read32(armctrl + ENABLE2);
            if pending2 != 0 {
                return Some(GPU_FIRST + 32 + pending2.trailing_zeros());
            }
            let basic = read32(armctrl + BASIC_PENDING) & read32(armctrl + ENABLE_BASIC) & ((1 << BASIC_COUNT) - 1);
            if basic != 0 {
                return Some(BASIC_FIRST + basic.trailing_zeros());
            }
        }
        None
    }

    /// Nothing: no end of interrupt here, a source stays pending until
    /// serviced.
    fn complete(&self, _id: u32) {}

    /// Bit 0 (TLB) or 1 (reschedule) of the target cores' mailbox 0.
    fn send_ipi(&self, to: Target, ipi: Ipi) {
        let bit = match ipi {
            Ipi::Tlb => SGI_TLB,
            Ipi::Resched => SGI_RESCHED,
        };
        let local = LOCAL.load(Ordering::Relaxed);
        unsafe {
            core::arch::asm!("dsb ishst", options(nostack));
        }
        let me = core();
        for target in 0..4 {
            let to_it = match to {
                Target::Cpu(cpu) => crate::smp::cpu_hw_id(cpu) & 3 == target as u64,
                Target::Others => target != me,
            };
            if to_it {
                write32(local + MAILBOX0_SET + target * 16, 1 << bit);
            }
        }
    }
}
