//! Supervisor trap vector, S-mode timer (`time`/`stimecmp`), and user `ecall`.
//!
//! Limine enters in S-mode with Sv39 on. QEMU `rv64` advertises `sstc`, so the
//! supervisor timer compare CSR is used instead of CLINT MMIO (not identity-mapped
//! by Limine base revision 3+).

use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicBool, Ordering};

static TIMER_FIRED: AtomicBool = AtomicBool::new(false);

// Trap frame (u64 words): x0..x31 at 0..31, sepc at 32, sstatus at 33,
// user sp at 34 — 280 bytes.

global_asm!(
    r#"
    .align 4
    .global trap_vector
trap_vector:
    # sscratch convention (same as Linux): 0 while this hart executes in
    # S-mode, the running task's kernel stack top while it executes in U-mode.
    # That lets entry pick the stack with a single swap and no scratch GPR.
    #
    # The previous entry tested sstatus.SPP with `csrr t0, sstatus` BEFORE
    # saving t0, and the exit repeated the test AFTER restoring it, so every
    # trap from U-mode (timer tick, ecall, IPI) silently zeroed user t0 and
    # every nested S-mode trap set kernel t0 to 0x100. That was the
    # intermittent riscv64 corruption family: runaway copy loops storing at
    # brk (dropbear conv_path), wrong mbedTLS ECDSA results ("ECP - The
    # signature is not valid"), SSH stalls, sepc=0 / zeroed-ra faults.
    csrrw sp, sscratch, sp
    bnez sp, 1f
    # From S-mode: sscratch was 0, undo the swap (sp = kernel sp again).
    csrrw sp, sscratch, sp
1:
    addi sp, sp, -280
    sd x0, 0(sp)
    sd x1, 8(sp)
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
    # Every GPR except sp is saved; t0/t1 are free from here on.
    csrr t0, sscratch    # U-mode: user sp. S-mode: 0.
    bnez t0, 2f
    addi t0, sp, 280     # S-mode: sp at the time of the trap
2:
    sd t0, 16(sp)
    sd t0, 272(sp)
    csrw sscratch, zero  # now running in S-mode
    csrr t0, sepc
    sd t0, 256(sp)
    csrr t0, sstatus
    sd t0, 264(sp)
    # RISC-V ABI: tp (x4) is the user TLS pointer, but smp::cpu_id() reads tp
    # as the logical CPU index (LLVM reserves x4, so kernel code never
    # clobbers it). On a trap from U-mode reload it from the footer word the
    # scheduler stamps at the base of this task's kernel stack
    # (`task::stamp_stack_cpu`): the frame sits at the top of that 64 KiB
    # stack, so the footer is at sp + 280 - STACK_SIZE. Nested S-mode traps
    # keep the hart tp. (The old `mv tp, zero` only worked while userspace
    # was BSP-only; user TLS in tp selected a never-scheduled CURRENT[] slot,
    # ripgrep sepc=0 #146/#150.)
    andi t0, t0, 0x100
    bnez t0, 5f
    li t1, 65536
    sub t1, sp, t1
    ld tp, 280(t1)
5:
    mv a0, sp
    call riscv64_trap_handler
    # Mask SIE before arming sscratch: a trap taken after sscratch holds the
    # kernel stack top but before sret would take the U-mode path and build
    # its frame over this live one. sret applies SIE <- SPIE atomically.
    ld t0, 264(sp)
    andi t0, t0, -3      # clear SIE
    ori t0, t0, 0x20     # set SPIE
    csrw sstatus, t0
    ld t1, 256(sp)
    csrw sepc, t1
    andi t0, t0, 0x100   # SPP: returning to S-mode?
    bnez t0, 6f
    # Return to U-mode: arm sscratch with this task's kernel stack top (the
    # U-mode frame always sits right below it), restore every GPR from the
    # frame, user sp last.
    addi t0, sp, 280
    csrw sscratch, t0
    ld x1, 8(sp)
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
    ld x2, 272(sp)
    sret
6:
    # Return to S-mode: sscratch stays 0. The kernel may have scheduled in
    # between; the resumed stack is still this frame's.
    ld x1, 8(sp)
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
    sret

    .global fork_sret_child_from_frame
fork_sret_child_from_frame:
    # a0 -> copied trap frame (not live stack). Caller must leave sscratch as
    # this task's kernel stack top (same invariant as enter_riscv64).
    mv t6, a0
    ld t0, 264(t6)
    andi t0, t0, -3      # clear SIE; sret applies SIE <- SPIE atomically
    ori t0, t0, 0x20
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
    fn fork_sret_child_from_frame(frame: *mut u64) -> !;
}

