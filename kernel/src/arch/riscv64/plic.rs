//! The SiFive-style PLIC (base from the device tree): the devices'
//! interrupts, enabled for the boot hart's S-mode context. The IPIs are
//! the SBI's (supervisor software interrupts), sent here as well.

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::irqchip::{DtNode, Driver, IrqChip, Ipi, Target};
use crate::platform::Intc;

pub static DRIVER: Driver = Driver {
    name: "plic",
    dt: &[DtNode { compatible: &["sifive,plic-1.0.0", "riscv,plic0"], regs: 1 }],
    xlate,
    start,
};

/// A PLIC interrupt specifier: the source number.
fn xlate(_node: usize, cells: &[u32]) -> Option<u32> {
    match cells {
        [src] if *src != 0 && *src < SOURCES => Some(*src),
        _ => None,
    }
}

/// The registers, where Limine's map has the low device range
/// (`paging::map_devices`).
fn start(intc: &Intc) -> Result<&'static dyn IrqChip, &'static str> {
    let reg = intc.reg(0).ok_or("platform: PLIC without registers")?;
    BASE.store(reg.base as usize, Ordering::SeqCst);
    Ok(&Plic)
}

static BASE: AtomicUsize = AtomicUsize::new(0);

fn base() -> usize {
    BASE.load(Ordering::Relaxed)
}

const SOURCES: u32 = 1024;
const PRIORITY: usize = 0x0;
const ENABLE: usize = 0x2000;
const ENABLE_STRIDE: usize = 0x80;
const CONTEXT: usize = 0x20_0000;
const CONTEXT_STRIDE: usize = 0x1000;

/// S-mode PLIC context of the BSP (`2 * hartid + 1`), resolved on first use.
static CTX: AtomicUsize = AtomicUsize::new(usize::MAX);

fn ctx() -> usize {
    let c = CTX.load(Ordering::Relaxed);
    if c != usize::MAX {
        return c;
    }
    let hart = crate::smp::cpu_hw_id(0) as usize;
    let c = 2 * hart + 1;
    CTX.store(c, Ordering::Relaxed);
    c
}

fn w(off: usize, v: u32) {
    unsafe { core::ptr::write_volatile((base() + off) as *mut u32, v) }
}

fn r(off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base() + off) as *const u32) }
}

struct Plic;

impl IrqChip for Plic {
    fn name(&self) -> &'static str {
        "plic"
    }

    /// Nothing per hart: the sources go to the BSP's context, which
    /// [`IrqChip::unmask`] sets up.
    fn init_cpu(&self) {}

    fn route(&self, irq: u32) -> Option<u32> {
        (1..SOURCES).contains(&irq).then_some(irq)
    }

    /// Enable `src` for the BSP's S-mode context (priority 1, threshold 0)
    /// and turn on supervisor external interrupts.
    fn unmask(&self, src: u32) {
        if src == 0 || src >= SOURCES {
            return;
        }
        let c = ctx();
        w(PRIORITY + 4 * src as usize, 1);
        let en = ENABLE + c * ENABLE_STRIDE + (src as usize / 32) * 4;
        w(en, r(en) | (1 << (src % 32)));
        w(CONTEXT + c * CONTEXT_STRIDE, 0);
        unsafe {
            core::arch::asm!("csrs sie, {}", in(reg) 1u64 << 9, options(nostack)); // SEIE
        }
    }

    fn claim(&self) -> Option<u32> {
        let src = r(CONTEXT + ctx() * CONTEXT_STRIDE + 4);
        (src != 0).then_some(src)
    }

    fn complete(&self, src: u32) {
        w(CONTEXT + ctx() * CONTEXT_STRIDE + 4, src);
    }

    /// A supervisor software interrupt through the SBI; which IPI it is
    /// travels in `smp`'s mailbox (`smp::ipi_mark_tlb`, ...).
    fn send_ipi(&self, to: Target, _ipi: Ipi) {
        match to {
            Target::Cpu(cpu) => sbi_send_ipi(1, crate::smp::cpu_hw_id(cpu)),
            Target::Others => ipi_others(),
        }
    }
}

const SBI_EXT_IPI: u64 = 0x73_5049; // "sPI", the SBI spec's IPI extension
const SBI_IPI_SEND: u64 = 0;

fn sbi_send_ipi(hart_mask: u64, hart_mask_base: u64) {
    let mut _err: i64;
    let mut _val: i64;
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") SBI_EXT_IPI,
            in("a6") SBI_IPI_SEND,
            inout("a0") hart_mask as i64 => _err,
            inout("a1") hart_mask_base as i64 => _val,
            options(nostack),
        );
    }
}

/// Send a software IPI to every online hart except self (logical indices).
fn ipi_others() {
    let self_id = crate::smp::cpu_id();
    let mut mask = 0u64;
    let mut base = u64::MAX;
    for i in 0..crate::smp::MAX_CPUS {
        if i == self_id || !crate::smp::cpu_online(i) {
            continue;
        }
        let hart = crate::smp::cpu_hw_id(i);
        if base == u64::MAX {
            base = hart;
        }
        if hart >= base && hart < base + 64 {
            mask |= 1u64 << (hart - base);
        } else {
            // Hart id outside current window — send a singleton.
            sbi_send_ipi(1, hart);
        }
    }
    if mask != 0 && base != u64::MAX {
        sbi_send_ipi(mask, base);
    }
}
