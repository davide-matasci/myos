//! Usermode: nested `user/init` ELF, per-process page tables, syscalls.

mod aspace;
mod enter;
mod image;
mod syscall;
mod uaccess;
pub use aspace::*;
pub use enter::*;
use image::*;
pub use syscall::*;
pub use uaccess::*;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

#[cfg(target_arch = "riscv64")]
use crate::arch::paging;
use crate::fs;
use crate::mm;
use crate::modules::elf;
use crate::task;

/// Linux mmap prot/flags (newlib + tcc).
const PROT_READ: usize = 1;
const PROT_WRITE: usize = 2;
const PROT_EXEC: usize = 4;
const MAP_PRIVATE: usize = 0x02;
const MAP_FIXED: usize = 0x10;
const MAP_ANON: usize = 0x20;
/// Anonymous mmap region after the brk heap.
const MMAP_AREA_PAGES: usize = 256;
pub const PAGE: usize = 4096;
/// User stack below the heap. x86_64 uses 1 MiB; AArch64 uses 512 KiB.
/// AArch64 user maps spill into L2[1+] when code+stack+heap exceed 512 pages.
#[cfg(target_arch = "aarch64")]
pub const USER_STACK_PAGES: usize = 128;
#[cfg(not(target_arch = "aarch64"))]
pub const USER_STACK_PAGES: usize = 256;
/// Per-process brk heap capacity (mapped on demand by `sys_brk`).
/// - x86_64: TLS arena stays in ELF BSS; git Phase-1 object writes need ≥1 MiB
///   and GNU make's os-test parsing xmallocs well past the old 512-page cap
///   ("make: *** virtual memory exhausted"), so allow 4096 pages (16 MiB).
/// - aarch64/riscv64: TLS arena is a 2 MiB brk allocation — window must fit
///   that plus headroom. aarch64 stays under the 4×512-page L2 spill cap
///   (image + stack + heap ≲ 2048 pages from USER_BASE).
#[cfg(target_arch = "aarch64")]
const HEAP_PAGES: usize = 768;
#[cfg(target_arch = "riscv64")]
const HEAP_PAGES: usize = 1024;
#[cfg(target_arch = "x86_64")]
const HEAP_PAGES: usize = 4096;
/// Cap for fresh `load_user_elf` (init + typical programs) and on-stack frame arrays.
/// Keep modest: bumping this also sizes `[u64; N]` on the task stack and used to
/// force `elf_scratch_mut` to grab N contiguous frames before init could run.
const MAX_INIT_PAGES: usize = 1024;
/// Cap for in-place `expand_user_elf` of larger bootfs ELFs (uutils / ripgrep / git).
/// Must stay within QEMU RAM given leaked post-exec frames (x86 CI is 1024 MiB).
/// Full feat_common_core (~2.4k pages) OOMed; ship a smaller multicall instead.
/// Phase-1 git static-pie spans ~1080 pages (BSS included); keep ≤1152 so
/// aarch64 image+stack+heap stays within the 4×512 L2 spill cap (2048 pages).
const MAX_EXPAND_PAGES: usize = 1152;
/// Largest image we may map, fork-copy, or stage in ELF scratch.
const MAX_ELF_PAGES: usize = if MAX_EXPAND_PAGES > MAX_INIT_PAGES {
    MAX_EXPAND_PAGES
} else {
    MAX_INIT_PAGES
};
/// In-place `reload_user_elf` scratch and mapping cap (sbase-cat scale).
const MAX_RELOAD_PAGES: usize = 40;
/// Minimum code pages reserved below the user stack so post-fork `exec` can
/// `reload_user_elf` the largest newlib/sbase ELFs (today `sbase-cat`).
const USER_EXEC_RELOAD_PAGES: usize = 36;
const MAX_PATH: usize = 256;
const MAX_ARGC: usize = 16;
const MAX_ARG_LEN: usize = 128;
const MAX_ENVC: usize = 32;
const MAX_ENV_LEN: usize = 128;
const SYSERR: usize = usize::MAX;
/// open(2) of a FIFO for writing with O_NONBLOCK and no reader (ENXIO).
const SYSERR_ENXIO: usize = usize::MAX - 2;

