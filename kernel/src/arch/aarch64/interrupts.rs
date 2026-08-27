//! EL1/EL2 exception vectors, GICv2, and the generic virtual timer.
//!
//! Physical CNTP (PPI 30) never fired under Limine on QEMU virt (CI #46).
//! CNTV / PPI 27 is forwarded to whatever EL we actually entered.

use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicBool, Ordering};

const GICD: usize = 0x0800_0000;
const GICC: usize = 0x0801_0000;
const TIMER_INTID: u32 = 27; // PPI 11: virtual timer

static TIMER_FIRED: AtomicBool = AtomicBool::new(false);

global_asm!(
    r#"
    .align 11
    .global exception_vectors
exception_vectors:
    // Current EL, SP_EL0
    .align 7
    b exception_hang
    .align 7
    b exception_hang
    .align 7
    b exception_hang
    .align 7
    b exception_hang
    // Current EL, SP_ELx (this is us)
    .align 7
    b exception_hang
    .align 7
    b irq_el1h
    .align 7
    b exception_hang
    .align 7
    b exception_hang
    // Lower EL, AArch64
    .align 7
    b exception_hang
    .align 7
    b exception_hang
    .align 7
    b exception_hang
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

pub fn init() {
    let v = exception_vectors as *const () as usize;
    unsafe {
        if current_el() >= 2 {
            asm!("msr vbar_el2, {v}", "isb", v = in(reg) v, options(nostack));
        } else {
            asm!("msr vbar_el1, {v}", "isb", v = in(reg) v, options(nostack));
        }
    }
    init_gic();
    init_timer();
    unsafe {
        asm!("msr daifclr, #2", options(nomem, nostack)); // unmask IRQ
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
    write32(GICD, 1); // GICD_CTLR enable Grp0
    write32(GICC, 1); // GICC_CTLR enable
    write32(GICC + 0x004, 0xFF); // PMR: accept all
    write32(GICD + 0x100, 1 << TIMER_INTID);
    unsafe {
        core::ptr::write_volatile((GICD + 0x400 + TIMER_INTID as usize) as *mut u8, 0x80);
    }
}

fn init_timer() {
    let freq: u64;
    unsafe {
        asm!("mrs {f}, cntfrq_el0", f = out(reg) freq, options(nomem, nostack, preserves_flags));
    }
    let ticks = (freq / 100).max(1);
    unsafe {
        asm!("msr cntv_tval_el0, {t}", t = in(reg) ticks, options(nomem, nostack));
        asm!("msr cntv_ctl_el0, {c}", c = in(reg) 1u64, options(nomem, nostack));
        asm!("isb", options(nomem, nostack));
    }
}

#[unsafe(no_mangle)]
extern "C" fn aarch64_irq_handler() {
    let iar = read32(GICC + 0x0C);
    let id = iar & 0x3FF;
    if id == TIMER_INTID {
        TIMER_FIRED.store(true, Ordering::SeqCst);
        unsafe {
            asm!("msr cntv_ctl_el0, {c}", c = in(reg) 0u64, options(nomem, nostack));
        }
    }
    if id < 1020 {
        write32(GICC + 0x10, iar);
    }
}

fn read32(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

fn write32(addr: usize, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}
