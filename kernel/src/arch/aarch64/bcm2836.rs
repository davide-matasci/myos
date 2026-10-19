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

/// The interrupt numbers this module hands out for the peripherals.
const GPU_FIRST: u32 = 32;
const BASIC_FIRST: u32 = 96;
const BASIC_COUNT: u32 = 8;

/// Spurious: nothing pending (as a GIC's 1023).
pub const SPURIOUS: u32 = 1023;

/// Both controllers' registers, mapped by the caller.
pub fn set(local: usize, armctrl: usize) {
    LOCAL.store(local, Ordering::SeqCst);
    ARMCTRL.store(armctrl, Ordering::SeqCst);
}

/// In use (the platform has no GIC but these).
pub fn active() -> bool {
    LOCAL.load(Ordering::Relaxed) != 0
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

/// This core's interrupts on: its timers and its mailbox 0; the BSP also
/// takes the peripherals' interrupt.
pub fn init_cpu() {
    let local = LOCAL.load(Ordering::Relaxed);
    let core = core();
    write32(local + MAILBOX0_CLEAR + core * 16, u32::MAX);
    write32(local + TIMER_CTL + core * 4, TIMER_IRQ_PHYS | TIMER_IRQ_VIRT);
    write32(local + MAILBOX_CTL + core * 4, 1);
    if core == 0 {
        write32(local + GPU_ROUTING, 0);
    }
}

/// The interrupt pending on this core, taken: a mailbox IPI is cleared
/// here; a timer or a peripheral stays pending until its source is
/// serviced. [`SPURIOUS`] when none is.
pub fn ack() -> u32 {
    let local = LOCAL.load(Ordering::Relaxed);
    let core = core();
    let source = read32(local + SOURCE + core * 4);
    if source & SOURCE_VIRT != 0 {
        return super::interrupts::PPI_EL1_VIRT;
    }
    if source & SOURCE_PHYS != 0 {
        return super::interrupts::PPI_EL1_PHYS;
    }
    if source & SOURCE_MAILBOX0 != 0 {
        let mailbox = local + MAILBOX0_CLEAR + core * 16;
        let bits = read32(mailbox);
        if bits != 0 {
            let ipi = bits.trailing_zeros();
            write32(mailbox, 1 << ipi);
            return ipi;
        }
    }
    if source & SOURCE_GPU != 0 {
        let armctrl = ARMCTRL.load(Ordering::Relaxed);
        let pending1 = read32(armctrl + PENDING1) & read32(armctrl + ENABLE1);
        if pending1 != 0 {
            return GPU_FIRST + pending1.trailing_zeros();
        }
        let pending2 = read32(armctrl + PENDING2) & read32(armctrl + ENABLE2);
        if pending2 != 0 {
            return GPU_FIRST + 32 + pending2.trailing_zeros();
        }
        let basic = read32(armctrl + BASIC_PENDING) & read32(armctrl + ENABLE_BASIC) & ((1 << BASIC_COUNT) - 1);
        if basic != 0 {
            return BASIC_FIRST + basic.trailing_zeros();
        }
    }
    SPURIOUS
}

/// Enable peripheral interrupt `irq` (delivered to the BSP).
pub fn enable(irq: u32) {
    let armctrl = ARMCTRL.load(Ordering::Relaxed);
    match irq {
        GPU_FIRST..BASIC_FIRST => {
            let n = irq - GPU_FIRST;
            let reg = if n < 32 { ENABLE1 } else { ENABLE2 };
            write32(armctrl + reg, 1 << (n % 32));
        }
        BASIC_FIRST.. if irq < BASIC_FIRST + BASIC_COUNT => write32(armctrl + ENABLE_BASIC, 1 << (irq - BASIC_FIRST)),
        _ => {}
    }
}

/// IPI `ipi` (0 or 1) to core `target`.
pub fn send(target: usize, ipi: u32) {
    let local = LOCAL.load(Ordering::Relaxed);
    unsafe {
        core::arch::asm!("dsb ishst", options(nostack));
    }
    write32(local + MAILBOX0_SET + (target & 3) * 16, 1 << ipi);
}

/// IPI `ipi` to every other core.
pub fn send_others(ipi: u32) {
    let me = core();
    for target in (0..4).filter(|&c| c != me) {
        send(target, ipi);
    }
}

/// A device tree interrupt specifier of the BCM2835 controller (`bank,
/// number`: bank 0 the basic interrupts, 1 and 2 GPU interrupts 0..31 and
/// 32..63) as this module numbers it.
pub fn irq_from_dt(bank: u32, n: u32) -> Option<u32> {
    match bank {
        0 if n < BASIC_COUNT => Some(BASIC_FIRST + n),
        1 | 2 if n < 32 => Some(GPU_FIRST + (bank - 1) * 32 + n),
        _ => None,
    }
}