const INIT_ELF: &[u8] = include_bytes!(env!("USER_INIT_PATH"));

static USERS_ALIVE: AtomicUsize = AtomicUsize::new(0);
static DID_SPAWN: AtomicBool = AtomicBool::new(false);

#[cfg(target_arch = "x86_64")]
const DEFAULT_USER_BASE: u64 = 0x0000_0080_0000_0000; // PML4[1]
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
const DEFAULT_USER_BASE: u64 = 0x4000_0000; // Sv39 root[1] / L1[1] on QEMU virt RAM

static USER_BASE: AtomicU64 = AtomicU64::new(DEFAULT_USER_BASE);

/// User callee-saved regs at syscall entry (before Rust can clobber them).
/// Copied into `ForkRegs` on SYS_FORK so fork-continue children resume correctly.
#[cfg(target_arch = "x86_64")]
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
#[cfg(target_arch = "x86_64")]
#[derive(Clone, Copy)]
#[repr(C, align(64))]
struct CpuSyscallState {
    kernel_rsp0: usize,
    fork: ForkCalleeSaved,
}

#[cfg(target_arch = "x86_64")]
static mut CPU_SYSCALL: [CpuSyscallState; crate::smp::MAX_CPUS] = [
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
    }; crate::smp::MAX_CPUS
];

#[cfg(target_arch = "riscv64")]
static mut KERNEL_SSCRATCH: usize = 0;

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    r#"
    .global syscall_entry
syscall_entry:
    cli
    mov r10, rsp
    # GS_BASE -> &CPU_SYSCALL[cpu] (set in load_percpu_gs)
    mov rsp, qword ptr gs:[0]
    # Snapshot user callee-saved before any Rust prologue can reuse them.
    mov gs:[8], rbx
    mov gs:[16], rbp
    mov gs:[24], r12
    mov gs:[32], r13
    mov gs:[40], r14
    mov gs:[48], r15
    push r11
    push r9
    push r8
    push rcx          # user rip
    push r10          # user rsp (on this kernel stack, survives wait/yield)
    push rax          # nr; 16-byte align for call
    mov r9, r10       # user_rsp
    mov r8, rcx       # user_rip
    mov rcx, rdx      # a2
    mov rdx, rsi      # a1
    mov rsi, rdi      # a0
    mov rdi, rax      # nr
    call {dispatch}
    add rsp, 8
    pop r10
    pop rcx
    pop r8
    pop r9
    pop r11
    mov rsp, r10
    sysretq
    "#,
    dispatch = sym syscall_dispatch,
);

#[cfg(target_arch = "x86_64")]
unsafe extern "C" {
    fn syscall_entry();
}

pub fn init() {
    #[cfg(target_arch = "x86_64")]
    {
        init_syscall_msrs();
        init_user_sse();
    }
    #[cfg(target_arch = "aarch64")]
    init_user_fp();
}

/// Per-CPU user-mode enable (SSE / FP / syscall MSRs are per-hart).
pub fn ap_init() {
    #[cfg(target_arch = "x86_64")]
    {
        init_syscall_msrs();
        init_user_sse();
    }
    #[cfg(target_arch = "aarch64")]
    init_user_fp();
}

/// newlib stdio and -O2 user code use SSE (movaps/xorps). Without OSFXSR/OSXMMEXCPT
/// and with CR0.TS set, the first SSE insn in userspace raises #NM.
#[cfg(target_arch = "x86_64")]
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
#[cfg(target_arch = "x86_64")]
#[repr(C, align(16))]
struct MxcsrCell(u32);
#[cfg(target_arch = "x86_64")]
static USER_MXCSR_DEFAULT: MxcsrCell = MxcsrCell(0x1F80);

/// newlib stdio init uses NEON (movi v0.2d). With CPACR_EL1.FPEN=0, EL0 traps on SIMD.
#[cfg(target_arch = "aarch64")]
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

