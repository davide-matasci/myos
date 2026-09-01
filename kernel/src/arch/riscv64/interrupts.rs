//! Supervisor trap vector, S-mode timer (`time`/`stimecmp`), and user `ecall`.
//!
//! Limine enters in S-mode with Sv39 on. QEMU `rv64` advertises `sstc`, so the
//! supervisor timer compare CSR is used instead of CLINT MMIO (not identity-mapped
//! by Limine base revision 3+).

use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicBool, Ordering};

static TIMER_FIRED: AtomicBool = AtomicBool::new(false);

// Frame: x0..x31 at 0..31, sepc at 32, sstatus at 33, user sp at 34.
const FRAME_WORDS: usize = 35;

global_asm!(
    r#"
    .align 4
    .global trap_vector
trap_vector:
    csrr t0, sstatus
    andi t0, t0, 0x100
    bnez t0, 1f
    csrrw sp, sscratch, sp
1:
    addi sp, sp, -280
    sd x0, 0(sp)
    sd x1, 8(sp)
    sd x2, 16(sp)
    sd x3, 24(sp)
    sd x4, 32(sp)
    sd x5, 40(sp)
    sd x6, 48(sp)
    sd x7, 56(sp)
    sd x8, 64(sp)
    sd x9, 72(sp)
    sd x10, 80(sp)
    sd x11, 88(sp)
    sd x12, 96(sp)
    sd x13, 104(sp)
    sd x14, 112(sp)
    sd x15, 120(sp)
    sd x16, 128(sp)
    sd x17, 136(sp)
    sd x18, 144(sp)
    sd x19, 152(sp)
    sd x20, 160(sp)
    sd x21, 168(sp)
    sd x22, 176(sp)
    sd x23, 184(sp)
    sd x24, 192(sp)
    sd x25, 200(sp)
    sd x26, 208(sp)
    sd x27, 216(sp)
    sd x28, 224(sp)
    sd x29, 232(sp)
    sd x30, 240(sp)
    sd x31, 248(sp)
    csrr t0, sepc
    sd t0, 256(sp)
    csrr t0, sstatus
    sd t0, 264(sp)
    csrr t1, sstatus
    andi t1, t1, 0x100
    bnez t1, 2f
    csrr t0, sscratch
    sd t0, 272(sp)
    j 3f
2:
    sd zero, 272(sp)
3:
    mv a0, sp
    call riscv64_trap_handler
    ld t0, 272(sp)
    csrw sscratch, t0
    ld t0, 264(sp)
    csrw sstatus, t0
    ld t0, 256(sp)
    csrw sepc, t0
    ld x0, 0(sp)
    ld x1, 8(sp)
    ld x2, 16(sp)
    ld x3, 24(sp)
    ld x4, 32(sp)
    ld x5, 40(sp)
    ld x6, 48(sp)
    ld x7, 56(sp)
    ld x8, 64(sp)
    ld x9, 72(sp)
    ld x10, 80(sp)
    ld x11, 88(sp)
    ld x12, 96(sp)
    ld x13, 104(sp)
    ld x14, 112(sp)
    ld x15, 120(sp)
    ld x16, 128(sp)
    ld x17, 136(sp)
    ld x18, 144(sp)
    ld x19, 152(sp)
    ld x20, 160(sp)
    ld x21, 168(sp)
    ld x22, 176(sp)
    ld x23, 184(sp)
    ld x24, 192(sp)
    ld x25, 200(sp)
    ld x26, 208(sp)
    ld x27, 216(sp)
    ld x28, 224(sp)
    ld x29, 232(sp)
    ld x30, 240(sp)
    ld x31, 248(sp)
    addi sp, sp, 280
    csrr t0, sstatus
    andi t0, t0, 0x100
    bnez t0, 4f
    csrrw sp, sscratch, sp
4:
    sret

    .global fork_sret_from_frame
fork_sret_from_frame:
    mv sp, a0
    ld t0, 272(sp)
    csrw sscratch, t0
    ld t0, 264(sp)
    csrw sstatus, t0
    ld t0, 256(sp)
    csrw sepc, t0
    ld x0, 0(sp)
    ld x1, 8(sp)
    ld x2, 16(sp)
    ld x3, 24(sp)
    ld x4, 32(sp)
    ld x5, 40(sp)
    ld x6, 48(sp)
    ld x7, 56(sp)
    ld x8, 64(sp)
    ld x9, 72(sp)
    ld x10, 80(sp)
    ld x11, 88(sp)
    ld x12, 96(sp)
    ld x13, 104(sp)
    ld x14, 112(sp)
    ld x15, 120(sp)
    ld x16, 128(sp)
    ld x17, 136(sp)
    ld x18, 144(sp)
    ld x19, 152(sp)
    ld x20, 160(sp)
    ld x21, 168(sp)
    ld x22, 176(sp)
    ld x23, 184(sp)
    ld x24, 192(sp)
    ld x25, 200(sp)
    ld x26, 208(sp)
    ld x27, 216(sp)
    ld x28, 224(sp)
    ld x29, 232(sp)
    ld x30, 240(sp)
    ld x31, 248(sp)
    addi sp, sp, 280
    csrr t0, sstatus
    andi t0, t0, 0x100
    bnez t0, 5f
    csrrw sp, sscratch, sp
5:
    sret

    .global fork_sret_child_from_frame
fork_sret_child_from_frame:
    # a0 -> copied trap frame (not live stack). Caller must leave sscratch as
    # this task's kernel stack top (same invariant as enter_riscv64).
    mv t6, a0
    ld t0, 264(t6)
    csrw sstatus, t0
    ld t0, 256(t6)
    csrw sepc, t0
    ld x1, 8(t6)
    ld x3, 24(t6)
    ld x4, 32(t6)
    ld x5, 40(t6)
    ld x6, 48(t6)
    ld x7, 56(t6)
    ld x8, 64(t6)
    ld x9, 72(t6)
    ld x11, 88(t6)
    ld x12, 96(t6)
    ld x13, 104(t6)
    ld x14, 112(t6)
    ld x15, 120(t6)
    ld x16, 128(t6)
    ld x17, 136(t6)
    ld x18, 144(t6)
    ld x19, 152(t6)
    ld x20, 160(t6)
    ld x21, 168(t6)
    ld x22, 176(t6)
    ld x23, 184(t6)
    ld x24, 192(t6)
    ld x25, 200(t6)
    ld x26, 208(t6)
    ld x27, 216(t6)
    ld x28, 224(t6)
    ld x29, 232(t6)
    ld x30, 240(t6)
    # t6 is x31: capture user sp before restoring x31 clobbers the frame pointer.
    ld t0, 272(t6)
    ld x31, 248(t6)
    mv sp, t0
    li x10, 0
    sret
    "#
);

