//! User mode on riscv64: the user VA layout, the live syscall frame, the
//! `sscratch` kernel-stack protocol, U-mode entry, and the registers a new
//! task starts with.

use crate::smp::MAX_CPUS;
use crate::task::STACK_SIZE;

/// User images load at Sv39 root[1] on QEMU virt RAM.
pub const DEFAULT_USER_BASE: u64 = 0x4000_0000;
/// The mmap window (after the brk heap), see x86: 960 MiB, the rest of the
/// 1 GiB span after the largest image, the stack and the heap.
pub const MMAP_AREA_PAGES: usize = 245760;
/// User stack below the heap: 1 MiB.
pub const USER_STACK_PAGES: usize = 256;
/// Per-process brk heap capacity: the TLS arena is a 2 MiB brk allocation,
/// so the window must fit that plus headroom.
pub const HEAP_PAGES: usize = 1024;
/// The process lives in Sv39 root[1]: 1 GiB.
pub const USER_SPAN_PAGES: usize = 1 << 18;

/// The user registers a new task starts with (fork child, thread).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct UserRegs {
    pub rip: usize,
    pub rsp: usize,
    /// Trap frame (x0..x31, sepc, sstatus, user sp). Index 35 unused.
    pub frame: [u64; 36],
}

/// Give `regs` thread pointer `v`: riscv64's thread pointer is the register
/// `tp`, saved in the frame with the others.
pub fn regs_set_tls(regs: &mut UserRegs, v: u64) {
    regs.frame[4] = v;
}

/// Make `regs` start a thread at its `rip` as if `entry(arg)` was called on
/// the 16-byte aligned stack top `top` (it must not return).
pub fn regs_thread_start(regs: &mut UserRegs, top: usize, arg: usize) {
    regs.rsp = top;
    regs.frame[10] = arg as u64; // a0
    regs.frame[1] = 0; // ra
}

// Trap frame indices (`user::SyscallRegs`): x0..x31, pc at 32, sp at 34.
pub const SYSCALL_PC: usize = 32;
pub const SYSCALL_SP: usize = 34;
/// The syscall number register (a7).
pub const SYSCALL_NR_REG: Option<usize> = Some(17);
/// The thread pointer register (tp, x4), restored from the frame.
pub const SYSCALL_TP_REG: Option<usize> = Some(4);
/// The fourth to sixth arguments: a3, a4, a5 (x13..x15).
pub const SYSCALL_ARGS_3_5: [usize; 3] = [13, 14, 15];
/// The result register (a0).
const SYSCALL_RESULT: usize = 10;
/// Length of the `ecall` instruction.
pub const SYSCALL_INSN_LEN: usize = 4;
/// A rewound syscall restarts with its first argument in the result register.
pub const SYSCALL_RESTART_IS_NR: bool = false;

/// Live trap frame of the syscall running on each CPU: a syscall sets it on
/// entry and clears it on exit; fork and exec read it on the same CPU. One
/// cell per CPU, so user tasks on several CPUs never see each other's frame,
/// and `schedule` keeps it with the task across a switch (`Task::syscall_frame`),
/// so a syscall that blocked finds its own frame again, not that of the
/// task that ran a syscall on this CPU meanwhile.
static mut SYSCALL_FRAMES: [*mut usize; MAX_CPUS] = [core::ptr::null_mut(); MAX_CPUS];

/// Record the live trap frame for fork/exec resume.
pub fn set_syscall_frame(frame: *mut u64) {
    let cpu = crate::smp::cpu_id().min(MAX_CPUS - 1);
    unsafe {
        core::ptr::addr_of_mut!(SYSCALL_FRAMES[cpu]).write(frame as *mut usize);
    }
}

/// The frame recorded by [`set_syscall_frame`] on this CPU (null outside a
/// syscall).
pub fn syscall_frame() -> *mut usize {
    let cpu = crate::smp::cpu_id().min(MAX_CPUS - 1);
    unsafe { core::ptr::addr_of!(SYSCALL_FRAMES[cpu]).read() }
}

/// The calling thread's user registers from its syscall trap frame, as a
/// forked child resumes with them (result 0).
pub fn caller_regs(frame: *mut u64) -> UserRegs {
    let mut f = copy_fork_syscall_frame(frame as *const u64);
    f[SYSCALL_RESULT] = 0;
    UserRegs {
        rip: f[SYSCALL_PC] as usize,
        rsp: f[SYSCALL_SP] as usize,
        frame: f,
    }
}