#[cfg(target_arch = "x86_64")]
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

    let star = ((crate::arch::gdt::user_ss() as u64 - 8) << 48)
        | ((crate::arch::gdt::kernel_cs() as u64) << 32);
    wrmsr(IA32_STAR, star);
    wrmsr(IA32_LSTAR, syscall_entry as *const () as usize as u64);
    wrmsr(IA32_FMASK, 0x257fd);
}

#[cfg(target_arch = "x86_64")]
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

#[cfg(target_arch = "x86_64")]
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

/// Load the nested `user/init` ELF at USER_BASE and spawn one process.
pub fn spawn_init() {
    let base = pick_user_base();
    USER_BASE.store(base, Ordering::SeqCst);
    let (aspace, entry, span, off) = load_user_elf(INIT_ELF).expect("init ELF");
    let (rsp, argv) = build_argv_stack(aspace, base, off, &[], &[]).expect("init stack");
    task::spawn_user(aspace, entry, rsp, base, span, off, 0, argv);
    USERS_ALIVE.fetch_add(1, Ordering::SeqCst);
    DID_SPAWN.store(true, Ordering::SeqCst);
}

/// Scratch for `load_user_elf` / `reload_user_elf` / `expand_user_elf`.
///
/// Backed by bump-allocator frames (HHDM-contiguous), not kernel `.bss`.
/// Putting `MAX_ELF_PAGES` pages in BSS grew the Limine-loaded image by ~1.5
/// MiB and let the frame bump walk into the kernel physical range on AArch64.
///
/// Allocate only as many contiguous frames as this call needs (grow on demand
/// up to [`MAX_ELF_PAGES`]). Always grabbing the max made UEFI init fail when
/// `MAX_INIT_PAGES` was raised to 3072 for feat_common_core uutils.
const ELF_SCRATCH_BYTES: usize = MAX_ELF_PAGES * PAGE;
#[cfg(target_arch = "aarch64")]
const AARCH64_USER_L3_PAGES: usize = 512;

pub fn both_exited() -> bool {
    DID_SPAWN.load(Ordering::SeqCst) && USERS_ALIVE.load(Ordering::SeqCst) == 0
}

pub fn note_exit() {
    USERS_ALIVE.fetch_sub(1, Ordering::SeqCst);
}

pub fn note_fork() {
    USERS_ALIVE.fetch_add(1, Ordering::SeqCst);
}

pub fn set_kernel_rsp0(top: usize) {
    #[cfg(not(target_arch = "aarch64"))]
    let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
    #[cfg(target_arch = "x86_64")]
    {
        // Keep GS_BASE coherent with cpu_id() before publishing rsp0.
        load_percpu_gs(cpu);
        unsafe {
            core::ptr::addr_of_mut!(CPU_SYSCALL[cpu].kernel_rsp0).write(top);
        }
    }
    #[cfg(target_arch = "riscv64")]
    {
        // Keep stack footer + static coherent. Do NOT write the sscratch CSR
        // here: it must stay 0 while the hart runs in S-mode (trap_vector
        // reads non-zero as "trapped from U-mode"). Every sret to U-mode arms
        // it with the kernel stack top itself.
        task::stamp_stack_cpu(top, cpu);
        unsafe {
            core::ptr::addr_of_mut!(KERNEL_SSCRATCH).write(top);
        }
    }
    let _ = top;
}

/// Point GS at this CPU's syscall state (x86). Called from BSP/AP interrupt init.
#[cfg(target_arch = "x86_64")]
pub fn load_percpu_gs(cpu: usize) {
    const IA32_GS_BASE: u32 = 0xC000_0101;
    let cpu = cpu.min(crate::smp::MAX_CPUS - 1);
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

#[cfg(target_arch = "riscv64")]
const USER_SSTATUS: u64 = (2 << 32) | (1 << 5); // UXL=64-bit user, SPIE, SPP=0

const S_IFDIR: u32 = 0o040000;
