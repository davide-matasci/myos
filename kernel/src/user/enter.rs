//! First entry to user mode after exec, and the fork-child return paths.

use super::*;

#[cfg(target_arch = "aarch64")]
pub(super) fn current_el() -> u64 {
    let el: u64;
    unsafe {
        core::arch::asm!(
            "mrs {el}, CurrentEL",
            el = out(reg) el,
            options(nomem, nostack, preserves_flags)
        );
    }
    (el >> 2) & 3
}

pub fn enter(user_rip: usize, user_rsp: usize, user_argc: usize, user_argv: usize) -> ! {
    let a = task::current_aspace();
    if a != 0 {
        switch_aspace(a);
    }
    // x86 passes argc/argv on the initial user stack, not in registers.
    #[cfg(target_arch = "x86_64")]
    {
        let _ = (user_argc, user_argv);
        enter_x86(user_rip, user_rsp);
    }
    #[cfg(target_arch = "aarch64")]
    enter_aarch64(user_rip, user_rsp, user_argc, user_argv);
    #[cfg(target_arch = "riscv64")]
    enter_riscv64(user_rip, user_rsp, user_argc, user_argv);
}

#[cfg(target_arch = "x86_64")]
fn enter_x86(user_rip: usize, user_rsp: usize) -> ! {
    // Refresh per-CPU ring0 state in case this CPU never scheduled the task
    // (or GS/TSS drifted). Required before the first AP iretq into ring3.
    let ktop = crate::task::current_kernel_stack_top();
    if ktop != 0 {
        set_kernel_rsp0(ktop);
        crate::arch::gdt::set_rsp0(ktop as u64);
    }
    let cs = (crate::arch::gdt::user_cs() | 3) as u64;
    let ss = (crate::arch::gdt::user_ss() | 3) as u64;
    let rflags: u64 = 0x202;
    // Iret frame in memory (RIP, CS, RFLAGS, RSP, SS). Do not feed five `in(reg)`
    // operands into one asm block: LLVM can reuse a register for CS and corrupt
    // iretq (post-fork exec of large ELFs → #GP on BIOS).
    //
    // Volatile + GPR scrub: `mov rsp, frame_ptr` leaves the kernel stack
    // address (HHDM) in a GPR across iretq. Userspace then faulted on that
    // pointer (UEFI boot-mini `cat | cat`: cr2=0xffff8000… code=0x5). Same
    // discipline as enter_fork_x86 / enter_riscv64.
    let resume = [user_rip as u64, cs, rflags, user_rsp as u64, ss];
    let mut slot = core::mem::MaybeUninit::<[u64; 5]>::uninit();
    const CR0_TS: u64 = 1 << 3;
    unsafe {
        let mut cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0);
        cr0 &= !CR0_TS;
        core::arch::asm!("mov cr0, {}", in(reg) cr0);
        core::ptr::write_volatile(slot.as_mut_ptr(), resume);
        let f = slot.as_ptr();
        // Mov rsp first so scrubbing GPRs cannot zero the frame pointer reg.
        core::arch::asm!(
            "cli",
            "mov rsp, {f}",
            "xor rax, rax",
            "xor rcx, rcx",
            "xor rdx, rdx",
            "xor rbx, rbx",
            "xor rbp, rbp",
            "xor rsi, rsi",
            "xor rdi, rdi",
            "xor r8, r8",
            "xor r9, r9",
            "xor r10, r10",
            "xor r11, r11",
            "xor r12, r12",
            "xor r13, r13",
            "xor r14, r14",
            "xor r15, r15",
            "iretq",
            f = in(reg) f,
            options(noreturn),
        );
    }
}