pub fn fork_sret_child_to_user(frame: *mut u64) -> ! {
    unsafe { fork_sret_child_from_frame(frame) }
}

pub fn init() {
    let v = trap_vector as *const () as usize;
    unsafe {
        // trap_vector treats a non-zero sscratch as "trapped from U-mode".
        asm!("csrw sscratch, zero", options(nostack));
        asm!("csrw stvec, {v}", v = in(reg) v, options(nostack));
        // Timer only on BSP bring-up. SSIE (IPI) is enabled in ap_init /
        // enable_ipi once secondaries are online — enabling it too early
        // raced with empty sscratch on the first user enter (sepc=-2).
        asm!("csrs sie, {}", in(reg) 1 << 5, options(nostack)); // STIE
        asm!("csrs sstatus, {}", in(reg) 1 << 1, options(nostack)); // SIE
    }
    init_timer();
}

pub fn enable_ipi() {
    unsafe {
        asm!("csrs sie, {}", in(reg) 1 << 1, options(nostack)); // SSIE
    }
}

pub fn ap_init(_logical: usize) {
    let v = trap_vector as *const () as usize;
    unsafe {
        asm!("csrw sscratch, zero", options(nostack));
        asm!("csrw stvec, {v}", v = in(reg) v, options(nostack));
        // Program STIE+SSIE but leave SIE clear until ap_idle_loop.
        asm!("csrs sie, {}", in(reg) (1 << 5) | (1 << 1), options(nostack));
    }
    init_timer();
}

