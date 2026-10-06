//! User mode on x86_64: the user VA layout, the `syscall` entry, per-CPU
//! syscall state, entering ring 3, and the registers a new task starts with.

use crate::smp::MAX_CPUS;

/// User images load at PML4[1] unless Limine took it (see `upaging::pick_user_base`).
pub const DEFAULT_USER_BASE: u64 = 0x0000_0080_0000_0000;
/// The mmap window (after the brk heap): VA only, its pages get frames on
/// first touch. 4 GiB: room for rustc (its libraries and allocator
/// reservation take over 500 MiB) under the optional Linux layer.
pub const MMAP_AREA_PAGES: usize = 1 << 20;
/// User stack below the heap: 1 MiB.
pub const USER_STACK_PAGES: usize = 256;
/// Per-process brk heap capacity (mapped on demand by `sys_brk`): the TLS
/// arena stays in ELF BSS; git Phase-1 object writes need ≥1 MiB and GNU
/// make's os-test parsing xmallocs well past 512 pages, so 16 MiB.
pub const HEAP_PAGES: usize = 4096;
/// No per-process span limit beyond the PML4 slot (512 GiB).
pub const USER_SPAN_PAGES: usize = usize::MAX;

/// The user registers a new task starts with (fork child, thread).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct UserRegs {
    pub rip: usize,
    pub rsp: usize,
    pub rbx: u64,
    pub rbp: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    /// The argument registers rdi, rsi, rdx, r10, r8, r9 (a syscall
    /// preserves them; rax, the result, starts at 0; rcx and r11 are
    /// clobbered by `sysret` anyway).
    pub args: [u64; 6],
}

/// Give `regs` thread pointer `v` (the FS base travels with the task via
/// `task::tp`, not in the registers: nothing to do).
pub fn regs_set_tls(_regs: &mut UserRegs, _v: u64) {}

/// Make `regs` start a thread at its `rip` as if `entry(arg)` was called on
/// the 16-byte aligned stack top `top` (it must not return).
pub fn regs_thread_start(regs: &mut UserRegs, top: usize, arg: usize) {
    regs.rsp = top - 8; // where `call` leaves the return address
    regs.args[0] = arg as u64;
}

// Layout of the block `syscall_entry` pushes (`user::SyscallRegs`): user
// rsp, rip (rcx), r8, r9, rflags (r11), r10, rdx, rsi, rdi.
pub const SYSCALL_PC: usize = 1;
pub const SYSCALL_SP: usize = 0;
/// The number travels in rax, which the result overwrites.
pub const SYSCALL_NR_REG: Option<usize> = None;
/// The fourth to sixth arguments: r10, r8, r9 (the first three, rdi, rsi
/// and rdx, reach the dispatcher as its own arguments).
pub const SYSCALL_ARGS_3_5: [usize; 3] = [5, 2, 3];
/// Length of the `syscall` instruction.
pub const SYSCALL_INSN_LEN: usize = 2;
/// A rewound syscall restarts with the number in the result register.
pub const SYSCALL_RESTART_IS_NR: bool = true;

/// x86 forks via the iret path and keeps no live trap frame pointer.
pub fn set_syscall_frame(_frame: *mut u64) {}

/// No frame is kept here (x86 enters every syscall at the task's `rsp0`).
pub fn syscall_frame() -> *mut usize {
    core::ptr::null_mut()
}

/// Nothing to do before `enter_user` after exec.
pub fn exec_resume(_entry: usize, _rsp: usize, _argc: usize, _argv: usize) {}

/// The calling thread's user registers from its syscall frame, as a forked
/// child resumes with them (result 0).
pub fn caller_regs(frame: *mut u64) -> UserRegs {
    // Use the snapshot from syscall_entry — live rbx/rbp/r12–r15 here may
    // already be Rust scratch (prologues saved the user values on the stack).
    let cpu = crate::smp::cpu_id().min(MAX_CPUS - 1);
    let c = unsafe { core::ptr::addr_of!(CPU_SYSCALL[cpu].fork).read() };
    let w = |i: usize| unsafe { *frame.add(i) };
    UserRegs {
        rip: w(SYSCALL_PC) as usize,
        rsp: w(SYSCALL_SP) as usize,
        rbx: c.rbx,
        rbp: c.rbp,
        r12: c.r12,
        r13: c.r13,
        r14: c.r14,
        r15: c.r15,
        args: [w(8), w(7), w(6), w(5), w(2), w(3)],
    }
}