#[cfg(target_arch = "aarch64")]
fn enter_aarch64(user_rip: usize, user_rsp: usize, user_argc: usize, user_argv: usize) -> ! {
    // Load from a stack slot so LLVM cannot reuse rip/argc in one asm block
    // and reorder mov before msr (CI: exec eret with elr=0).
    //
    // Volatile + GPR scrub: leftover kernel GPRs (HHDM / frame pointers) across
    // eret used to reach userspace. Same discipline as enter_x86 / enter_riscv64
    // — aarch64 boot-mini `echo pipe | cat` hit FAR=0 with elr in oksh
    // (CI #35570070681) after exec of pipeline children.
    let resume = [
        user_rip as u64,
        user_rsp as u64,
        user_argc as u64,
        user_argv as u64,
    ];
    let mut slot = core::mem::MaybeUninit::<[u64; 4]>::uninit();
    unsafe {
        core::ptr::write_volatile(slot.as_mut_ptr(), resume);
        let p = slot.as_ptr();
        core::arch::asm!(
            "ldr {rip}, [{p}]",
            "ldr {rsp}, [{p}, #8]",
            "msr elr_el1, {rip}",
            "msr sp_el0, {rsp}",
            "msr spsr_el1, xzr",
            p = in(reg) p,
            rip = out(reg) _,
            rsp = out(reg) _,
            options(nostack, preserves_flags),
        );
        // Load argc/argv first, then scrub remaining GPRs so eret cannot leak
        // kernel addresses into EL0 (mov/ldr into x0/x1 must precede the zeros).
        core::arch::asm!(
            "ldr x0, [{p}, #16]",
            "ldr x1, [{p}, #24]",
            "mov x2, xzr",
            "mov x3, xzr",
            "mov x4, xzr",
            "mov x5, xzr",
            "mov x6, xzr",
            "mov x7, xzr",
            "mov x8, xzr",
            "mov x9, xzr",
            "mov x10, xzr",
            "mov x11, xzr",
            "mov x12, xzr",
            "mov x13, xzr",
            "mov x14, xzr",
            "mov x15, xzr",
            "mov x16, xzr",
            "mov x17, xzr",
            "mov x18, xzr",
            "mov x19, xzr",
            "mov x20, xzr",
            "mov x21, xzr",
            "mov x22, xzr",
            "mov x23, xzr",
            "mov x24, xzr",
            "mov x25, xzr",
            "mov x26, xzr",
            "mov x27, xzr",
            "mov x28, xzr",
            "mov x29, xzr",
            "mov x30, xzr",
            "isb",
            "eret",
            p = in(reg) p,
            options(noreturn, nostack),
        );
    }
}

#[cfg(target_arch = "riscv64")]
pub(super) fn enter_riscv64(user_rip: usize, user_rsp: usize, user_argc: usize, user_argv: usize) -> ! {
    crate::smp::sync_tp_for_kernel();
    let ksp = {
        let t = task::current_kernel_stack_top();
        if t != 0 {
            t
        } else {
            unsafe { KERNEL_SSCRATCH }
        }
    };
    task::stamp_stack_cpu(ksp, crate::smp::cpu_id());
    // Volatile resume image — same discipline as enter_aarch64 / enter_x86.
    // Keep argc/argv/sp/sepc in memory; load into dedicated regs, then scrub.
    // Under MTTCG full-boot, interactive `http https://…` hit
    // `instruction page fault stval=0 sepc=0` when LLVM parked `rip`/`usp` in
    // a register the scrub then zeroed (sret with sepc=0).
    let resume = [
        user_rip as u64,
        user_rsp as u64,
        user_argc as u64,
        user_argv as u64,
    ];
    let mut slot = core::mem::MaybeUninit::<[u64; 4]>::uninit();
    unsafe {
        core::ptr::write_volatile(slot.as_mut_ptr(), resume);
        let p = slot.as_ptr();
        let rip: u64;
        let usp: u64;
        let argc: u64;
        let argv: u64;
        core::arch::asm!(
            "ld {rip}, 0({p})",
            "ld {usp}, 8({p})",
            "ld {argc}, 16({p})",
            "ld {argv}, 24({p})",
            p = in(reg) p,
            rip = out(reg) rip,
            usp = out(reg) usp,
            argc = out(reg) argc,
            argv = out(reg) argv,
            options(nostack, preserves_flags),
        );
        core::arch::asm!(
            // USER_SSTATUS has SIE clear: mask interrupts before arming
            // sscratch so no S-mode trap can see the non-zero value.
            "csrw sstatus, {s}",
            "csrw sscratch, {ksp}",
            "mv sp, {usp}",
            "mv a0, {argc}",
            "mv a1, {argv}",
            "csrw sepc, {rip}",
            "mv ra, zero",
            "mv gp, zero",
            "mv tp, zero",
            "mv t0, zero",
            "mv t1, zero",
            "mv t2, zero",
            "mv s0, zero",
            "mv s1, zero",
            "mv a2, zero",
            "mv a3, zero",
            "mv a4, zero",
            "mv a5, zero",
            "mv a6, zero",
            "mv a7, zero",
            "mv s2, zero",
            "mv s3, zero",
            "mv s4, zero",
            "mv s5, zero",
            "mv s6, zero",
            "mv s7, zero",
            "mv s8, zero",
            "mv s9, zero",
            "mv s10, zero",
            "mv s11, zero",
            "mv t3, zero",
            "mv t4, zero",
            "mv t5, zero",
            "mv t6, zero",
            "sret",
            ksp = in(reg) ksp,
            usp = in(reg) usp,
            argc = in(reg) argc,
            argv = in(reg) argv,
            rip = in(reg) rip,
            s = in(reg) user_sstatus(),
            options(noreturn, nostack),
        );
    }
}

