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
    # between; the resumed stack is still this frame's. tp is not restored:
    # in S-mode it is this hart's index, and a task without a home CPU (a
    # module's kernel thread blocking in `block_until`) that the handler
    # scheduled away from may resume on another hart, whose index must stay.
    ld x1, 8(sp)
    ld x3, 24(sp)
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
    ld x10, 80(t6)
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
    crate::console::status_info(crate::irqchip::root().name());
    crate::irqchip::root().init_cpu();
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
    crate::irqchip::root().init_cpu();
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

fn read_stimecmp() -> u64 {
    let v: u64;
    unsafe {
        asm!("csrr {v}, stimecmp", v = out(reg) v, options(nomem, nostack));
    }
    v
}

/// `time` ticks per scheduler tick: 10 ms (100 Hz, like aarch64) at the
/// device tree's `timebase-frequency`.
fn timer_interval() -> u64 {
    (super::clock::timebase_hz() / 100).max(1)
}

/// A monotonic-ns deadline as a `time` value (same clock as
/// `clock::monotonic_ns`).
fn ns_to_ticks(ns: u64) -> u64 {
    ((ns as u128 * super::clock::timebase_hz() as u128) / 1_000_000_000) as u64
}

/// Arm this hart's timer for the next tick, or for the earliest sleep
/// deadline (`task::next_deadline_ns`) when that is sooner, so a
/// `nanosleep` / `poll` timeout ends when it should and not at the next
/// 10 ms boundary.
fn arm_timer() {
    let now = read_time();
    let mut target = now.wrapping_add(timer_interval());
    let deadline = crate::task::next_deadline_ns();
    if deadline != u64::MAX {
        target = target.min(ns_to_ticks(deadline).max(now + 1));
    }
    write_stimecmp(target);
}

/// A new sleep deadline on this hart: pull the timer in if it is armed
/// later than that.
pub fn timer_deadline(deadline_ns: u64) {
    let target = ns_to_ticks(deadline_ns);
    if target < read_stimecmp() {
        write_stimecmp(target.max(read_time() + 1));
    }
}

/// The harts whose tick [`timer_idle`] stopped.
static TICK_STOPPED: [AtomicBool; crate::smp::MAX_CPUS] = [const { AtomicBool::new(false) }; crate::smp::MAX_CPUS];

/// This hart halts with nothing to run: no tick, its timer fires once, at
/// `wake_ns` (monotonic). Interrupts off.
pub fn timer_idle(wake_ns: u64) {
    let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
    TICK_STOPPED[cpu].store(true, Ordering::Relaxed);
    write_stimecmp(ns_to_ticks(wake_ns).max(read_time() + 1));
}

/// The tick back on this hart if [`timer_idle`] stopped it. Interrupts off.
pub fn timer_resume() {
    let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
    if TICK_STOPPED[cpu].swap(false, Ordering::Relaxed) {
        arm_timer();
    }
}

fn init_timer() {
    arm_timer();
}

fn rearm_timer() {
    arm_timer();
}

/// Whether the trap came from user mode: the saved `sstatus.SPP` is clear.
fn from_user(frame: *const u64) -> bool {
    (unsafe { *frame.add(33) } & 0x100) == 0
}

/// A U-mode fault's signal `sig` (`code`, `addr`: its siginfo) to the
/// program's handler, from the trap frame: false when it does not catch it.
fn raise(frame: *mut u64, sig: u32, code: u32, addr: u64) -> bool {
    let info = crate::signal::Info { sig, code, addr: addr as usize };
    crate::signal::on_fault(&mut crate::user::SyscallRegs::from_ptr(frame), info)
}

/// After an interrupt's reschedule: the task's signals act if it came from
/// user mode, from the trap frame.
fn preempted(frame: *mut u64) {
    if from_user(frame) {
        crate::signal::on_user_interrupt(&mut crate::user::SyscallRegs::from_ptr(frame));
    }
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
                preempted(frame);
            }
            return;
        }
        if code == 9 {
            // Supervisor external interrupt: claim from the interrupt
            // controller (the PLIC, the AIA's IMSIC), dispatch, complete.
            // Loop: several sources may be pending.
            let chip = crate::irqchip::root();
            while let Some(src) = chip.claim() {
                crate::irq::dispatch(src);
                chip.complete(src);
            }
            return;
        }
        if code == 5 {
            // Supervisor timer
            TIMER_FIRED.store(true, Ordering::SeqCst);
            crate::time::note_tick();
            crate::rng::stir_tick();
            rearm_timer();
            // The BSP stages UART input when the UART has no interrupt.
            if crate::smp::cpu_id() == 0 {
                crate::input::tick();
            }
            crate::task::timer_tick();
            crate::task::schedule();
            preempted(frame);
        }
        return;
    }

    let code = scause & 0xfff;
    use crate::signal::{code::*, *};
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
            // An mmap page not touched yet: page it in and retry.
            let access = match code {
                12 => crate::user::Access::Exec,
                13 => crate::user::Access::Read,
                _ => crate::user::Access::Write,
            };
            if crate::user::fault_in(stval as usize, access) {
                return;
            }
            // A program's fault: its SIGSEGV (a page there refused the
            // access, or none is mapped).
            let mapped = super::upaging::virt_to_phys(crate::task::current_aspace(), stval).is_some();
            if from_user(frame) && raise(frame, SIGSEGV, segv_code(stval as usize, mapped), stval) {
                return;
            }
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
                SIGSEGV,
                kind,
                sepc,
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
            // From U-mode (an illegal instruction, scause 2; an `ebreak`, 3;
            // a misaligned access, 0, 4 and 6; an access fault, 1, 5 and 7):
            // the program's signal, or the task ends instead of halting the
            // machine. Only a genuine S-mode trap is fatal.
            if from_user(frame) {
                let (sig, si, addr) = match code {
                    2 => (SIGILL, ILL_ILLOPC, sepc),
                    3 => (SIGTRAP, TRAP_BRKPT, sepc),
                    0 | 4 | 6 => (SIGBUS, BUS_ADRALN, stval),
                    _ => (SIGSEGV, SEGV_ACCERR, stval),
                };
                if raise(frame, sig, si, addr) {
                    return;
                }
                crate::exception::user_fault_kill(
                    sig,
                    "trap",
                    sepc,
                    &alloc::format!("scause={code:#x} sepc={sepc:#x} stval={stval:#x}{}", crate::exception::task_ctx()),
                );
            }
            super::exception::riscv64_trap(code, sepc, stval);
        }
    }
}