/// User callee-saved regs at syscall entry (before Rust can clobber them).
/// Copied into `UserRegs` on SYS_FORK so fork-continue children resume correctly.
#[derive(Clone, Copy)]
#[repr(C)]
struct ForkCalleeSaved {
    rbx: u64,
    rbp: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
}

/// Per-CPU syscall state. `IA32_GS_BASE` points at the current CPU's element so
/// `syscall_entry` can load RSP0 without clobbering SYSCALL's RCX (user RIP).
#[derive(Clone, Copy)]
#[repr(C, align(64))]
struct CpuSyscallState {
    kernel_rsp0: usize,
    fork: ForkCalleeSaved,
    /// `syscall_entry` scratch (gs:[56]): the user rsp while the user
    /// registers are pushed and popped, so none of them is used as a temp.
    user_rsp: usize,
}

// `syscall_entry` addresses these fields as gs:[0], gs:[8..56] and gs:[56].
const _: () = {
    assert!(core::mem::offset_of!(CpuSyscallState, fork) == 8);
    assert!(core::mem::offset_of!(CpuSyscallState, user_rsp) == 56);
};

static mut CPU_SYSCALL: [CpuSyscallState; MAX_CPUS] = [
    CpuSyscallState {
        kernel_rsp0: 0,
        fork: ForkCalleeSaved {
            rbx: 0,
            rbp: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
        },
        user_rsp: 0,
    }; MAX_CPUS
];

core::arch::global_asm!(
    r#"
    .global syscall_entry
syscall_entry:
    cli
    # GS_BASE -> &CPU_SYSCALL[cpu] (set in load_percpu_gs). Stash the user
    # rsp there so no user register has to serve as a temporary.
    mov gs:[56], rsp
    mov rsp, qword ptr gs:[0]
    # Snapshot user callee-saved before any Rust prologue can reuse them.
    mov gs:[8], rbx
    mov gs:[16], rbp
    mov gs:[24], r12
    mov gs:[32], r13
    mov gs:[40], r14
    mov gs:[48], r15
    # Like Linux, a syscall changes only rax (result), rcx and r11 (sysret's
    # rip/rflags): userspace wrappers declare just those clobbered, so every
    # other register Rust may reuse is saved here and restored below. The
    # lowest five words (user rsp, rip, r8, r9, rflags) are `SyscallRegs`.
    push rdi
    push rsi
    push rdx
    push r10
    push r11          # user rflags
    push r9
    push r8
    push rcx          # user rip
    push qword ptr gs:[56] # user rsp (on this kernel stack, survives wait/yield)
    # 7th argument (and 16-byte alignment for the call): the address of the
    # block just pushed (`push rsp` stores rsp before the decrement), through
    # which the signal code redirects rip/rsp.
    push rsp
    mov r9, qword ptr [rsp + 8] # user_rsp
    mov r8, rcx       # user_rip
    mov rcx, rdx      # a2
    mov rdx, rsi      # a1
    mov rsi, rdi      # a0
    mov rdi, rax      # nr
    call {dispatch}
    # The body may have enabled IRQs (and moved CPUs); keep them off while
    # gs:[56] carries the user rsp. sysret restores IF from r11.
    cli
    add rsp, 8
    pop r10
    mov gs:[56], r10
    pop rcx
    pop r8
    pop r9
    pop r11
    pop r10
    pop rdx
    pop rsi
    pop rdi
    mov rsp, gs:[56]
    sysretq
    "#,
    dispatch = sym crate::user::syscall_dispatch,
);

unsafe extern "C" {
    fn syscall_entry();
}

/// Enable user mode on the BSP (syscall MSRs, SSE).
pub fn user_init() {
    init_syscall_msrs();
    init_user_sse();
}

/// Per-CPU user-mode enable (SSE / syscall MSRs are per-CPU).
pub fn user_ap_init() {
    init_syscall_msrs();
    init_user_sse();
}

