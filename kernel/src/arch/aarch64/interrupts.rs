//! Exception vectors, the GIC (v2: memory-mapped distributor and CPU
//! interface; v3: distributor with affinity routing, one redistributor per
//! CPU found by MPIDR, the CPU interface through the `ICC_*` system
//! registers) and the generic timers.
//!
//! Limine (base rev 6) enters with PSTATE.SP=0 (SP_EL0), either at EL1 or at
//! EL2 with VHE (`HCR_EL2.{E2H,TGE}`). IRQs taken with SPSel=0 use the
//! Current-EL SP0 slot, not SP_ELx; the handler has to live in both.
//!
//! CI #47: CNTP (PPI 30) plus SP0 vectors prints int ok. CI #50: LLVM on
//! nightly-2026-07-26 rejects `cnthv_*_el2`, so stay on EL0 timer registers.

use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// The GIC, from the platform description (`set_gic` / `set_gicv3`): the
/// distributor, and either the GICv2 CPU interface or the GICv3
/// redistributor region (`GICR_SIZE` bytes of frames, one per CPU).
static GICD_BASE: AtomicUsize = AtomicUsize::new(0);
static GICC_BASE: AtomicUsize = AtomicUsize::new(0);
static GICR_BASE: AtomicUsize = AtomicUsize::new(0);
static GICR_SIZE: AtomicUsize = AtomicUsize::new(0);
static GIC_V3: AtomicBool = AtomicBool::new(false);
static GICD_V3_DONE: AtomicBool = AtomicBool::new(false);
/// The BSP's affinity, where SPIs are routed (GICv3 `GICD_IROUTER`).
static BSP_AFFINITY: AtomicU64 = AtomicU64::new(0);

pub fn set_gic(gicd: usize, gicc: usize) {
    GICD_BASE.store(gicd, Ordering::SeqCst);
    GICC_BASE.store(gicc, Ordering::SeqCst);
    GIC_V3.store(false, Ordering::SeqCst);
}

pub fn set_gicv3(gicd: usize, gicr: usize, gicr_size: usize) {
    // QEMU `virt` keeps both in the identity-mapped low 1 GiB; a board
    // with them elsewhere gets its 1 GiB device blocks.
    let gicd = super::paging::map_mmio(gicd as u64, 0x1_0000).unwrap_or(gicd);
    let gicr = super::paging::map_mmio(gicr as u64, gicr_size as u64).unwrap_or(gicr);
    GICD_BASE.store(gicd, Ordering::SeqCst);
    GICR_BASE.store(gicr, Ordering::SeqCst);
    GICR_SIZE.store(gicr_size, Ordering::SeqCst);
    GIC_V3.store(true, Ordering::SeqCst);
}

fn gicd() -> usize {
    GICD_BASE.load(Ordering::Relaxed)
}

fn gicc() -> usize {
    GICC_BASE.load(Ordering::Relaxed)
}

fn gic_v3() -> bool {
    GIC_V3.load(Ordering::Relaxed)
}

/// The GIC version, for the boot log.
pub fn gic_name() -> &'static str {
    if gic_v3() { "gicv3" } else { "gicv2" }
}

// GICv3 distributor registers (with affinity routing).
const GICD_CTLR: usize = 0x0000;
const GICD_TYPER: usize = 0x0004;
const GICD_IGROUPR: usize = 0x0080;
const GICD_ISENABLER: usize = 0x0100;
const GICD_ICENABLER: usize = 0x0180;
const GICD_IPRIORITYR: usize = 0x0400;
const GICD_ICFGR: usize = 0x0C00;
const GICD_IROUTER: usize = 0x6000;
const GICD_CTLR_RWP: u32 = 1 << 31;
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
const PPI_EL1_VIRT: u32 = 27; // CNTV
const PPI_EL1_PHYS: u32 = 30; // CNTP
const SGI_TLB: u32 = 0;
const SGI_RESCHED: u32 = 1;

static TIMER_FIRED: AtomicBool = AtomicBool::new(false);