pub fn wait_for_interrupt_proof() {
    while !TIMER_FIRED.load(Ordering::SeqCst) {
        unsafe {
            asm!("wfi", options(nostack, preserves_flags));
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
        asm!("csrw stimecmp, {v}", v = in(reg) val, options(nostack));
    }
}

fn timer_interval() -> u64 {
    // 10 ms at the 10 MHz `time` clock on QEMU virt (100 Hz, like aarch64).
    // The previous 1_000_000 was a 100 ms quantum — a tenth of the intended
    // rate — which made every preemption, deadline wake and console poll
    // wait up to 100 ms.
    100_000
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
        if code == 1 {
            // Supervisor software interrupt (SBI IPI). Clear SSIP.
            unsafe {
                asm!("csrc sip, {}", in(reg) 1 << 1, options(nostack));
            }
            // Bit0 of SSCRATCH-less mailbox: generation in smp; always flush + schedule.
            if crate::smp::ipi_is_tlb() {
                unsafe {
                    asm!("sfence.vma zero, zero", options(nostack));
                }
                crate::smp::tlb_ipi_ack();
            }
            if crate::smp::ipi_is_resched() {
                crate::task::schedule();
            }
            return;
        }
        if code == 5 {
            // Supervisor timer
            TIMER_FIRED.store(true, Ordering::SeqCst);
            crate::time::note_tick();
            crate::rng::stir_tick();
            rearm_timer();
            // BSP stages 16550 RX so a blocked console reader is woken instead
            // of polling the UART itself (see x86 timer). The CI #150 hang
            // once blamed on this was the trap-vector t0 clobber, fixed since.
            if crate::smp::cpu_id() == 0 && crate::input::drain_uart_irq() {
                crate::task::wake(crate::task::KEY_CONSOLE);
            }
            crate::task::timer_tick();
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
                // Keep SIE clear for the syscall body (x86 cli / aarch64 daifset).
                // Enabling SIE here raced with the global SYSCALL_FRAME and with
                // signal-driven user_exit (die without clearing the frame
                // pointer), which corrupted kernel stacks on riscv and showed
                // up as illegal insn in HHDM/.rodata at getty login.
                // Return past the ecall unless the signal code redirects it.
                *frame.add(32) = sepc + 4;
                crate::user::set_syscall_frame(frame);
                let ret = crate::user::syscall_dispatch(
                    nr,
                    a0,
                    a1,
                    a2,
                    (sepc + 4) as usize,
                    user_sp as usize,
                    frame,
                );
                crate::user::set_syscall_frame(core::ptr::null_mut());
                *frame.add(10) = ret as u64;
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
            // Dump ra/a-regs + a few stack words so the faulting caller is
            // symbolizable (x86 does the same for page faults).
            let ra = unsafe { *frame.add(1) };
            let mut regs = alloc::string::String::new();
            for idx in 3..18usize {
                let v = unsafe { *frame.add(idx) };
                regs.push_str(&alloc::format!(" x{idx}={v:#x}"));
            }
            let mut stack = alloc::string::String::new();
            {
                let a = crate::task::current_aspace();
                if a != 0 {
                    for i in 0..8usize {
                        let addr = user_sp as usize + i * 8;
                        let mut v: u64 = 0;
                        let mut ok = true;
                        for b in 0..8usize {
                            match crate::user::try_read_user_u8(a, addr + b) {
                                Some(byte) => v |= (byte as u64) << (8 * b),
                                None => {
                                    ok = false;
                                    break;
                                }
                            }
                        }
                        if !ok {
                            break;
                        }
                        stack.push_str(&alloc::format!(" [{addr:#x}]={v:#x}"));
                    }
                }
            }
            // U-mode faults: kill the task (SIGSEGV convention) instead of
            // halting QEMU — same policy as aarch64 lower_sync data/insn aborts.
            // Two lines: the harness may kill QEMU within ~100ms of the WARN
            // (fail-fast), truncating the serial tail. Keep the caller regs on
            // the first line so they always survive.
            crate::exception::user_fault_warn(
                kind,
                &alloc::format!("ra={ra:#x}{}", regs),
            );
            crate::exception::user_fault_kill(
                kind,
                &alloc::format!(
                    "stval={stval:#x} sepc={sepc:#x} sp={user_sp:#x}{}{stack}{map}",
                    crate::exception::task_ctx(),
                    map = {
                        let (ub, span, soff) = crate::task::current_user_map();
                        let top = ub + soff + (crate::user::USER_STACK_PAGES * 4096) as u64;
                        alloc::format!(
                            " userbase={ub:#x} span={span:#x} stackoff={soff:#x} stacktop={top:#x} brk={:#x}",
                            crate::task::current_brk()
                        )
                    },
                ),
            );
        }
        _ => {
            let sepc = unsafe { *frame.add(32) };
            crate::exception::riscv64_trap(code, sepc, stval);
        }
    }
}


const SBI_EXT_IPI: u64 = 0x7350_4949; // "IPI\0"
const SBI_IPI_SEND: u64 = 0;

fn sbi_send_ipi(hart_mask: u64, hart_mask_base: u64) {
    let mut _err: i64;
    let mut _val: i64;
    unsafe {
        asm!(
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

pub fn ipi_tlb_shootdown() {
    crate::smp::ipi_mark_tlb();
    ipi_others();
}

pub fn ipi_reschedule() {
    crate::smp::ipi_mark_resched();
    ipi_others();
}

/// Reschedule IPI to one logical CPU (its hart id via SBI).
pub fn ipi_reschedule_cpu(cpu: usize) {
    if !crate::smp::cpu_online(cpu) {
        return;
    }
    crate::smp::ipi_mark_resched();
    sbi_send_ipi(1, crate::smp::cpu_hw_id(cpu));
}