/// newlib stdio and -O2 user code use SSE (movaps/xorps). Without OSFXSR/OSXMMEXCPT
/// and with CR0.TS set, the first SSE insn in userspace raises #NM.
fn init_user_sse() {
    const CR0_EM: u64 = 1 << 2;
    const CR0_TS: u64 = 1 << 3;
    const CR0_MP: u64 = 1 << 1;
    const CR4_OSFXSR: u64 = 1 << 9;
    const CR4_OSXMMEXCPT: u64 = 1 << 10;
    let (mut cr0, mut cr4): (u64, u64);
    unsafe {
        core::arch::asm!("mov {}, cr0", out(reg) cr0);
        core::arch::asm!("mov {}, cr4", out(reg) cr4);
        cr0 &= !(CR0_EM | CR0_TS);
        cr0 |= CR0_MP;
        cr4 |= CR4_OSFXSR | CR4_OSXMMEXCPT;
        core::arch::asm!("mov cr0, {}", in(reg) cr0);
        core::arch::asm!("mov cr4, {}", in(reg) cr4);
        // Clean x87/SSE control state so user ldmxcsr/SSE spills see default MXCSR.
        core::arch::asm!(
            "fninit",
            "ldmxcsr [rip + {mxcsr}]",
            mxcsr = sym USER_MXCSR_DEFAULT,
            options(nostack),
        );
    }
}

/// Default MXCSR (same as post-RESET): flush-to-zero off, all exceptions masked.
#[repr(C, align(16))]
struct MxcsrCell(u32);
static USER_MXCSR_DEFAULT: MxcsrCell = MxcsrCell(0x1F80);

fn init_syscall_msrs() {
    const IA32_EFER: u32 = 0xC000_0080;
    const IA32_STAR: u32 = 0xC000_0081;
    const IA32_LSTAR: u32 = 0xC000_0082;
    const IA32_FMASK: u32 = 0xC000_0084;
    const SCE: u64 = 1;
    // NXE must be set on every CPU: user stacks / MMIO maps use PTE bit 63.
    // Limine enables it on the BSP; APs that miss it #PF (RSVD) on CR3 switch.
    const NXE: u64 = 1 << 11;

    let mut efer = rdmsr(IA32_EFER);
    efer |= SCE | NXE;
    wrmsr(IA32_EFER, efer);

    let star = ((super::gdt::user_ss() as u64 - 8) << 48)
        | ((super::gdt::kernel_cs() as u64) << 32);
    wrmsr(IA32_STAR, star);
    wrmsr(IA32_LSTAR, syscall_entry as *const () as usize as u64);
    wrmsr(IA32_FMASK, 0x257fd);
}

fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") lo,
            in("edx") hi,
            options(nostack, preserves_flags),
        );
    }
}

/// Publish the running task's kernel stack top for the next syscall /
/// interrupt from user mode on this CPU.
pub fn set_kernel_stack_top(top: usize) {
    let cpu = crate::smp::cpu_id().min(MAX_CPUS - 1);
    // Keep GS_BASE coherent with cpu_id() before publishing rsp0.
    load_percpu_gs(cpu);
    unsafe {
        core::ptr::addr_of_mut!(CPU_SYSCALL[cpu].kernel_rsp0).write(top);
    }
    super::gdt::set_rsp0(top as u64);
}

/// Record which CPU a kernel stack belongs to (riscv64 only; see there).
pub fn stamp_stack_cpu(_kstack_top: usize, _cpu: usize) {}

/// The arch word of a foreign-personality signal frame: the user CS (a
/// Linux `sigcontext` carries it in `csgsfs`).
pub fn signal_arch_word() -> u64 {
    (super::gdt::user_cs() | 3) as u64
}

/// Point GS at this CPU's syscall state (x86). Called from BSP/AP interrupt init.
pub fn load_percpu_gs(cpu: usize) {
    const IA32_GS_BASE: u32 = 0xC000_0101;
    let cpu = cpu.min(MAX_CPUS - 1);
    let ptr = unsafe { core::ptr::addr_of!(CPU_SYSCALL[cpu]) as u64 };
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") IA32_GS_BASE,
            in("eax") ptr as u32,
            in("edx") (ptr >> 32) as u32,
            options(nostack, preserves_flags),
        );
    }
}

/// First entry to user mode (the aspace is already loaded). x86 passes
/// argc/argv on the initial user stack, not in registers.
pub fn enter_user(user_rip: usize, user_rsp: usize, _argc: usize, _argv: usize) -> ! {
    enter_x86(user_rip, user_rsp)
}

