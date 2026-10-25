//! Exception vectors, the interrupt entry and the generic timers. The
//! interrupt controller is `irqchip::root()` (`gic`, `bcm2836`): every one
//! numbers its interrupts as the GIC does, which the entry reads (SGI 0
//! and 1 the IPIs, PPI 27 and 30 the timers, 32 and up the devices).
//!
//! Limine (base rev 6) enters with PSTATE.SP=0 (SP_EL0), either at EL1 or at
//! EL2 with VHE (`HCR_EL2.{E2H,TGE}`). IRQs taken with SPSel=0 use the
//! Current-EL SP0 slot, not SP_ELx; the handler has to live in both.
//!
//! CI #47: CNTP (PPI 30) plus SP0 vectors prints int ok. CI #50: LLVM on
//! nightly-2026-07-26 rejects `cnthv_*_el2`, so stay on EL0 timer registers.

use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicBool, Ordering};

pub(super) const PPI_EL1_VIRT: u32 = 27; // CNTV
pub(super) const PPI_EL1_PHYS: u32 = 30; // CNTP
pub(super) const SGI_TLB: u32 = 0;
pub(super) const SGI_RESCHED: u32 = 1;

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
    crate::console::status_info(crate::irqchip::root().name());
    crate::irqchip::root().init_cpu();
    init_timer();
    unsafe {
        asm!("dsb sy", options(nostack));
        asm!("msr daifclr, #3", options(nostack)); // unmask IRQ+FIQ
    }
}

/// Secondary CPU: vectors already set globally; this CPU's interrupt
/// controller side and timers.
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
    // This CPU's side of the interrupt controller (a GICv2's banked
    // registers, a GICv3's redistributor, ...): its IPIs and timers.
    crate::irqchip::root().init_cpu();
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
    let chip = crate::irqchip::root();
    let Some(id) = chip.claim() else {
        return;
    };
    let timer = id == PPI_EL1_VIRT || id == PPI_EL1_PHYS;
    let tlb = id == SGI_TLB;
    let resched = id == SGI_RESCHED;
    if timer {
        TIMER_FIRED.store(true, Ordering::SeqCst);
        crate::time::note_tick();
        crate::rng::stir_tick();
        rearm_timers();
        // The BSP stages UART input when the UART has no interrupt.
        if crate::smp::cpu_id() == 0 {
            crate::input::tick();
        }
        crate::task::timer_tick();
    }
    if tlb {
        flush_tlb_local();
        crate::smp::tlb_ipi_ack();
    }
    // Shared peripheral interrupts (PCI INTx lines and the like): second-level
    // dispatch before EOI so a level-triggered line is deasserted first.
    if id >= 32 {
        crate::irq::dispatch(id);
    }
    chip.complete(id);
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
    // EC 0x20/0x24: insn/data abort from EL0: SIGSEGV (SIGBUS for an
    // alignment fault), or the task ends instead of halting QEMU: a
    // userspace null deref during `cat | cat` must not be a machine-wide
    // `[ FAIL ] exception`.
    use crate::signal::{code::*, *};
    let mut regs = crate::user::SyscallRegs::from_ptr(frame);
    let mut raise = |sig, code, addr: u64| on_fault(&mut regs, Info { sig, code, addr: addr as usize });
    if ec == 0x20 || ec == 0x24 {
        // A translation fault (status 0b0001xx) on an mmap page not touched
        // yet: page it in and retry the instruction. A permission fault
        // (0b0011xx): on a store, a page the page cache or a fork shares
        // read-only, copied for the process; on a fetch or a load, a stale
        // translation, or a page whose region was made executable after it
        // was mapped (`fault_in` says when it is a real one).
        let translation = esr & 0x3c == 0x04;
        let permission = esr & 0x3c == 0x0c;
        let access = if ec == 0x20 {
            crate::user::Access::Exec
        } else if esr & (1 << 6) != 0 {
            crate::user::Access::Write
        } else {
            crate::user::Access::Read
        };
        if (translation || permission) && crate::user::fault_in(far as usize, access) {
            return;
        }
        let (sig, si) = match esr & 0x3f {
            0x21 => (SIGBUS, BUS_ADRALN),
            _ => (SIGSEGV, segv_code(far as usize, !translation)),
        };
        if raise(sig, si, far) {
            return;
        }
        crate::exception::user_fault_kill(
            sig,
            if ec == 0x20 { "insn abort" } else { "data abort" },
            elr,
            &alloc::format!("ec={ec:#x} esr={esr:#x} elr={elr:#x} far={far:#x} sp_el0={sp_el0:#x}"),
        );
    }
    // Any other synchronous exception from EL0: an undefined instruction
    // (EC 0x00), a misaligned PC or SP (0x22, 0x26), a trapped FP exception
    // (0x2c), a `brk` (0x3c), ... This is the lower-EL handler, so it always
    // came from userspace: the signal, or the task ends instead of halting
    // the machine (`aarch64_sync_abort` is fatal).
    let (sig, si, addr) = match ec {
        0x22 => (SIGBUS, BUS_ADRALN, elr),
        0x26 => (SIGBUS, BUS_ADRALN, sp_el0),
        0x2c => (SIGFPE, fpe_code(esr), elr),
        0x30 | 0x32 | 0x34 | 0x3c => (SIGTRAP, TRAP_BRKPT, elr),
        _ => (SIGILL, ILL_ILLOPC, elr),
    };
    if raise(sig, si, addr) {
        return;
    }
    crate::exception::user_fault_kill(
        sig,
        "sync exception",
        elr,
        &alloc::format!("ec={ec:#x} esr={esr:#x} elr={elr:#x} far={far:#x} sp_el0={sp_el0:#x}"),
    );
}

/// The `si_code` of a trapped FP exception (EC 0x2c): its ISS flags IOF,
/// DZF, OFF, UFF, IXF (bits 0-4) and IDF (7), the first in Linux's order.
fn fpe_code(esr: u64) -> u32 {
    use crate::signal::code::*;
    if esr & 0x01 != 0 {
        FPE_FLTINV
    } else if esr & 0x02 != 0 {
        FPE_FLTDIV
    } else if esr & 0x04 != 0 {
        FPE_FLTOVF
    } else if esr & 0x88 != 0 {
        FPE_FLTUND
    } else if esr & 0x10 != 0 {
        FPE_FLTRES
    } else {
        0
    }
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