/// Exec from a syscall: rewrite the saved frame and eret (aarch64).
///
/// riscv64 deliberately does **not** resume through the live syscall trap
/// frame after exec — deep `expand_user_elf` (ripgrep) shares the 64KiB
/// kstack with that frame and reopened `sepc=0` IPF; see the riscv64 branch
/// of `sys_exec` which clears `SYSCALL_FRAME` and falls through to
/// `enter_riscv64`'s volatile resume image instead.
#[cfg(target_arch = "aarch64")]
pub(super) fn try_resume_exec_via_syscall_frame(entry: usize, rsp: usize, argc: usize, argv: usize) {
    let frame_ptr = super::syscall::syscall_frame();
    if frame_ptr.is_null() {
        return;
    }
    unsafe {
        let frame = frame_ptr as *mut u64;
        *frame.add(0) = argc as u64;
        *frame.add(1) = argv as u64;
        *frame.add(32) = entry as u64;
        *frame.add(34) = rsp as u64;
        crate::arch::fork_eret_to_user(frame);
    }
}

/// Resume a forked child with the parent's user GPRs (rax/x0 = 0).
pub fn enter_fork(regs: task::ForkRegs) -> ! {
    let a = task::current_aspace();
    if a != 0 {
        switch_aspace(a);
    }
    #[cfg(target_arch = "x86_64")]
    enter_fork_x86(regs);
    #[cfg(target_arch = "aarch64")]
    enter_fork_aarch64(regs);
    #[cfg(target_arch = "riscv64")]
    enter_fork_riscv64(regs);
}

/// Packed resume image for `fork_iret_to_user` (global_asm). Layout must match
/// the offsets in that stub — do not reorder fields.
#[cfg(target_arch = "x86_64")]
#[repr(C, align(16))]
struct ForkResumeX86 {
    rbx: u64,
    rbp: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    r#"
    .global fork_iret_to_user
fork_iret_to_user:
    cli
    # Clear CR0.EM|TS (mask ~0xc); set MP.
    mov rax, cr0
    # ~ (EM|TS)= ~0xc = -13; signed imm32 (0xfffffff3 alone is rejected)
    and rax, -13
    or  rax, 2
    mov cr0, rax
    fninit
    ldmxcsr [rip + {mxcsr}]
    # rdi -> ForkResumeX86. Restore user callee-saved, then iret frame.
    mov rbx, [rdi]
    mov rbp, [rdi + 8]
    mov r12, [rdi + 16]
    mov r13, [rdi + 24]
    mov r14, [rdi + 32]
    mov r15, [rdi + 40]
    lea rsp, [rdi + 48]
    # Scrub caller-saved GPRs (incl. rdi = kernel resume ptr / HHDM). Matches
    # enter_x86: leftover kernel addresses across iretq became user #PF
    # (cr2 in HHDM) on UEFI pipeline fork before exec.
    xor rax, rax
    xor rcx, rcx
    xor rdx, rdx
    xor rsi, rsi
    xor rdi, rdi
    xor r8, r8
    xor r9, r9
    xor r10, r10
    xor r11, r11
    iretq
    "#,
    mxcsr = sym USER_MXCSR_DEFAULT,
);

