//! User mode on aarch64: the user VA layout, the live syscall frame, EL0
//! entry, and the registers a new task starts with.

use crate::smp::MAX_CPUS;

/// User images load at L1[1] on QEMU virt RAM.
pub const DEFAULT_USER_BASE: u64 = 0x4000_0000;
/// The mmap window (after the brk heap), see x86. Stays within
/// `upaging::USER_L2_TABLES`.
pub const MMAP_AREA_PAGES: usize = 16384;
/// User stack below the heap: 512 KiB (user maps spill into L2[1+] when
/// code+stack+heap exceed 512 pages).
pub const USER_STACK_PAGES: usize = 128;
/// Per-process brk heap capacity: the TLS arena is a 2 MiB brk allocation,
/// and zstd's 4 MiB window in get-alpine needs more than 3 MiB; 16 MiB.
pub const HEAP_PAGES: usize = 4096;
/// Image, stack, heap and the mmap window must fit the L2 span (128 MiB).
pub const USER_SPAN_PAGES: usize = super::upaging::USER_L2_TABLES * super::upaging::USER_L3_PAGES;

/// The user registers a new task starts with (fork child, thread).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct UserRegs {
    pub rip: usize,
    pub rsp: usize,
    /// Full `lower_sync` frame (x0..x30, elr, spsr, sp_el0). Index 31 unused.
    pub frame: [u64; 36],
}

/// Give `regs` thread pointer `v` (`tpidr_el0` travels with the task via
/// `task::tp`, not in the frame: nothing to do).
pub fn regs_set_tls(_regs: &mut UserRegs, _v: u64) {}

/// Make `regs` start a thread at its `rip` as if `entry(arg)` was called on
/// the 16-byte aligned stack top `top` (it must not return).
pub fn regs_thread_start(regs: &mut UserRegs, top: usize, arg: usize) {
    regs.rsp = top;
    regs.frame[0] = arg as u64;
    regs.frame[30] = 0; // lr
}

// Trap frame indices (`user::SyscallRegs`): x0..x30, pc at 32, sp at 34.
pub const SYSCALL_PC: usize = 32;
pub const SYSCALL_SP: usize = 34;
/// The syscall number register (x8).
pub const SYSCALL_NR_REG: Option<usize> = Some(8);
/// The result register (x0).
const SYSCALL_RESULT: usize = 0;
/// Length of the `svc` instruction.
pub const SYSCALL_INSN_LEN: usize = 4;
/// A rewound syscall restarts with its first argument in the result register.
pub const SYSCALL_RESTART_IS_NR: bool = false;

/// Live trap frame of the syscall running on each CPU: a syscall sets it on
/// entry and clears it on exit; fork and exec read it on the same CPU. One
/// cell per CPU, so user tasks on several CPUs never see each other's frame.
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
#[allow(dead_code)]
fn syscall_frame() -> *mut usize {
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

/// Enable user mode on the BSP (FP/SIMD at EL0).
pub fn user_init() {
    init_user_fp();
}

/// Per-CPU user-mode enable.
pub fn user_ap_init() {
    init_user_fp();
}

/// newlib stdio init uses NEON (movi v0.2d). With CPACR_EL1.FPEN=0, EL0 traps on SIMD.
fn init_user_fp() {
    const CPACR_EL1_FPEN: u64 = 3 << 20;
    unsafe {
        let mut cpacr: u64;
        core::arch::asm!("mrs {}, cpacr_el1", out(reg) cpacr);
        cpacr |= CPACR_EL1_FPEN;
        core::arch::asm!("msr cpacr_el1, {}", in(reg) cpacr);
        core::arch::asm!("isb");
    }
}

/// Nothing to publish: the EL1 stack pointer stays in `sp_el1` across EL0.
pub fn set_kernel_stack_top(_top: usize) {}

/// Record which CPU a kernel stack belongs to (riscv64 only; see there).
pub fn stamp_stack_cpu(_kstack_top: usize, _cpu: usize) {}

/// Exec from a syscall: rewrite the saved frame and eret through it, or
/// return so the caller falls through to [`enter_user`].
pub fn exec_resume(entry: usize, rsp: usize, argc: usize, argv: usize) {
    try_resume_exec_via_syscall_frame(entry, rsp, argc, argv);
}

/// Exec from a syscall: rewrite the saved frame and eret (aarch64).
///
/// riscv64 deliberately does **not** resume through the live syscall trap
/// frame after exec — deep `expand_user_elf` (ripgrep) shares the 64KiB
/// kstack with that frame and reopened `sepc=0` IPF; see the riscv64 branch
/// of `sys_exec` which clears `SYSCALL_FRAME` and falls through to
/// `enter_riscv64`'s volatile resume image instead.
fn try_resume_exec_via_syscall_frame(entry: usize, rsp: usize, argc: usize, argv: usize) {
    let frame_ptr = syscall_frame();
    if frame_ptr.is_null() {
        return;
    }
    unsafe {
        let frame = frame_ptr as *mut u64;
        *frame.add(0) = argc as u64;
        *frame.add(1) = argv as u64;
        *frame.add(32) = entry as u64;
        *frame.add(34) = rsp as u64;
        super::interrupts::fork_eret_to_user(frame);
    }
}

/// First entry to user mode (the aspace is already loaded).
pub fn enter_user(user_rip: usize, user_rsp: usize, argc: usize, argv: usize) -> ! {
    enter_aarch64(user_rip, user_rsp, argc, argv)
}

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

/// Start a new task (forked child or thread) in user mode with `regs` (the
/// aspace is already loaded).
pub fn enter_user_regs(regs: UserRegs) -> ! {
    enter_regs_aarch64(regs)
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

fn enter_regs_aarch64(regs: UserRegs) -> ! {
    // Resume through the same restore path as `lower_sync` (preserves spsr and
    // callee-saved state). Rebuilding ELR/SP_EL0 in one asm block miscompiled on
    // CI and left the child with x0 != 0 → parent+child both blocked in wait.
    let mut frame = regs.frame;
    frame[32] = regs.rip as u64;
    frame[34] = regs.rsp as u64;
    super::interrupts::fork_eret_to_user(frame.as_mut_ptr());
}