unsafe extern "C" {
    fn trap_vector();
    fn fork_sret_from_frame(frame: *mut u64) -> !;
    fn fork_sret_child_from_frame(frame: *mut u64) -> !;
}

pub fn fork_sret_to_user(frame: *mut u64) -> ! {
    unsafe { fork_sret_from_frame(frame) }
}

pub fn fork_sret_child_to_user(frame: *mut u64) -> ! {
    unsafe { fork_sret_child_from_frame(frame) }
}

pub fn init() {
    let v = trap_vector as *const () as usize;
    unsafe {
        asm!("csrw stvec, {v}", v = in(reg) v, options(nostack));
        asm!("csrs sie, {}", in(reg) 1 << 5, options(nostack)); // STIE
        asm!("csrs sstatus, {}", in(reg) 1 << 1, options(nostack)); // SIE
    }
    init_timer();
}

pub fn wait_for_interrupt_proof() {
    while !TIMER_FIRED.load(Ordering::SeqCst) {
        unsafe {
            asm!("wfi", options(nomem, nostack, preserves_flags));
        }
    }
}

fn read_time() -> u64 {
    let t: u64;
    unsafe {
        asm!("csrr {t}, time", t = out(reg) t, options(nomem, nostack));
    }
    t
}

fn write_stimecmp(val: u64) {
    unsafe {
        asm!("csrw stimecmp, {v}", v = in(reg) val, options(nomem, nostack));
    }
}

fn timer_interval() -> u64 {
    1_000_000 // ~10 ms at 10 MHz `time` clock on QEMU virt
}

fn init_timer() {
    let next = read_time().wrapping_add(timer_interval());
    write_stimecmp(next);
}

fn rearm_timer() {
    let next = read_time().wrapping_add(timer_interval());
    write_stimecmp(next);
}

#[unsafe(no_mangle)]
extern "C" fn riscv64_trap_handler(frame: *mut u64) {
    let scause: u64;
    let stval: u64;
    unsafe {
        asm!("csrr {c}, scause", c = out(reg) scause, options(nomem, nostack));
        asm!("csrr {t}, stval", t = out(reg) stval, options(nomem, nostack));
    }

    if scause >> 63 != 0 {
        // Interrupt
        let code = scause & 0xfff;
        if code == 5 {
            // Supervisor timer
            TIMER_FIRED.store(true, Ordering::SeqCst);
            rearm_timer();
            crate::task::schedule();
        }
        return;
    }

    let code = scause & 0xfff;
    match code {
        8 => {
            // Environment call from U-mode
            unsafe {
                asm!("csrs sstatus, {}", in(reg) 1 << 18, options(nostack)); // SUM
            }
            let sepc = unsafe { *frame.add(32) };
            let user_sp = unsafe { *frame.add(34) };
            let nr = unsafe { *frame.add(17) } as usize;
            let a0 = unsafe { *frame.add(10) } as usize;
            let a1 = unsafe { *frame.add(11) } as usize;
            let a2 = unsafe { *frame.add(12) } as usize;
            unsafe {
                asm!("csrs sstatus, {}", in(reg) 1 << 1, options(nostack)); // SIE during dispatch
                crate::user::set_syscall_frame(frame);
                let ret = crate::user::syscall_dispatch(
                    nr,
                    a0,
                    a1,
                    a2,
                    (sepc + 4) as usize,
                    user_sp as usize,
                );
                crate::user::set_syscall_frame(core::ptr::null_mut());
                *frame.add(10) = ret as u64;
                *frame.add(32) = sepc + 4;
                asm!("csrc sstatus, {}", in(reg) 1 << 18, options(nostack)); // clear SUM
            }
        }
        12 | 13 | 15 => {
            let sepc = unsafe { *frame.add(32) };
            let user_sp = unsafe { *frame.add(34) };
            let kind = match code {
                12 => "instruction page fault",
                13 => "load page fault",
                _ => "store page fault",
            };
            crate::exception::riscv64_page_fault(kind, stval, sepc, user_sp);
        }
        _ => {
            let sepc = unsafe { *frame.add(32) };
            crate::exception::riscv64_trap(code, sepc, stval);
        }
    }
}