#[cfg(target_arch = "x86_64")]
unsafe extern "C" {
    fn fork_iret_to_user(resume: *const ForkResumeX86) -> !;
}

#[cfg(target_arch = "x86_64")]
fn enter_fork_x86(regs: task::ForkRegs) -> ! {
    let cs = (crate::arch::gdt::user_cs() | 3) as u64;
    let ss = (crate::arch::gdt::user_ss() | 3) as u64;
    let rflags: u64 = 0x202;
    // Resume through global_asm — not inline asm. Prior enter_fork_x86 variants
    // used black_box arrays + as_ptr() / forbidden rbx,rbp constraints; LLVM
    // could leave the child with garbage callee-saved regs. Identical #GP
    // rip/rsp across those "fixes" matches a restore that never stuck.
    // Parent returns via sysret with the same user RSP; child must get the
    // exact syscall RSP and the FORK_CALLEE snapshot of rbx/rbp/r12–r15.
    let resume = ForkResumeX86 {
        rbx: regs.rbx,
        rbp: regs.rbp,
        r12: regs.r12,
        r13: regs.r13,
        r14: regs.r14,
        r15: regs.r15,
        rip: regs.rip as u64,
        cs,
        rflags,
        rsp: regs.rsp as u64,
        ss,
    };
    // Volatile write so the stores exist in memory before the asm reads them.
    let mut slot = core::mem::MaybeUninit::<ForkResumeX86>::uninit();
    unsafe {
        core::ptr::write_volatile(slot.as_mut_ptr(), resume);
        fork_iret_to_user(slot.as_ptr());
    }
}

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
pub(super) fn copy_fork_syscall_frame(src: *const u64) -> [u64; 36] {
    let mut frame = [0u64; 36];
    unsafe {
        for i in 0..=31 {
            frame[i] = *src.add(i);
        }
        frame[32] = *src.add(32);
        frame[33] = *src.add(33);
        frame[34] = *src.add(34);
    }
    frame
}

#[cfg(target_arch = "aarch64")]
fn enter_fork_aarch64(regs: task::ForkRegs) -> ! {
    // Resume through the same restore path as `lower_sync` (preserves spsr and
    // callee-saved state). Rebuilding ELR/SP_EL0 in one asm block miscompiled on
    // CI and left the child with x0 != 0 → parent+child both blocked in wait.
    let mut frame = regs.frame;
    frame[0] = 0;
    crate::arch::fork_eret_to_user(frame.as_mut_ptr());
}

#[cfg(target_arch = "riscv64")]
fn enter_fork_riscv64(regs: task::ForkRegs) -> ! {
    let mut frame = regs.frame;
    frame[32] = regs.rip as u64; // resume past the fork ecall
    frame[33] = user_sstatus();
    frame[34] = regs.rsp as u64;
    crate::smp::sync_tp_for_kernel();
    let ksp = {
        let t = task::current_kernel_stack_top();
        if t != 0 {
            t
        } else {
            unsafe { KERNEL_SSCRATCH }
        }
    };
    unsafe {
        // Arm sscratch with the kernel stack top for the next user trap
        // (enter_riscv64 invariant). Mask SIE first: sscratch must be 0 for
        // any trap taken in S-mode, and fork_sret_child_from_frame keeps SIE
        // clear until sret.
        if ksp != 0 {
            task::stamp_stack_cpu(ksp, crate::smp::cpu_id());
            core::ptr::addr_of_mut!(KERNEL_SSCRATCH).write(ksp);
            core::arch::asm!(
                "csrci sstatus, 2",
                "csrw sscratch, {ksp}",
                ksp = in(reg) ksp,
                options(nostack),
            );
        }
        crate::arch::fork_sret_child_to_user(frame.as_mut_ptr());
    }
}