/// Nothing to enable: native programs are soft-float (the Linux layer turns
/// the FPU on per task through `sstatus`).
pub fn user_init() {}

pub fn user_ap_init() {}

/// Kernel stack top for the next trap from U-mode on this hart. Not written
/// to the `sscratch` CSR here: it must stay 0 while the hart runs in S-mode
/// (the trap vector reads non-zero as "trapped from U-mode"); every sret to
/// U-mode arms it with this value itself.
static mut KERNEL_SSCRATCH: usize = 0;

/// Publish the running task's kernel stack top for the next trap from user
/// mode on this hart (stack footer + static kept coherent).
pub fn set_kernel_stack_top(top: usize) {
    let cpu = crate::smp::cpu_id().min(MAX_CPUS - 1);
    stamp_stack_cpu(top, cpu);
    unsafe {
        core::ptr::addr_of_mut!(KERNEL_SSCRATCH).write(top);
    }
}

/// The arch word of a foreign-personality signal frame (x86_64's user CS):
/// none here.
pub fn signal_arch_word() -> u64 {
    0
}

/// Stamp the logical CPU id into the footer below a task's kernel stack so
/// the trap vector can reload `tp` without trusting user TLS (see the trap
/// vector in `interrupts`). Word 0 of the stack allocation is reserved for
/// this footer.
pub fn stamp_stack_cpu(kstack_top: usize, cpu: usize) {
    if kstack_top < STACK_SIZE {
        return;
    }
    let cpu = cpu.min(MAX_CPUS - 1);
    unsafe {
        ((kstack_top - STACK_SIZE) as *mut usize).write(cpu);
    }
}

/// Exec from a syscall: do **not** resume through the live syscall trap frame
/// after a deep in-place expand (ripgrep after uutils ls on /heap). That
/// frame sits near the top of the 64 KiB kstack; expand + realize + PTE walks
/// push LLVM spill slots into the same page, and rewriting `sepc` there has
/// left it 0 before the sret. Clear the frame and let the caller fall through
/// to [`enter_user`]'s volatile resume image instead.
pub fn exec_resume(_entry: usize, _rsp: usize, _argc: usize, _argv: usize) {
    set_syscall_frame(core::ptr::null_mut());
}

const USER_SSTATUS: u64 = (2 << 32) | (1 << 5); // UXL=64-bit user, SPIE, SPP=0

/// `sstatus` for entering U-mode. The optional Linux layer turns the FPU on
/// for Linux tasks (rv64gc); native programs are soft-float.
fn user_sstatus() -> u64 {
    if crate::personality::fpu_on() {
        return USER_SSTATUS | super::fpu::SSTATUS_FS_INITIAL;
    }
    USER_SSTATUS
}

/// First entry to user mode (the aspace is already loaded).
pub fn enter_user(user_rip: usize, user_rsp: usize, argc: usize, argv: usize) -> ! {
    enter_riscv64(user_rip, user_rsp, argc, argv)
}

fn enter_riscv64(user_rip: usize, user_rsp: usize, user_argc: usize, user_argv: usize) -> ! {
    super::cpu::sync_cpu_id_reg();
    let ksp = {
        let t = crate::task::current_kernel_stack_top();
        if t != 0 {
            t
        } else {
            unsafe { KERNEL_SSCRATCH }
        }
    };
    stamp_stack_cpu(ksp, crate::smp::cpu_id());
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

/// Start a new task (forked child or thread) in user mode with `regs` (the
/// aspace is already loaded).
pub fn enter_user_regs(regs: UserRegs) -> ! {
    enter_regs_riscv64(regs)
}

fn copy_fork_syscall_frame(src: *const u64) -> [u64; 36] {
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

fn enter_regs_riscv64(regs: UserRegs) -> ! {
    let mut frame = regs.frame;
    frame[32] = regs.rip as u64;
    frame[33] = user_sstatus();
    frame[34] = regs.rsp as u64;
    super::cpu::sync_cpu_id_reg();
    let ksp = {
        let t = crate::task::current_kernel_stack_top();
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
            stamp_stack_cpu(ksp, crate::smp::cpu_id());
            core::ptr::addr_of_mut!(KERNEL_SSCRATCH).write(ksp);
            core::arch::asm!(
                "csrci sstatus, 2",
                "csrw sscratch, {ksp}",
                ksp = in(reg) ksp,
                options(nostack),
            );
        }
        super::interrupts::fork_sret_child_to_user(frame.as_mut_ptr());
    }
}