fn enter_x86(user_rip: usize, user_rsp: usize) -> ! {
    // Refresh per-CPU ring0 state in case this CPU never scheduled the task
    // (or GS/TSS drifted). Required before the first AP iretq into ring3.
    let ktop = crate::task::current_kernel_stack_top();
    if ktop != 0 {
        set_kernel_stack_top(ktop);
    }
    let cs = (super::gdt::user_cs() | 3) as u64;
    let ss = (super::gdt::user_ss() | 3) as u64;
    let rflags: u64 = 0x202;
    // Iret frame in memory (RIP, CS, RFLAGS, RSP, SS). Do not feed five `in(reg)`
    // operands into one asm block: LLVM can reuse a register for CS and corrupt
    // iretq (post-fork exec of large ELFs → #GP on BIOS).
    //
    // Volatile + GPR scrub: `mov rsp, frame_ptr` leaves the kernel stack
    // address (HHDM) in a GPR across iretq. Userspace then faulted on that
    // pointer (UEFI boot-mini `cat | cat`: cr2=0xffff8000… code=0x5). Same
    // discipline as enter_regs_x86 / enter_riscv64.
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

/// Start a new task (forked child or thread) in user mode with `regs` (the
/// aspace is already loaded).
pub fn enter_user_regs(regs: UserRegs) -> ! {
    enter_regs_x86(regs)
}

/// Packed resume image for `fork_iret_to_user` (global_asm). Layout must match
/// the offsets in that stub — do not reorder fields.
#[repr(C, align(16))]
struct ResumeX86 {
    rbx: u64,
    rbp: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
    /// rdi, rsi, rdx, r10, r8, r9.
    args: [u64; 6],
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

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
    # rdi -> ResumeX86. Restore the user GPRs, then the iret frame; rdi
    # (the kernel resume pointer) last. The other registers are scrubbed:
    # leftover kernel addresses across iretq became user #PF (cr2 in HHDM)
    # on UEFI pipeline fork before exec. rax is the result (0).
    mov rbx, [rdi]
    mov rbp, [rdi + 8]
    mov r12, [rdi + 16]
    mov r13, [rdi + 24]
    mov r14, [rdi + 32]
    mov r15, [rdi + 40]
    mov rsi, [rdi + 56]
    mov rdx, [rdi + 64]
    mov r10, [rdi + 72]
    mov r8, [rdi + 80]
    mov r9, [rdi + 88]
    lea rsp, [rdi + 96]
    mov rdi, [rdi + 48]
    xor rax, rax
    xor rcx, rcx
    xor r11, r11
    iretq
    "#,
    mxcsr = sym USER_MXCSR_DEFAULT,
);

unsafe extern "C" {
    fn fork_iret_to_user(resume: *const ResumeX86) -> !;
}

fn enter_regs_x86(regs: UserRegs) -> ! {
    let cs = (super::gdt::user_cs() | 3) as u64;
    let ss = (super::gdt::user_ss() | 3) as u64;
    let rflags: u64 = 0x202;
    // Resume through global_asm — not inline asm. Prior enter_regs_x86 variants
    // used black_box arrays + as_ptr() / forbidden rbx,rbp constraints; LLVM
    // could leave the child with garbage callee-saved regs. Identical #GP
    // rip/rsp across those "fixes" matches a restore that never stuck.
    // Parent returns via sysret with the same user RSP; child must get the
    // exact syscall RSP and the FORK_CALLEE snapshot of rbx/rbp/r12–r15.
    let resume = ResumeX86 {
        rbx: regs.rbx,
        rbp: regs.rbp,
        r12: regs.r12,
        r13: regs.r13,
        r14: regs.r14,
        r15: regs.r15,
        args: regs.args,
        rip: regs.rip as u64,
        cs,
        rflags,
        rsp: regs.rsp as u64,
        ss,
    };
    // Volatile write so the stores exist in memory before the asm reads them.
    let mut slot = core::mem::MaybeUninit::<ResumeX86>::uninit();
    unsafe {
        core::ptr::write_volatile(slot.as_mut_ptr(), resume);
        fork_iret_to_user(slot.as_ptr());
    }
}
