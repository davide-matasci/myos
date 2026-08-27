//! Exception vectors, GICv2, and the generic timers.
//!
//! Limine (base rev 6) enters with PSTATE.SP=0 (SP_EL0), either at EL1 or at
//! EL2 with VHE (`HCR_EL2.{E2H,TGE}`). IRQs taken with SPSel=0 use the
//! Current-EL SP0 slot, not SP_ELx; the handler has to live in both.
//!
//! At EL2, CNTHCTL_EL2 is in the CNTKCTL layout and Limine only sets bits 0-1
//! (EL0 access). The EL1 physical timer (CNTP / PPI 30) is the wrong one;
//! also arm CNTV (PPI 27) and, at EL2, CNTHP (PPI 26) and CNTHV (PPI 28).

use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicBool, Ordering};

const GICD: usize = 0x0800_0000;
const GICC: usize = 0x0801_0000;
const PPI_EL2_PHYS: u32 = 26; // CNTHP
const PPI_EL1_VIRT: u32 = 27; // CNTV
const PPI_EL2_VIRT: u32 = 28; // CNTHV
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
    b exception_hang
    // Current EL, SP_ELx
    .align 7
    b sync_el
    .align 7
    b irq_el1h
    .align 7
    b irq_el1h
    .align 7
    b exception_hang
    // Lower EL, AArch64
    .align 7
    b exception_hang
    .align 7
    b irq_el1h
    .align 7
    b irq_el1h
    .align 7
    b exception_hang
    // Lower EL, AArch32
    .align 7
    b exception_hang
    .align 7
    b exception_hang
    .align 7
    b exception_hang
    .align 7
    b exception_hang

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
    bl aarch64_irq_handler
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
    b exception_hang

exception_hang:
    b exception_hang
    "#
);

unsafe extern "C" {
    fn exception_vectors();
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
    let enables = (1 << PPI_EL2_PHYS) | (1 << PPI_EL1_VIRT) | (1 << PPI_EL2_VIRT) | (1 << PPI_EL1_PHYS);
    write32(GICD + 0x100, enables);
    for id in [PPI_EL2_PHYS, PPI_EL1_VIRT, PPI_EL2_VIRT, PPI_EL1_PHYS] {
        unsafe {
            core::ptr::write_volatile((GICD + 0x400 + id as usize) as *mut u8, 0x80);
        }
    }
}

fn cntfrq() -> u64 {
    let freq: u64;
    unsafe {
        asm!("mrs {f}, cntfrq_el0", f = out(reg) freq, options(nomem, nostack, preserves_flags));
    }
    freq
}

fn init_timer() {
    let ticks = (cntfrq() / 100).max(1);
    unsafe {
        // EL1 virtual + physical (PPI 27 / 30). Harmless extras if EL2 takes another PPI.
        asm!("msr cntv_tval_el0, {t}", t = in(reg) ticks, options(nomem, nostack));
        asm!("msr cntv_ctl_el0, {c}", c = in(reg) 1u64, options(nomem, nostack));
        asm!("msr cntp_tval_el0, {t}", t = in(reg) ticks, options(nomem, nostack));
        asm!("msr cntp_ctl_el0, {c}", c = in(reg) 1u64, options(nomem, nostack));
    }
    if current_el() >= 2 {
        unsafe {
            // VHE CNTHCTL uses the CNTKCTL layout; bits 10-11 enable EL1 physical timer.
            let mut h: u64;
            asm!("mrs {h}, cnthctl_el2", h = out(reg) h, options(nomem, nostack, preserves_flags));
            h |= (1 << 10) | (1 << 11);
            asm!("msr cnthctl_el2, {h}", h = in(reg) h, options(nomem, nostack));
            asm!("msr cnthp_tval_el2, {t}", t = in(reg) ticks, options(nomem, nostack));
            asm!("msr cnthp_ctl_el2, {c}", c = in(reg) 1u64, options(nomem, nostack));
            asm!("msr cnthv_tval_el2, {t}", t = in(reg) ticks, options(nomem, nostack));
            asm!("msr cnthv_ctl_el2, {c}", c = in(reg) 1u64, options(nomem, nostack));
        }
    }
    unsafe {
        asm!("isb", options(nomem, nostack));
    }
}

fn disable_timers() {
    unsafe {
        asm!("msr cntp_ctl_el0, {c}", c = in(reg) 0u64, options(nomem, nostack));
        asm!("msr cntv_ctl_el0, {c}", c = in(reg) 0u64, options(nomem, nostack));
    }
    if current_el() >= 2 {
        unsafe {
            asm!("msr cnthp_ctl_el2, {c}", c = in(reg) 0u64, options(nomem, nostack));
            asm!("msr cnthv_ctl_el2, {c}", c = in(reg) 0u64, options(nomem, nostack));
        }
    }
}

fn is_timer_ppi(id: u32) -> bool {
    id == PPI_EL2_PHYS || id == PPI_EL1_VIRT || id == PPI_EL2_VIRT || id == PPI_EL1_PHYS
}

#[unsafe(no_mangle)]
extern "C" fn aarch64_irq_handler() {
    let iar = read32(GICC + 0x0C);
    let id = iar & 0x3FF;
    if is_timer_ppi(id) {
        TIMER_FIRED.store(true, Ordering::SeqCst);
        disable_timers();
    }
    if id < 1020 {
        write32(GICC + 0x10, iar);
    }
}

#[unsafe(no_mangle)]
extern "C" fn aarch64_sync_handler() -> ! {
    let esr: u64;
    let elr: u64;
    let far: u64;
    unsafe {
        asm!("mrs {esr}, esr_el1", esr = out(reg) esr, options(nomem, nostack));
        asm!("mrs {elr}, elr_el1", elr = out(reg) elr, options(nomem, nostack));
        asm!("mrs {far}, far_el1", far = out(reg) far, options(nomem, nostack));
    }
    panic!("sync abort esr={esr:#x} elr={elr:#x} far={far:#x}");
}

fn read32(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

fn write32(addr: usize, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}