global_asm!(
    r#"
    .align 11
    .global exception_vectors
exception_vectors:
    // Current EL, SP_EL0 (Limine entry: PSTATE.SP=0)
    .align 7
    b sync_el
    .align 7
    b irq_el1h
    .align 7
    b irq_el1h
    .align 7
    b exception_unhandled
    // Current EL, SP_ELx
    .align 7
    b sync_el
    .align 7
    b irq_el1h
    .align 7
    b irq_el1h
    .align 7
    b exception_unhandled
    // Lower EL, AArch64
    .align 7
    b lower_sync
    .align 7
    b irq_el1h
    .align 7
    b irq_el1h
    .align 7
    b exception_unhandled
    // Lower EL, AArch32
    .align 7
    b exception_unhandled
    .align 7
    b exception_unhandled
    .align 7
    b exception_unhandled
    .align 7
    b exception_hang

    // Frame (16*18): x0-x29 pairs at 16*0..14, x30 at 16*15,
    // elr/spsr at 16*16, sp_el0 at 16*17. ELR/SPSR are not VHE-aliased:
    // exception to EL2 writes ELR_EL2, so CurrentEL>=2 uses the EL2 bank.
    // CPU ELR/SPSR/SP_EL0 are not banked per-task; wait/preempt enter()
    // would clobber them.

irq_el1h:
    sub sp, sp, #(16 * 18)
    stp x0, x1, [sp, #16 * 0]
    stp x2, x3, [sp, #16 * 1]
    stp x4, x5, [sp, #16 * 2]
    stp x6, x7, [sp, #16 * 3]
    stp x8, x9, [sp, #16 * 4]
    stp x10, x11, [sp, #16 * 5]
    stp x12, x13, [sp, #16 * 6]
    stp x14, x15, [sp, #16 * 7]
    stp x16, x17, [sp, #16 * 8]
    stp x18, x19, [sp, #16 * 9]
    stp x20, x21, [sp, #16 * 10]
    stp x22, x23, [sp, #16 * 11]
    stp x24, x25, [sp, #16 * 12]
    stp x26, x27, [sp, #16 * 13]
    stp x28, x29, [sp, #16 * 14]
    str x30, [sp, #16 * 15]
    mrs x3, CurrentEL
    cmp x3, #8
    b.lt irq_save_el1
    mrs x0, elr_el2
    mrs x1, spsr_el2
    b irq_save_done
irq_save_el1:
    mrs x0, elr_el1
    mrs x1, spsr_el1
irq_save_done:
    mrs x2, sp_el0
    stp x0, x1, [sp, #16 * 16]
    str x2, [sp, #16 * 17]
    mov x0, x1 // spsr: the interrupted mode
    bl aarch64_irq_handler
    ldp x0, x1, [sp, #16 * 16]
    ldr x2, [sp, #16 * 17]
    mrs x3, CurrentEL
    cmp x3, #8
    b.lt irq_rest_el1
    msr elr_el2, x0
    msr spsr_el2, x1
    b irq_rest_done
irq_rest_el1:
    msr elr_el1, x0
    msr spsr_el1, x1
irq_rest_done:
    msr sp_el0, x2
    isb
    ldr x30, [sp, #16 * 15]
    ldp x28, x29, [sp, #16 * 14]
    ldp x26, x27, [sp, #16 * 13]
    ldp x24, x25, [sp, #16 * 12]
    ldp x22, x23, [sp, #16 * 11]
    ldp x20, x21, [sp, #16 * 10]
    ldp x18, x19, [sp, #16 * 9]
    ldp x16, x17, [sp, #16 * 8]
    ldp x14, x15, [sp, #16 * 7]
    ldp x12, x13, [sp, #16 * 6]
    ldp x10, x11, [sp, #16 * 5]
    ldp x8, x9, [sp, #16 * 4]
    ldp x6, x7, [sp, #16 * 3]
    ldp x4, x5, [sp, #16 * 2]
    ldp x2, x3, [sp, #16 * 1]
    ldp x0, x1, [sp, #16 * 0]
    add sp, sp, #(16 * 18)
    eret

lower_sync:
    sub sp, sp, #(16 * 18)
    stp x0, x1, [sp, #16 * 0]
    stp x2, x3, [sp, #16 * 1]
    stp x4, x5, [sp, #16 * 2]
    stp x6, x7, [sp, #16 * 3]
    stp x8, x9, [sp, #16 * 4]
    stp x10, x11, [sp, #16 * 5]
    stp x12, x13, [sp, #16 * 6]
    stp x14, x15, [sp, #16 * 7]
    stp x16, x17, [sp, #16 * 8]
    stp x18, x19, [sp, #16 * 9]
    stp x20, x21, [sp, #16 * 10]
    stp x22, x23, [sp, #16 * 11]
    stp x24, x25, [sp, #16 * 12]
    stp x26, x27, [sp, #16 * 13]
    stp x28, x29, [sp, #16 * 14]
    str x30, [sp, #16 * 15]
    mrs x3, CurrentEL
    cmp x3, #8
    b.lt sync_save_el1
    mrs x0, elr_el2
    mrs x1, spsr_el2
    b sync_save_done
sync_save_el1:
    mrs x0, elr_el1
    mrs x1, spsr_el1
sync_save_done:
    mrs x2, sp_el0
    stp x0, x1, [sp, #16 * 16]
    str x2, [sp, #16 * 17]
    mov x0, sp
    bl aarch64_lower_sync
    ldp x0, x1, [sp, #16 * 16]
    ldr x2, [sp, #16 * 17]
    mrs x3, CurrentEL
    cmp x3, #8
    b.lt sync_rest_el1
    msr elr_el2, x0
    msr spsr_el2, x1
    b sync_rest_done
sync_rest_el1:
    msr elr_el1, x0
    msr spsr_el1, x1
sync_rest_done:
    msr sp_el0, x2
    isb
    ldr x30, [sp, #16 * 15]
    ldp x28, x29, [sp, #16 * 14]
    ldp x26, x27, [sp, #16 * 13]
    ldp x24, x25, [sp, #16 * 12]
    ldp x22, x23, [sp, #16 * 11]
    ldp x20, x21, [sp, #16 * 10]
    ldp x18, x19, [sp, #16 * 9]
    ldp x16, x17, [sp, #16 * 8]
    ldp x14, x15, [sp, #16 * 7]
    ldp x12, x13, [sp, #16 * 6]
    ldp x10, x11, [sp, #16 * 5]
    ldp x8, x9, [sp, #16 * 4]
    ldp x6, x7, [sp, #16 * 3]
    ldp x4, x5, [sp, #16 * 2]
    ldp x2, x3, [sp, #16 * 1]
    ldp x0, x1, [sp, #16 * 0]
    add sp, sp, #(16 * 18)
    eret

sync_el:
    sub sp, sp, #(16 * 4)
    stp x0, x1, [sp, #16 * 0]
    stp x2, x3, [sp, #16 * 1]
    str x30, [sp, #16 * 2]
    mov x0, sp
    bl aarch64_sync_handler

exception_unhandled:
    sub sp, sp, #(16 * 4)
    stp x0, x1, [sp, #16 * 0]
    stp x2, x3, [sp, #16 * 1]
    str x30, [sp, #16 * 2]
    mov x0, sp
    bl aarch64_unhandled_exception
    b exception_hang

exception_hang:
    b exception_hang

    // Fork child resume: same restore path as lower_sync after a syscall.
    // x0 = pointer to a 16*18 byte frame (x0 patched to 0 by the caller).
    .global fork_eret_from_frame
fork_eret_from_frame:
    mov sp, x0
    ldp x0, x1, [sp, #16 * 16]
    ldr x2, [sp, #16 * 17]
    mrs x3, CurrentEL
    cmp x3, #8
    b.lt fork_rest_el1
    msr elr_el2, x0
    msr spsr_el2, x1
    b fork_rest_done
fork_rest_el1:
    msr elr_el1, x0
    msr spsr_el1, x1
fork_rest_done:
    msr sp_el0, x2
    isb
    ldr x30, [sp, #16 * 15]
    ldp x28, x29, [sp, #16 * 14]
    ldp x26, x27, [sp, #16 * 13]
    ldp x24, x25, [sp, #16 * 12]
    ldp x22, x23, [sp, #16 * 11]
    ldp x20, x21, [sp, #16 * 10]
    ldp x18, x19, [sp, #16 * 9]
    ldp x16, x17, [sp, #16 * 8]
    ldp x14, x15, [sp, #16 * 7]
    ldp x12, x13, [sp, #16 * 6]
    ldp x10, x11, [sp, #16 * 5]
    ldp x8, x9, [sp, #16 * 4]
    ldp x6, x7, [sp, #16 * 3]
    ldp x4, x5, [sp, #16 * 2]
    ldp x2, x3, [sp, #16 * 1]
    ldp x0, x1, [sp, #16 * 0]
    add sp, sp, #(16 * 18)
    eret
    "#
);

unsafe extern "C" {
    fn exception_vectors();
    fn fork_eret_from_frame(frame: *mut u64) -> !;
}

/// Resume a forked child by restoring a saved `lower_sync` frame (x0 = 0).
pub fn fork_eret_to_user(frame: *mut u64) -> ! {
    unsafe { fork_eret_from_frame(frame) }
}

fn current_el() -> u64 {
    let el: u64;
    unsafe {
        asm!(
            "mrs {el}, CurrentEL",
            el = out(reg) el,
            options(nomem, nostack, preserves_flags)
        );
    }
    (el >> 2) & 3
}

/// Copy SP_EL0 onto SP_ELx and switch to SPSel=1 so later IRQs use the SPx slot.
fn use_spx() {
    unsafe {
        asm!(
            "mov {tmp}, sp",
            "msr spsel, #1",
            "isb",
            "mov sp, {tmp}",
            tmp = out(reg) _,
            options(),
        );
    }
}

pub fn init() {
    use_spx();
    // VHE at EL2 redirects *_EL1 onto the EL2 bank; still set VBAR_EL2 when
    // we are actually at EL2 so a non-VHE handoff would work too.
    let v = exception_vectors as *const () as usize;
    unsafe {
        asm!("msr vbar_el1, {v}", "isb", v = in(reg) v, options(nostack));
        if current_el() >= 2 {
            asm!("msr vbar_el2, {v}", "isb", v = in(reg) v, options(nostack));
        }
        asm!("msr tpidr_el1, xzr", options(nostack));
    }
    crate::console::status_info(gic_name());
    init_gic();
    init_timer();
    unsafe {
        asm!("dsb sy", options(nostack));
        asm!("msr daifclr, #3", options(nostack)); // unmask IRQ+FIQ
    }
}

/// Secondary CPU: vectors already set globally; enable gicc() + timers.
pub fn ap_init(logical: usize) {
    // Match BSP TTBR0 (device MMIO) before any GIC/UART access.
    super::paging::apply_bsp_device_map();
    use_spx();
    let v = exception_vectors as *const () as usize;
    unsafe {
        asm!("msr vbar_el1, {v}", "isb", v = in(reg) v, options(nostack));
        if current_el() >= 2 {
            asm!("msr vbar_el2, {v}", "isb", v = in(reg) v, options(nostack));
        }
        // Logical CPU id for per-CPU syscall / stack state.
        asm!("msr tpidr_el1, {id}", id = in(reg) logical, options(nostack));
        // Limine parks APs with CPACR/CPTR FPEN=0; enable FP/SIMD for any
        // codegen that touches NEON (and for later EL0 userspace on this CPU).
        let mut cpacr: u64;
        asm!("mrs {c}, cpacr_el1", c = out(reg) cpacr, options(nomem, nostack));
        cpacr |= 3 << 20;
        asm!("msr cpacr_el1, {c}", "isb", c = in(reg) cpacr, options(nostack));
    }
    // GICv2 CPU interface is banked per-CPU: run the full GIC init on this
    // CPU's own bank, including ISENABLER0 (SGI 0/1 + timer PPIs) and per-ID
    // priorities. With only the interface enabled the AP's distributor bank
    // stays reset-disabled: no timer PPI, so an idle AP sleeps in WFI forever
    // (sched counts frozen), never soft-ACKs TLB shootdown epochs, and every
    // shootdown burns the full 2M-spin bound — under `-smp 4` interactive
    // stages crawl (curl timeout, ostest crawl; PR #164 root cause).
    init_gic();
    init_timer();
    // Leave DAIF masked until ap_idle_loop installs CURRENT / ONLINE.
    unsafe {
        asm!("dsb sy", options(nostack));
    }
}

pub fn wait_for_interrupt_proof() {
    while !TIMER_FIRED.load(Ordering::SeqCst) {
        unsafe {
            asm!("wfi", options(nostack, preserves_flags));
        }
    }
}

/// The interrupts every CPU takes: the two SGIs (IPIs) and the timer PPIs.
const PER_CPU_INTS: u32 =
    (1 << SGI_TLB) | (1 << SGI_RESCHED) | (1 << PPI_EL1_VIRT) | (1 << PPI_EL1_PHYS);

/// Bring this CPU's interrupt delivery up: on a GICv2 the banked distributor
/// registers and the CPU interface, on a GICv3 the distributor once (the
/// BSP) then this CPU's redistributor and system-register interface.
fn init_gic() {
    if gic_v3() {
        // The BSP's `init` runs before any AP starts.
        if !GICD_V3_DONE.swap(true, Ordering::SeqCst) {
            init_gicd_v3();
        }
        init_gicv3_cpu();
        return;
    }
    write32(gicd(), 3); // GICD_CTLR enable group 0+1
    write32(gicc(), 3); // GICC_CTLR enable group 0+1
    write32(gicc() + 0x004, 0xFF); // PMR: accept all
    // Enable SGIs 0/1 (IPI) + timer PPIs.
    write32(gicd() + 0x100, PER_CPU_INTS);
    for id in [SGI_TLB, SGI_RESCHED, PPI_EL1_VIRT, PPI_EL1_PHYS] {
        unsafe {
            core::ptr::write_volatile((gicd() + 0x400 + id as usize) as *mut u8, 0x80);
        }
    }
}

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
/// disabled, routed to the BSP when enabled (`gic_enable_spi`).
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

/// GICv3, this CPU: wake its redistributor, enable the SGIs and timer PPIs
/// there (group 1, priority 0x80), then the system-register CPU interface:
/// `ICC_SRE` (at EL2 as well when running there), the priority mask, group
/// 1 delivery.
fn init_gicv3_cpu() {
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
        unsafe {
            core::ptr::write_volatile((sgi + GICR_IPRIORITYR + id as usize) as *mut u8, 0x80);
        }
    }
    write32(sgi + GICR_ISENABLER0, PER_CPU_INTS);
    unsafe {
        // ICC_SRE_EL1: SRE, DFB, DIB.
        asm!("msr S3_0_C12_C12_5, {v}", "isb", v = in(reg) 0x7u64, options(nostack));
        if current_el() >= 2 {
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

/// Enable SPI `id` in the distributor, level-triggered, priority 0x80,
/// delivered to the BSP (GICv2: CPU interface 0; GICv3: the BSP's affinity).
pub fn gic_enable_spi(id: u32) {
    if !(32..1020).contains(&id) {
        return;
    }
    unsafe {
        // ICFGR: 2 bits per interrupt, 0b00 = level-sensitive.
        let cfg = gicd() + GICD_ICFGR + (id as usize / 16) * 4;
        let shift = (id % 16) * 2;
        write32(cfg, read32(cfg) & !(0b11 << shift));
        core::ptr::write_volatile((gicd() + GICD_IPRIORITYR + id as usize) as *mut u8, 0x80);
        if gic_v3() {
            let group = gicd() + GICD_IGROUPR + (id as usize / 32) * 4;
            write32(group, read32(group) | (1 << (id % 32)));
            write64(gicd() + GICD_IROUTER + id as usize * 8, BSP_AFFINITY.load(Ordering::Relaxed));
        } else {
            core::ptr::write_volatile((gicd() + 0x800 + id as usize) as *mut u8, 0x01);
        }
        write32(gicd() + GICD_ISENABLER + (id as usize / 32) * 4, 1 << (id % 32));
        asm!("dsb sy", options(nostack));
    }
}

/// Acknowledge the pending interrupt: its INTID (1020.. special).
fn ack() -> u32 {
    if gic_v3() {
        let iar: u64;
        unsafe {
            asm!("mrs {i}, S3_0_C12_C12_0", i = out(reg) iar, options(nomem, nostack)); // ICC_IAR1_EL1
        }
        return (iar & 0xFF_FFFF) as u32;
    }
    read32(gicc() + 0x0C) & 0x3FF
}

fn eoi(id: u32) {
    if gic_v3() {
        unsafe {
            asm!("msr S3_0_C12_C12_1, {i}", i = in(reg) u64::from(id), options(nostack)); // ICC_EOIR1_EL1
        }
        return;
    }
    write32(gicc() + 0x10, id);
}

fn cnt_freq() -> u64 {
    let freq: u64;
    unsafe {
        asm!("mrs {f}, cntfrq_el0", f = out(reg) freq, options(nomem, nostack, preserves_flags));
    }
    freq.max(1)
}

fn cnt_now() -> u64 {
    let cnt: u64;
    unsafe {
        asm!("isb", "mrs {c}, cntvct_el0", c = out(reg) cnt, options(nomem, nostack));
    }
    cnt
}

/// Counter ticks per scheduler tick (100 Hz).
fn timer_ticks() -> u64 {
    (cnt_freq() / 100).max(1)
}

/// A monotonic-ns deadline as a counter value (same clock as
/// `clock::monotonic_ns`).
fn ns_to_ticks(ns: u64) -> u64 {
    ((ns as u128 * cnt_freq() as u128) / 1_000_000_000) as u64
}

/// Arm this CPU's virtual timer for the next tick, or for the earliest
/// sleep deadline (`task::next_deadline_ns`) when that is sooner, so a
/// `nanosleep` / `poll` timeout ends when it should and not at the next
/// 10 ms boundary. The physical timer stays periodic.
fn arm_virt_timer() {
    let now = cnt_now();
    let mut target = now + timer_ticks();
    let deadline = crate::task::next_deadline_ns();
    if deadline != u64::MAX {
        target = target.min(ns_to_ticks(deadline).max(now + 1));
    }
    unsafe {
        asm!("msr cntv_cval_el0, {t}", t = in(reg) target, options(nomem, nostack));
    }
}

/// A new sleep deadline on this CPU: pull the virtual timer in if it is
/// armed later than that.
pub fn timer_deadline(deadline_ns: u64) {
    let target = ns_to_ticks(deadline_ns);
    let armed: u64;
    unsafe {
        asm!("mrs {c}, cntv_cval_el0", c = out(reg) armed, options(nomem, nostack));
    }
    if target < armed {
        let target = target.max(cnt_now() + 1);
        unsafe {
            asm!("msr cntv_cval_el0, {t}", t = in(reg) target, options(nomem, nostack));
        }
    }
}

/// The CPUs whose tick [`timer_idle`] stopped.
static TICK_STOPPED: [AtomicBool; crate::smp::MAX_CPUS] = [const { AtomicBool::new(false) }; crate::smp::MAX_CPUS];

/// This CPU halts with nothing to run: stop its periodic (physical) timer
/// and fire the virtual one once, at `wake_ns` (monotonic). Interrupts off.
pub fn timer_idle(wake_ns: u64) {
    let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
    let target = ns_to_ticks(wake_ns).max(cnt_now() + 1);
    TICK_STOPPED[cpu].store(true, Ordering::Relaxed);
    unsafe {
        asm!("msr cntp_ctl_el0, {c}", c = in(reg) 0u64, options(nomem, nostack));
        asm!("msr cntv_cval_el0, {t}", t = in(reg) target, options(nomem, nostack));
        asm!("isb", options(nostack));
    }
}

/// The periodic tick back on this CPU if [`timer_idle`] stopped it.
/// Interrupts off.
pub fn timer_resume() {
    let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
    if !TICK_STOPPED[cpu].swap(false, Ordering::Relaxed) {
        return;
    }
    rearm_timers();
    unsafe {
        asm!("msr cntp_ctl_el0, {c}", c = in(reg) 1u64, options(nomem, nostack));
        asm!("isb", options(nostack));
    }
}

fn init_timer() {
    let ticks = timer_ticks();
    arm_virt_timer();
    unsafe {
        asm!("msr cntv_ctl_el0, {c}", c = in(reg) 1u64, options(nomem, nostack));
        asm!("msr cntp_tval_el0, {t}", t = in(reg) ticks, options(nomem, nostack));
        asm!("msr cntp_ctl_el0, {c}", c = in(reg) 1u64, options(nomem, nostack));
        asm!("isb", options(nostack));
    }
}

/// Re-arm both timers instead of disabling them.
fn rearm_timers() {
    let ticks = timer_ticks();
    arm_virt_timer();
    unsafe {
        asm!("msr cntp_tval_el0, {t}", t = in(reg) ticks, options(nomem, nostack));
    }
}

#[unsafe(no_mangle)]
extern "C" fn aarch64_irq_handler(spsr: u64) {
    let id = ack();
    let timer = id == PPI_EL1_VIRT || id == PPI_EL1_PHYS;
    let tlb = id == SGI_TLB;
    let resched = id == SGI_RESCHED;
    if timer {
        TIMER_FIRED.store(true, Ordering::SeqCst);
        crate::time::note_tick();
        crate::rng::stir_tick();
        rearm_timers();
        // BSP stages PL011 RX so a blocked console reader is woken instead
        // of polling the UART itself (see x86 timer).
        if crate::smp::cpu_id() == 0 && crate::input::drain_uart_irq() {
            crate::task::wake(crate::task::KEY_CONSOLE);
        }
        crate::task::timer_tick();
    }
    if tlb {
        flush_tlb_local();
        crate::smp::tlb_ipi_ack();
    }
    // Shared peripheral interrupts (PCI INTx lines and the like): second-level
    // dispatch before EOI so a level-triggered line is deasserted first.
    if id >= 32 && id < 1020 {
        crate::irq::dispatch(id);
    }
    if id < 1020 {
        eoi(id);
    }
    if timer || resched {
        crate::task::schedule();
        // SPSR.M = EL0t: the interrupt came from user mode.
        if spsr & 0xf == 0 {
            crate::signal::on_user_preempted();
        }
    }
}

fn flush_tlb_local() {
    unsafe {
        asm!("dsb ishst", options(nostack));
        if current_el() >= 2 {
            asm!("tlbi alle2is", options(nostack));
            asm!("tlbi vmalle1is", options(nostack));
        } else {
            asm!("tlbi vmalle1is", options(nostack));
        }
        asm!("dsb ish; isb", options(nostack));
    }
}

fn send_sgi(id: u32) {
    if gic_v3() {
        // ICC_SGI1R_EL1: IRM = all except self.
        sgi1r((1 << 40) | (u64::from(id & 0xf) << 24));
        return;
    }
    // GICD_SGIR: target filter = all except self (bits 25:24 = 01).
    write32(gicd() + 0xF00, (0b01 << 24) | (id & 0xf));
}

fn sgi1r(value: u64) {
    unsafe {
        asm!("dsb ishst", "msr S3_0_C12_C11_5, {v}", "isb", v = in(reg) value, options(nostack));
    }
}

pub fn ipi_tlb_shootdown() {
    send_sgi(SGI_TLB);
}

pub fn ipi_reschedule() {
    send_sgi(SGI_RESCHED);
}

/// Reschedule SGI to one logical CPU. GICv3: the target's affinity in
/// `ICC_SGI1R_EL1` (Aff0 as a target list bit). GICv2: CPU interface `n` is
/// the CPU with MPIDR Aff0 = `n` on QEMU virt (CPUTargetList bit `n`).
pub fn ipi_reschedule_cpu(cpu: usize) {
    if !crate::smp::cpu_online(cpu) {
        return;
    }
    let mpidr = crate::smp::cpu_hw_id(cpu);
    if gic_v3() {
        let aff0 = mpidr & 0xff;
        if aff0 >= 16 {
            send_sgi(SGI_RESCHED);
            return;
        }
        sgi1r(
            (u64::from(SGI_RESCHED) << 24)
                | (1 << aff0)
                | (((mpidr >> 8) & 0xff) << 16)
                | (((mpidr >> 16) & 0xff) << 32)
                | (((mpidr >> 32) & 0xff) << 48),
        );
        return;
    }
    let target = (mpidr & 0xff) as u32;
    if target >= 8 {
        send_sgi(SGI_RESCHED);
        return;
    }
    unsafe {
        asm!("dsb ishst", options(nostack));
    }
    write32(gicd() + 0xF00, (1 << (16 + target)) | (SGI_RESCHED & 0xf));
}

#[unsafe(no_mangle)]
extern "C" fn aarch64_lower_sync(frame: *mut u64) {
    let esr: u64;
    let far: u64;
    unsafe {
        // ESR/FAR are not VHE-aliased: exception to EL2 writes ESR_EL2/FAR_EL2.
        if current_el() >= 2 {
            asm!("mrs {esr}, esr_el2", esr = out(reg) esr, options(nomem, nostack));
            asm!("mrs {far}, far_el2", far = out(reg) far, options(nomem, nostack));
        } else {
            asm!("mrs {esr}, esr_el1", esr = out(reg) esr, options(nomem, nostack));
            asm!("mrs {far}, far_el1", far = out(reg) far, options(nomem, nostack));
        }
    }
    let ec = (esr >> 26) & 0x3f;
    // Stacked by lower_sync: elr/spsr at 16*16, sp_el0 at 16*17.
    let elr = unsafe { *frame.add(32) };
    let sp_el0 = unsafe { *frame.add(34) };
    // EC 0x18: trapped AArch64 SYS/MRS (not PSTATE.IL). TinyCC __clear_cache
    // does `dc cvau` / `ic ivau` at EL0; if SCTLR.UCI is still 0, skip the
    // insn. Kernel mprotect already cleaned D-cache and invalidated I-cache.
    // ISS layout matches QEMU syn_aa64_sysregtrap: CRn at [13:10].
    if ec == 0x18 {
        let iss = esr & 0x1ff_ffff;
        let crn = (iss >> 10) & 0xf;
        if iss != 0 && crn == 7 {
            unsafe {
                *frame.add(32) = elr.wrapping_add(4);
            }
            return;
        }
    }
    if ec == 0x15 {
        unsafe {
            // Keep IRQs masked for the syscall body (x86 syscall_entry does cli).
            core::arch::asm!("msr daifset, #0xf", options(nostack));
            let nr = *frame.add(8) as usize;
            let a0 = *frame.add(0) as usize;
            let a1 = *frame.add(1) as usize;
            let a2 = *frame.add(2) as usize;
            crate::user::set_syscall_frame(frame);
            let ret = crate::user::syscall_dispatch(
                nr,
                a0,
                a1,
                a2,
                elr as usize,
                sp_el0 as usize,
                frame,
            );
            crate::user::set_syscall_frame(core::ptr::null_mut());
            *frame.add(0) = ret as u64;
        }
        return;
    }
    // EC 0x20/0x24: insn/data abort from EL0. Kill the faulting task (SIGSEGV
    // convention) instead of halting QEMU — a userspace null deref during
    // `cat | cat` must not be a machine-wide `[ FAIL ] exception`.
    if ec == 0x20 || ec == 0x24 {
        // A translation fault (status 0b0001xx) on an mmap page not touched
        // yet: page it in and retry the instruction.
        let translation = esr & 0x3c == 0x04;
        let access = if ec == 0x20 {
            crate::user::Access::Exec
        } else if esr & (1 << 6) != 0 {
            crate::user::Access::Write
        } else {
            crate::user::Access::Read
        };
        if translation && crate::user::fault_in(far as usize, access) {
            return;
        }
        crate::exception::user_fault_kill(
            if ec == 0x20 { "insn abort" } else { "data abort" },
            &alloc::format!("ec={ec:#x} esr={esr:#x} elr={elr:#x} far={far:#x} sp_el0={sp_el0:#x}"),
        );
    }
    // Any other synchronous exception from EL0 (an undefined instruction,
    // EC 0x00; a `brk`, EC 0x3c; a trapped FP exception, ...): this is the
    // lower-EL handler, so it always came from userspace — kill the task
    // instead of halting the machine (`aarch64_sync_abort` is fatal).
    crate::exception::user_fault_kill(
        "sync exception",
        &alloc::format!("ec={ec:#x} esr={esr:#x} elr={elr:#x} far={far:#x} sp_el0={sp_el0:#x}"),
    );
}

fn read_esr_elr_far() -> (u64, u64, u64) {
    let esr: u64;
    let elr: u64;
    let far: u64;
    unsafe {
        if current_el() >= 2 {
            asm!("mrs {esr}, esr_el2", esr = out(reg) esr, options(nomem, nostack));
            asm!("mrs {elr}, elr_el2", elr = out(reg) elr, options(nomem, nostack));
            asm!("mrs {far}, far_el2", far = out(reg) far, options(nomem, nostack));
        } else {
            asm!("mrs {esr}, esr_el1", esr = out(reg) esr, options(nomem, nostack));
            asm!("mrs {elr}, elr_el1", elr = out(reg) elr, options(nomem, nostack));
            asm!("mrs {far}, far_el1", far = out(reg) far, options(nomem, nostack));
        }
    }
    (esr, elr, far)
}

/// The x30 and sp at a kernel fault, from the 16 * 4 frame `sync_el` and
/// `exception_unhandled` push (x30 at [sp, #16 * 2]): with `elr` at 0 or in
/// the weeds, the link register still names the caller of the bad `blr`
/// (a `ret` to 0 leaves it 0), and sp says which kernel stack it ran on.
fn kernel_frame_lr_sp(frame: *const u64) -> (u64, u64) {
    let lr = unsafe { frame.add(4).read() };
    (lr, frame as u64 + 16 * 4)
}

#[unsafe(no_mangle)]
extern "C" fn aarch64_sync_handler(frame: *const u64) -> ! {
    let (esr, elr, far) = read_esr_elr_far();
    let lr_sp = kernel_frame_lr_sp(frame);
    super::exception::aarch64_sync_abort("kernel sync abort", esr, elr, far, None, Some(lr_sp));
}

#[unsafe(no_mangle)]
extern "C" fn aarch64_unhandled_exception(frame: *const u64) -> ! {
    let (esr, elr, far) = read_esr_elr_far();
    let lr_sp = kernel_frame_lr_sp(frame);
    super::exception::aarch64_sync_abort("unhandled exception", esr, elr, far, None, Some(lr_sp));
}

fn read32(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

fn read64(addr: usize) -> u64 {
    unsafe { core::ptr::read_volatile(addr as *const u64) }
}

fn write64(addr: usize, value: u64) {
    unsafe { core::ptr::write_volatile(addr as *mut u64, value) }
}

fn write32(addr: usize, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}
