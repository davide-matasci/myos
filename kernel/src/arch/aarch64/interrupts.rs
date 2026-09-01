//! Exception vectors, GICv2, and the generic timers.
//!
//! Limine (base rev 6) enters with PSTATE.SP=0 (SP_EL0), either at EL1 or at
//! EL2 with VHE (`HCR_EL2.{E2H,TGE}`). IRQs taken with SPSel=0 use the
//! Current-EL SP0 slot, not SP_ELx; the handler has to live in both.
//!
//! CI #47: CNTP (PPI 30) plus SP0 vectors prints int ok. CI #50: LLVM on
//! nightly-2026-07-26 rejects `cnthv_*_el2`, so stay on EL0 timer registers.

use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicBool, Ordering};

const GICD: usize = 0x0800_0000;
const GICC: usize = 0x0801_0000;
const PPI_EL1_VIRT: u32 = 27; // CNTV
const PPI_EL1_PHYS: u32 = 30; // CNTP

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
    bl aarch64_sync_handler

exception_unhandled:
    sub sp, sp, #(16 * 4)
    stp x0, x1, [sp, #16 * 0]
    stp x2, x3, [sp, #16 * 1]
    str x30, [sp, #16 * 2]
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
            options(nomem),
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
    }
    init_gic();
    init_timer();
    unsafe {
        asm!("dsb sy", options(nomem, nostack));
        asm!("msr daifclr, #3", options(nomem, nostack)); // unmask IRQ+FIQ
    }
}

pub fn wait_for_interrupt_proof() {
    while !TIMER_FIRED.load(Ordering::SeqCst) {
        unsafe {
            asm!("wfi", options(nomem, nostack, preserves_flags));
        }
    }
}

fn init_gic() {
    write32(GICD, 3); // GICD_CTLR enable group 0+1
    write32(GICC, 3); // GICC_CTLR enable group 0+1
    write32(GICC + 0x004, 0xFF); // PMR: accept all
    write32(GICD + 0x100, (1 << PPI_EL1_VIRT) | (1 << PPI_EL1_PHYS));
    for id in [PPI_EL1_VIRT, PPI_EL1_PHYS] {
        unsafe {
            core::ptr::write_volatile((GICD + 0x400 + id as usize) as *mut u8, 0x80);
        }
    }
}

fn timer_ticks() -> u64 {
    let freq: u64;
    unsafe {
        asm!("mrs {f}, cntfrq_el0", f = out(reg) freq, options(nomem, nostack, preserves_flags));
    }
    (freq / 100).max(1)
}

fn init_timer() {
    let ticks = timer_ticks();
    unsafe {
        asm!("msr cntv_tval_el0, {t}", t = in(reg) ticks, options(nomem, nostack));
        asm!("msr cntv_ctl_el0, {c}", c = in(reg) 1u64, options(nomem, nostack));
        asm!("msr cntp_tval_el0, {t}", t = in(reg) ticks, options(nomem, nostack));
        asm!("msr cntp_ctl_el0, {c}", c = in(reg) 1u64, options(nomem, nostack));
        asm!("isb", options(nomem, nostack));
    }
}

/// Reload the same interval used at init instead of disabling the timers.
fn rearm_timers() {
    let ticks = timer_ticks();
    unsafe {
        asm!("msr cntv_tval_el0, {t}", t = in(reg) ticks, options(nomem, nostack));
        asm!("msr cntp_tval_el0, {t}", t = in(reg) ticks, options(nomem, nostack));
    }
}

#[unsafe(no_mangle)]
extern "C" fn aarch64_irq_handler() {
    let iar = read32(GICC + 0x0C);
    let id = iar & 0x3FF;
    let timer = id == PPI_EL1_VIRT || id == PPI_EL1_PHYS;
    if timer {
        TIMER_FIRED.store(true, Ordering::SeqCst);
        rearm_timers();
    }
    if id < 1020 {
        write32(GICC + 0x10, iar);
    }
    if timer {
        crate::task::schedule();
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
            core::arch::asm!("msr daifset, #0xf", options(nomem, nostack));
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
            );
            crate::user::set_syscall_frame(core::ptr::null_mut());
            *frame.add(0) = ret as u64;
        }
        return;
    }
    crate::exception::aarch64_sync_abort("user sync abort", esr, elr, far, Some(sp_el0));
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

#[unsafe(no_mangle)]
extern "C" fn aarch64_sync_handler() -> ! {
    let (esr, elr, far) = read_esr_elr_far();
    crate::exception::aarch64_sync_abort("kernel sync abort", esr, elr, far, None);
}

#[unsafe(no_mangle)]
extern "C" fn aarch64_unhandled_exception() -> ! {
    let (esr, elr, far) = read_esr_elr_far();
    crate::exception::aarch64_sync_abort("unhandled exception", esr, elr, far, None);
}

fn read32(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

fn write32(addr: usize, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}
