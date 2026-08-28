//! Usermode: nested `user/init` ELF, per-process page tables, syscalls.

use alloc::alloc::{alloc_zeroed, dealloc, Layout};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::fs;
use crate::mm;
use crate::modules::elf;
use crate::task;

const SYS_WRITE: usize = 0;
const SYS_EXIT: usize = 1;
const SYS_OPEN: usize = 2;
const SYS_READ: usize = 3;
const SYS_CLOSE: usize = 4;
const SYS_EXEC: usize = 5;
const SYS_FORK: usize = 6;
const SYS_WAIT: usize = 7;
const SYS_LISTDIR: usize = 8;
const SYS_BRK: usize = 9;
pub const PAGE: usize = 4096;
const HEAP_PAGES: usize = 64;
const MAX_INIT_PAGES: usize = 32;
const MAX_PATH: usize = 64;
const MAX_ARGC: usize = 16;
const MAX_ARG_LEN: usize = 128;
const SYSERR: usize = usize::MAX;

const INIT_ELF: &[u8] = include_bytes!(env!("USER_INIT_PATH"));

static USERS_ALIVE: AtomicUsize = AtomicUsize::new(0);
static DID_SPAWN: AtomicBool = AtomicBool::new(false);

#[cfg(target_arch = "x86_64")]
const DEFAULT_USER_BASE: u64 = 0x0000_0080_0000_0000; // PML4[1]
#[cfg(target_arch = "aarch64")]
const DEFAULT_USER_BASE: u64 = 0x4000_0000; // L1[1] on QEMU virt RAM

static USER_BASE: AtomicU64 = AtomicU64::new(DEFAULT_USER_BASE);

#[cfg(target_arch = "x86_64")]
static mut KERNEL_RSP0: usize = 0;

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    r#"
    .global syscall_entry
syscall_entry:
    cli
    mov r10, rsp
    mov rsp, [rip + {kernel_rsp0}]
    push r11
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
    pop r11
    mov rsp, r10
    sysretq
    "#,
    kernel_rsp0 = sym KERNEL_RSP0,
    dispatch = sym syscall_dispatch,
);

#[cfg(target_arch = "x86_64")]
unsafe extern "C" {
    fn syscall_entry();
}

pub fn init() {
    #[cfg(target_arch = "x86_64")]
    init_syscall_msrs();
}

#[cfg(target_arch = "x86_64")]
fn init_syscall_msrs() {
    const IA32_EFER: u32 = 0xC000_0080;
    const IA32_STAR: u32 = 0xC000_0081;
    const IA32_LSTAR: u32 = 0xC000_0082;
    const IA32_FMASK: u32 = 0xC000_0084;
    const SCE: u64 = 1;

    let mut efer = rdmsr(IA32_EFER);
    efer |= SCE;
    wrmsr(IA32_EFER, efer);

    let star = ((crate::arch::gdt::user_ss() as u64 - 8) << 48)
        | ((crate::arch::gdt::kernel_cs() as u64) << 32);
    wrmsr(IA32_STAR, star);
    wrmsr(IA32_LSTAR, syscall_entry as usize as u64);
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
            options(nomem, nostack, preserves_flags),
        );
    }
}

/// Load the nested `user/init` ELF at USER_BASE and spawn one process.
pub fn spawn_init() {
    let base = pick_user_base();
    USER_BASE.store(base, Ordering::SeqCst);
    let (aspace, entry, span, off) = load_user_elf(INIT_ELF).expect("init ELF");
    let (rsp, argv) = build_argv_stack(aspace, base, off, &[]).expect("init stack");
    task::spawn_user(aspace, entry, rsp, base, span, off, 0, argv);
    USERS_ALIVE.fetch_add(1, Ordering::SeqCst);
    DID_SPAWN.store(true, Ordering::SeqCst);
}

/// Realize `bytes` at USER_BASE: frames, stack page, aspace.
/// Old frames are leaked on exec (ok).
fn load_user_elf(bytes: &[u8]) -> Option<(u64, usize, usize, u64)> {
    let info = elf::image_span(bytes).ok()?;
    let n_pages = info.span.div_ceil(PAGE);
    if n_pages == 0 || n_pages > MAX_INIT_PAGES {
        return None;
    }

    let base = USER_BASE.load(Ordering::SeqCst);
    let stack_off = (n_pages * PAGE) as u64;

    let layout = Layout::from_size_align(info.span.max(1), PAGE).ok()?;
    let buf = unsafe { alloc_zeroed(layout) };
    if buf.is_null() {
        return None;
    }
    let load_bias = base - info.min_vaddr;
    let entry = match elf::realize(bytes, buf, load_bias) {
        Ok(e) => e,
        Err(_) => {
            unsafe { dealloc(buf, layout) };
            return None;
        }
    };

    let mut frames = [0u64; MAX_INIT_PAGES];
    for i in 0..n_pages {
        frames[i] = mm::alloc_frame();
        let off = i * PAGE;
        let len = core::cmp::min(PAGE, info.span - off);
        unsafe {
            core::ptr::copy_nonoverlapping(buf.add(off), mm::hhdm(frames[i]), len);
        }
        sync_icache(mm::hhdm(frames[i]) as usize, len);
    }
    unsafe { dealloc(buf, layout) };

    let stack = mm::alloc_frame();
    let aspace = create_aspace(&frames[..n_pages], stack, base, stack_off);
    Some((aspace, entry as usize, info.span, stack_off))
}

/// SysV-style user stack: `[argc][argv…][NULL][strings]`, 16-byte aligned.
/// Writes through the user aspace page tables (kernel cannot dereference user VAs).
fn build_argv_stack(aspace: u64, user_base: u64, stack_off: u64, args: &[&[u8]]) -> Option<(usize, usize)> {
    if args.len() > MAX_ARGC {
        return None;
    }
    let stack_top = (user_base + stack_off + PAGE as u64) as usize;
    let stack_bot = (user_base + stack_off) as usize;
    let mut sp = stack_top;
    let mut ptrs = [0usize; MAX_ARGC];
    for (i, arg) in args.iter().enumerate() {
        if arg.len() > MAX_ARG_LEN {
            return None;
        }
        let slen = arg.len() + 1;
        sp = sp.checked_sub(slen)?;
        if sp < stack_bot {
            return None;
        }
        if !write_user_bytes(aspace, sp, arg) {
            return None;
        }
        if !write_user_byte(aspace, sp + arg.len(), 0) {
            return None;
        }
        ptrs[i] = sp;
    }
    sp &= !15;
    sp = sp.checked_sub(8)?;
    if sp < stack_bot {
        return None;
    }
    if !write_user_usize(aspace, sp, 0) {
        return None;
    }
    for i in (0..args.len()).rev() {
        sp = sp.checked_sub(8)?;
        if sp < stack_bot {
            return None;
        }
        if !write_user_usize(aspace, sp, ptrs[i]) {
            return None;
        }
    }
    sp = sp.checked_sub(8)?;
    if sp < stack_bot {
        return None;
    }
    if !write_user_usize(aspace, sp, args.len()) {
        return None;
    }
    Some((sp, sp + core::mem::size_of::<usize>()))
}

fn write_user_byte(aspace: u64, va: usize, byte: u8) -> bool {
    let page = va & !0xfff;
    let off = va & 0xfff;
    let Some(phys) = virt_to_phys(aspace, page as u64) else {
        return false;
    };
    unsafe {
        *mm::hhdm(phys).add(off) = byte;
    }
    true
}

fn write_user_bytes(aspace: u64, va: usize, src: &[u8]) -> bool {
    for (i, &b) in src.iter().enumerate() {
        if !write_user_byte(aspace, va + i, b) {
            return false;
        }
    }
    true
}

fn write_user_usize(aspace: u64, va: usize, val: usize) -> bool {
    write_user_bytes(aspace, va, &val.to_le_bytes())
}

/// Copy this process's user code+stack+heap pages into a new aspace at the same VA.
pub fn copy_user_aspace(base: u64, span: usize, stack_off: u64, brk_cur: u64) -> Option<u64> {
    let n_pages = span.div_ceil(PAGE);
    if n_pages == 0 || n_pages > MAX_INIT_PAGES {
        return None;
    }
    let src = task::current_aspace();
    if src == 0 {
        return None;
    }
    let mut frames = [0u64; MAX_INIT_PAGES];
    for i in 0..n_pages {
        let va = base + (i * PAGE) as u64;
        let phys = virt_to_phys(src, va)?;
        frames[i] = mm::alloc_frame();
        unsafe {
            core::ptr::copy_nonoverlapping(mm::hhdm(phys), mm::hhdm(frames[i]), PAGE);
        }
        sync_icache(mm::hhdm(frames[i]) as usize, PAGE);
    }
    let stack_va = base + stack_off;
    let stack_phys = virt_to_phys(src, stack_va)?;
    let stack = mm::alloc_frame();
    unsafe {
        core::ptr::copy_nonoverlapping(mm::hhdm(stack_phys), mm::hhdm(stack), PAGE);
    }
    let aspace = create_aspace(&frames[..n_pages], stack, base, stack_off);
    let heap_base = heap_base_va(base, stack_off);
    let heap_end = align_up_usize(brk_cur as usize, PAGE);
    let mut va = heap_base as usize;
    while va < heap_end {
        if virt_to_phys(src, va as u64).is_some() {
            let phys = mm::alloc_frame();
            unsafe {
                core::ptr::copy_nonoverlapping(
                    mm::hhdm(virt_to_phys(src, va as u64)?),
                    mm::hhdm(phys),
                    PAGE,
                );
            }
            map_heap_page(aspace, va as u64, phys);
        }
        va += PAGE;
    }
    Some(aspace)
}

fn heap_base_va(base: u64, stack_off: u64) -> u64 {
    base + stack_off + PAGE as u64
}

fn heap_limit_va(base: u64, stack_off: u64) -> u64 {
    heap_base_va(base, stack_off) + (HEAP_PAGES * PAGE) as u64
}

fn align_up_usize(v: usize, align: usize) -> usize {
    (v + align - 1) & !(align - 1)
}

fn virt_to_phys(aspace: u64, va: u64) -> Option<u64> {
    #[cfg(target_arch = "x86_64")]
    {
        virt_to_phys_x86(aspace, va)
    }
    #[cfg(target_arch = "aarch64")]
    {
        virt_to_phys_aarch64(aspace, va)
    }
}

#[cfg(target_arch = "x86_64")]
fn virt_to_phys_x86(pml4_phys: u64, va: u64) -> Option<u64> {
    const PRESENT: u64 = 1;
    const HUGE: u64 = 1 << 7;
    let i4 = ((va >> 39) & 0x1ff) as usize;
    let i3 = ((va >> 30) & 0x1ff) as usize;
    let i2 = ((va >> 21) & 0x1ff) as usize;
    let i1 = ((va >> 12) & 0x1ff) as usize;
    unsafe {
        let pml4 = &*mm::table(pml4_phys);
        if pml4[i4] & PRESENT == 0 {
            return None;
        }
        let pdpt = &*mm::table(pml4[i4]);
        if pdpt[i3] & PRESENT == 0 || pdpt[i3] & HUGE != 0 {
            return None;
        }
        let pd = &*mm::table(pdpt[i3]);
        if pd[i2] & PRESENT == 0 || pd[i2] & HUGE != 0 {
            return None;
        }
        let pt = &*mm::table(pd[i2]);
        if pt[i1] & PRESENT == 0 {
            return None;
        }
        Some(pt[i1] & 0x000f_ffff_ffff_f000)
    }
}

#[cfg(target_arch = "aarch64")]
fn virt_to_phys_aarch64(l0_phys: u64, va: u64) -> Option<u64> {
    const PA: u64 = 0x0000_FFFF_FFFF_F000;
    // USER_BASE 0x4000_0000 → L0[0], L1[1], L2[0], L3[page]
    let i3 = ((va >> 12) & 0x1ff) as usize;
    unsafe {
        let l0 = &*mm::table(l0_phys);
        let l1_phys = l0[0] & PA;
        if l1_phys == 0 {
            return None;
        }
        let l1 = &*mm::table(l1_phys);
        let l2_phys = l1[1] & PA;
        if l2_phys == 0 {
            return None;
        }
        let l2 = &*mm::table(l2_phys);
        let l3_phys = l2[0] & PA;
        if l3_phys == 0 {
            return None;
        }
        let l3 = &*mm::table(l3_phys);
        let pte = l3[i3];
        if pte & 0b11 != 0b11 {
            return None;
        }
        Some(pte & PA)
    }
}

fn pick_user_base() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let src = task::kernel_aspace() & !0xfff;
        let pml4 = unsafe { &*mm::table(src) };
        if pml4[1] == 0 {
            return DEFAULT_USER_BASE;
        }
        for i in 1..256 {
            if pml4[i] == 0 {
                return (i as u64) << 39;
            }
        }
        panic!("no free PML4 slot for user");
    }
    #[cfg(target_arch = "aarch64")]
    {
        DEFAULT_USER_BASE
    }
}

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
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::ptr::addr_of_mut!(KERNEL_RSP0).write(top);
    }
    let _ = top;
}

pub fn read_aspace() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let c: u64;
        core::arch::asm!(
            "mov {c}, cr3",
            c = out(reg) c,
            options(nomem, nostack, preserves_flags)
        );
        c
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let t: u64;
        core::arch::asm!(
            "mrs {t}, ttbr0_el1",
            t = out(reg) t,
            options(nomem, nostack, preserves_flags)
        );
        t
    }
}

pub fn switch_aspace(aspace: u64) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!(
            "mov cr3, {a}",
            a = in(reg) aspace,
            options(nostack, preserves_flags)
        );
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!(
            "msr ttbr0_el1, {a}",
            "dsb sy",
            a = in(reg) aspace,
            options(nostack),
        );
        if current_el() >= 2 {
            core::arch::asm!("tlbi alle2is", options(nostack));
        } else {
            core::arch::asm!("tlbi vmalle1", options(nostack));
        }
        core::arch::asm!("dsb sy; isb", options(nostack));
    }
}

#[cfg(target_arch = "aarch64")]
fn current_el() -> u64 {
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
    #[cfg(target_arch = "x86_64")]
    enter_x86(user_rip, user_rsp);
    #[cfg(target_arch = "aarch64")]
    enter_aarch64(user_rip, user_rsp, user_argc, user_argv);
}

#[cfg(target_arch = "x86_64")]
fn enter_x86(user_rip: usize, user_rsp: usize) -> ! {
    let cs = (crate::arch::gdt::user_cs() | 3) as u64;
    let ss = (crate::arch::gdt::user_ss() | 3) as u64;
    let rflags: u64 = 0x202;
    unsafe {
        core::arch::asm!(
            "cli",
            "push {uss}",
            "push {ursp}",
            "push {rf}",
            "push {ucs}",
            "push {urip}",
            "iretq",
            uss = in(reg) ss,
            ursp = in(reg) user_rsp,
            rf = in(reg) rflags,
            ucs = in(reg) cs,
            urip = in(reg) user_rip,
            in("rax") 0u64,
            options(noreturn),
        );
    }
}

#[cfg(target_arch = "aarch64")]
fn enter_aarch64(user_rip: usize, user_rsp: usize, user_argc: usize, user_argv: usize) -> ! {
    // Locals + ELR/SP before argc/argv: LLVM may assign rip/argc to the same
    // register and reorder mov before msr (CI: exec eret with elr=0).
    let rip = user_rip;
    let rsp = user_rsp;
    let argc = user_argc;
    let argv = user_argv;
    unsafe {
        core::arch::asm!(
            "msr elr_el1, {rip}",
            "msr sp_el0, {rsp}",
            "msr spsr_el1, xzr",
            rip = in(reg) rip,
            rsp = in(reg) rsp,
            options(nostack, preserves_flags),
        );
        core::arch::asm!(
            "mov x0, {argc}",
            "mov x1, {argv}",
            "isb",
            "eret",
            argc = in(reg) argc,
            argv = in(reg) argv,
            options(noreturn, nostack),
        );
    }
}

/// Exec from a syscall: patch the exception frame and return through lower_sync
/// instead of eret inside the handler (same register-reuse hazard as enter_fork).
#[cfg(target_arch = "aarch64")]
fn resume_exec_via_syscall_frame(
    entry: usize,
    rsp: usize,
    argc: usize,
    argv: usize,
) -> Option<usize> {
    let frame = unsafe { SYSCALL_FRAME };
    if frame.is_null() {
        return None;
    }
    unsafe {
        // lower_sync frame: elr/spsr at 16*16, sp_el0 at 16*17.
        *frame.add(32) = entry as u64;
        *frame.add(34) = rsp as u64;
        *frame.add(1) = argv as u64;
    }
    Some(argc)
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
}

#[cfg(target_arch = "x86_64")]
fn enter_fork_x86(regs: task::ForkRegs) -> ! {
    let cs = (crate::arch::gdt::user_cs() | 3) as u64;
    let ss = (crate::arch::gdt::user_ss() | 3) as u64;
    let rflags: u64 = 0x202;
    // Iret frame in memory (RIP, CS, RFLAGS, RSP, SS). Do not feed eleven
    // `in(reg)` operands into one asm block: LLVM reused rbx for both user CS
    // and ForkRegs.rbx, then pushed 0 as CS → #GP(0) on iretq (CI #121).
    let frame = [
        regs.rip as u64,
        cs,
        rflags,
        regs.rsp as u64,
        ss,
    ];
    let r = core::ptr::addr_of!(regs);
    unsafe {
        core::arch::asm!(
            "cli",
            "mov rbx, [{r} + 16]",
            "mov rbp, [{r} + 24]",
            "mov r12, [{r} + 32]",
            "mov r13, [{r} + 40]",
            "mov r14, [{r} + 48]",
            "mov r15, [{r} + 56]",
            "mov rsp, {f}",
            "xor rax, rax",
            "iretq",
            r = in(reg) r,
            f = in(reg) frame.as_ptr(),
            options(noreturn),
        );
    }
}

#[cfg(target_arch = "aarch64")]
fn enter_fork_aarch64(regs: task::ForkRegs) -> ! {
    // GPR image in a stack local; reload via x0 as base. Program ELR/SP_EL0
    // *before* the ldrs — otherwise LLVM's in(reg) for rip/rsp can live in
    // x1..x30 and get overwritten (elr left as ~0xfff… → PC alignment abort).
    let mut img = regs.x;
    img[0] = 0;
    let base = img.as_ptr();
    let rip = regs.rip;
    let rsp = regs.rsp;
    unsafe {
        core::arch::asm!(
            "msr elr_el1, {rip}",
            "msr sp_el0, {rsp}",
            "msr spsr_el1, xzr",
            "mov x0, {b}",
            "ldr x1, [x0, #8]",
            "ldr x2, [x0, #16]",
            "ldr x3, [x0, #24]",
            "ldr x4, [x0, #32]",
            "ldr x5, [x0, #40]",
            "ldr x6, [x0, #48]",
            "ldr x7, [x0, #56]",
            "ldr x8, [x0, #64]",
            "ldr x9, [x0, #72]",
            "ldr x10, [x0, #80]",
            "ldr x11, [x0, #88]",
            "ldr x12, [x0, #96]",
            "ldr x13, [x0, #104]",
            "ldr x14, [x0, #112]",
            "ldr x15, [x0, #120]",
            "ldr x16, [x0, #128]",
            "ldr x17, [x0, #136]",
            "ldr x18, [x0, #144]",
            "ldr x19, [x0, #152]",
            "ldr x20, [x0, #160]",
            "ldr x21, [x0, #168]",
            "ldr x22, [x0, #176]",
            "ldr x23, [x0, #184]",
            "ldr x24, [x0, #192]",
            "ldr x25, [x0, #200]",
            "ldr x26, [x0, #208]",
            "ldr x27, [x0, #216]",
            "ldr x28, [x0, #224]",
            "ldr x29, [x0, #232]",
            "ldr x30, [x0, #240]",
            "mov x0, xzr",
            "isb",
            "eret",
            b = in(reg) base,
            rip = in(reg) rip,
            rsp = in(reg) rsp,
            options(noreturn),
        );
    }
}

#[cfg(target_arch = "aarch64")]
static mut SYSCALL_FRAME: *mut u64 = core::ptr::null_mut();

#[cfg(target_arch = "aarch64")]
pub fn set_syscall_frame(frame: *mut u64) {
    unsafe { SYSCALL_FRAME = frame; }
}

#[unsafe(no_mangle)]
pub extern "C" fn syscall_dispatch(
    nr: usize,
    a0: usize,
    a1: usize,
    a2: usize,
    user_rip: usize,
    user_rsp: usize,
) -> usize {
    task::save_user_context(user_rip, user_rsp);
    match nr {
        SYS_WRITE => sys_write(a0, a1),
        SYS_EXIT => task::die(),
        SYS_OPEN => sys_open(a0, a1),
        SYS_READ => sys_read(a0, a1, a2),
        SYS_CLOSE => sys_close(a0),
        SYS_EXEC => sys_exec(a0, a1, a2),
        SYS_FORK => sys_fork(user_rip, user_rsp),
        SYS_WAIT => sys_wait(),
        SYS_LISTDIR => sys_listdir(a0, a1),
        SYS_BRK => sys_brk(a0),
        _ => SYSERR,
    }
}

fn sys_write(ptr: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let n = len.min(128);
    if !user_range_ok(ptr, n) {
        return SYSERR;
    }
    let mut buf = [0u8; 128];
    unsafe {
        core::ptr::copy_nonoverlapping(ptr as *const u8, buf.as_mut_ptr(), n);
    }
    task::print_bytes(&buf[..n]);
    len
}

fn copy_user_path(ptr: usize, len: usize) -> Option<[u8; MAX_PATH]> {
    if len == 0 || len > MAX_PATH {
        return None;
    }
    if !user_range_ok(ptr, len) {
        return None;
    }
    let mut buf = [0u8; MAX_PATH];
    unsafe {
        core::ptr::copy_nonoverlapping(ptr as *const u8, buf.as_mut_ptr(), len);
    }
    Some(buf)
}

fn sys_open(ptr: usize, path_len: usize) -> usize {
    let Some(buf) = copy_user_path(ptr, path_len) else {
        return SYSERR;
    };
    let Ok(path) = core::str::from_utf8(&buf[..path_len]) else {
        return SYSERR;
    };
    let Some(data) = fs::lookup(path) else {
        return SYSERR;
    };
    match task::fd_open(data) {
        Some(fd) => fd,
        None => SYSERR,
    }
}

fn sys_read(fd: usize, buf: usize, len: usize) -> usize {
    task::fd_read(fd, buf, len, user_range_ok)
}

fn sys_close(fd: usize) -> usize {
    if task::fd_close(fd) {
        0
    } else {
        SYSERR
    }
}

fn sys_exec(ptr: usize, path_len: usize, args_ptr: usize) -> usize {
    let Some(buf) = copy_user_path(ptr, path_len) else {
        return SYSERR;
    };
    let Ok(path) = core::str::from_utf8(&buf[..path_len]) else {
        return SYSERR;
    };
    let Some(bytes) = fs::lookup(path) else {
        return SYSERR;
    };
    let arg_bufs = match copy_user_exec_args(args_ptr) {
        Ok(v) => v,
        Err(()) => return SYSERR,
    };
    let arg_refs: Vec<&[u8]> = arg_bufs.iter().map(|s| s.as_slice()).collect();
    let Some((aspace, entry, span, off)) = load_user_elf(bytes) else {
        return SYSERR;
    };
    let base = USER_BASE.load(Ordering::SeqCst);
    let Some((rsp, argv)) = build_argv_stack(aspace, base, off, &arg_refs) else {
        return SYSERR;
    };
    let argc = arg_refs.len();
    task::replace_user(aspace, entry, rsp, base, span, off, argc, argv);
    #[cfg(target_arch = "aarch64")]
    if let Some(x0) = resume_exec_via_syscall_frame(entry, rsp, argc, argv) {
        return x0;
    }
    enter(entry, rsp, argc, argv);
}

fn copy_user_exec_args(args_ptr: usize) -> Result<alloc::vec::Vec<alloc::vec::Vec<u8>>, ()> {
    if args_ptr == 0 {
        return Ok(alloc::vec::Vec::new());
    }
    if !user_range_ok(args_ptr, core::mem::size_of::<usize>()) {
        return Err(());
    }
    let argc = unsafe { *(args_ptr as *const usize) };
    if argc > MAX_ARGC {
        return Err(());
    }
    let mut out = alloc::vec::Vec::with_capacity(argc);
    let pairs = args_ptr + core::mem::size_of::<usize>();
    for i in 0..argc {
        let off = pairs + i * 2 * core::mem::size_of::<usize>();
        if !user_range_ok(off, 2 * core::mem::size_of::<usize>()) {
            return Err(());
        }
        let p = unsafe { *((off) as *const usize) };
        let n = unsafe { *((off + core::mem::size_of::<usize>()) as *const usize) };
        if n > MAX_ARG_LEN {
            return Err(());
        }
        if n != 0 && !user_range_ok(p, n) {
            return Err(());
        }
        let mut v = alloc::vec::Vec::with_capacity(n);
        if n != 0 {
            v.resize(n, 0);
            unsafe {
                core::ptr::copy_nonoverlapping(p as *const u8, v.as_mut_ptr(), n);
            }
        }
        out.push(v);
    }
    Ok(out)
}

fn sys_listdir(buf: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    if !user_range_ok(buf, len) {
        return SYSERR;
    }
    let mut kbuf = [0u8; 512];
    let n = fs::listdir(&mut kbuf).min(kbuf.len()).min(len);
    unsafe {
        core::ptr::copy_nonoverlapping(kbuf.as_ptr(), buf as *mut u8, n);
    }
    n
}

fn sys_fork(user_rip: usize, user_rsp: usize) -> usize {
    #[cfg(target_arch = "x86_64")]
    let child = {
        let rbx: u64;
        let rbp: u64;
        let r12: u64;
        let r13: u64;
        let r14: u64;
        let r15: u64;
        // syscall_entry leaves user callee-saved regs in place.
        unsafe {
            core::arch::asm!(
                "mov {rbx}, rbx",
                "mov {rbp}, rbp",
                "mov {r12}, r12",
                "mov {r13}, r13",
                "mov {r14}, r14",
                "mov {r15}, r15",
                rbx = out(reg) rbx,
                rbp = out(reg) rbp,
                r12 = out(reg) r12,
                r13 = out(reg) r13,
                r14 = out(reg) r14,
                r15 = out(reg) r15,
                options(nomem, nostack, preserves_flags),
            );
        }
        task::ForkRegs {
            rip: user_rip,
            rsp: user_rsp,
            rbx,
            rbp,
            r12,
            r13,
            r14,
            r15,
        }
    };
    #[cfg(target_arch = "aarch64")]
    let child = {
        let frame = unsafe { SYSCALL_FRAME };
        if frame.is_null() {
            return SYSERR;
        }
        let mut x = [0u64; 31];
        unsafe {
            for i in 0..31 {
                x[i] = *frame.add(i);
            }
        }
        x[0] = 0; // child return value
        task::ForkRegs {
            rip: user_rip,
            rsp: user_rsp,
            x,
        }
    };
    match task::fork_current(child) {
        Some(pid) => pid,
        None => SYSERR,
    }
}

fn sys_wait() -> usize {
    task::wait_child()
}

fn sys_brk(req: usize) -> usize {
    let (base, _span, stack_off) = task::current_user_map();
    let heap_base = heap_base_va(base, stack_off) as usize;
    let heap_limit = heap_limit_va(base, stack_off) as usize;
    let cur = task::current_brk() as usize;
    if req == 0 {
        return cur;
    }
    if req < heap_base || req > heap_limit {
        return cur;
    }
    if req > cur {
        let aspace = task::current_aspace();
        let map_end = align_up_usize(req, PAGE);
        let mut va = if cur == heap_base {
            heap_base
        } else {
            align_up_usize(cur, PAGE)
        };
        while va < map_end {
            if virt_to_phys(aspace, va as u64).is_none() {
                let frame = mm::alloc_frame();
                map_heap_page(aspace, va as u64, frame);
            }
            va += PAGE;
        }
    }
    task::set_brk(req as u64);
    req
}

fn user_range_ok(ptr: usize, len: usize) -> bool {
    let (base, img, stack) = task::current_user_map();
    let base = base as usize;
    let end = match ptr.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    let stack = stack as usize;
    let in_code = ptr >= base && end <= base + img;
    let in_stack = ptr >= base + stack && end <= base + stack + PAGE;
    let heap_base = heap_base_va(base as u64, stack as u64) as usize;
    let brk = task::current_brk() as usize;
    let in_heap = brk > heap_base && ptr >= heap_base && end <= brk;
    in_code || in_stack || in_heap
}

fn create_aspace(code: &[u64], stack_phys: u64, base: u64, stack_off: u64) -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        create_aspace_x86(code, stack_phys, base, stack_off)
    }
    #[cfg(target_arch = "aarch64")]
    {
        create_aspace_aarch64(code, stack_phys, base, stack_off)
    }
}

#[cfg(target_arch = "x86_64")]
fn create_aspace_x86(code: &[u64], stack_phys: u64, base: u64, stack_off: u64) -> u64 {
    const PRESENT: u64 = 1;
    const WRITE: u64 = 1 << 1;
    const USER: u64 = 1 << 2;
    const NX: u64 = 1 << 63;

    let src = task::kernel_aspace() & !0xfff;
    let pml4_phys = mm::alloc_frame();
    unsafe {
        let src_t = &*mm::table(src);
        let dst_t = &mut *mm::table(pml4_phys);
        dst_t.copy_from_slice(src_t);
    }

    // RW so sys_read can fill PT_LOAD (user/ok MSG_BUF). Still executable.
    for (i, &phys) in code.iter().enumerate() {
        map_page_x86(
            pml4_phys,
            base + (i * PAGE) as u64,
            phys,
            PRESENT | WRITE | USER,
        );
    }
    let stack_va = base + stack_off;
    map_page_x86(
        pml4_phys,
        stack_va,
        stack_phys,
        PRESENT | WRITE | USER | NX,
    );
    pml4_phys
}

#[cfg(target_arch = "x86_64")]
fn map_heap_page(pml4_phys: u64, va: u64, pa: u64) {
    const PRESENT: u64 = 1;
    const WRITE: u64 = 1 << 1;
    const USER: u64 = 1 << 2;
    const NX: u64 = 1 << 63;
    map_page_x86(pml4_phys, va, pa, PRESENT | WRITE | USER | NX);
}

#[cfg(target_arch = "aarch64")]
fn map_heap_page(l0_phys: u64, va: u64, pa: u64) {
    const PAGE_DESC: u64 = 0b11;
    const SH_INNER: u64 = 0b11 << 8;
    const AF: u64 = 1 << 10;
    const AP_RW: u64 = 0b01 << 6;
    const UXN: u64 = 1 << 54;
    const PA_MASK: u64 = 0x0000_FFFF_FFFF_F000;
    let i3 = ((va >> 12) & 0x1ff) as usize;
    unsafe {
        let l0 = &*mm::table(l0_phys);
        let l1_phys = l0[0] & PA_MASK;
        let l1 = &*mm::table(l1_phys);
        let l2_phys = l1[1] & PA_MASK;
        let l2 = &*mm::table(l2_phys);
        let l3_phys = l2[0] & PA_MASK;
        let l3 = &mut *mm::table(l3_phys);
        l3[i3] = (pa & PA_MASK) | PAGE_DESC | SH_INNER | AP_RW | AF | UXN;
    }
}

#[cfg(target_arch = "x86_64")]
fn map_page_x86(pml4_phys: u64, va: u64, pa: u64, flags: u64) {
    const PRESENT: u64 = 1;
    const WRITE: u64 = 1 << 1;
    const USER: u64 = 1 << 2;
    const HUGE: u64 = 1 << 7;

    let i4 = ((va >> 39) & 0x1ff) as usize;
    let i3 = ((va >> 30) & 0x1ff) as usize;
    let i2 = ((va >> 21) & 0x1ff) as usize;
    let i1 = ((va >> 12) & 0x1ff) as usize;

    unsafe {
        let pml4 = &mut *mm::table(pml4_phys);
        let pdpt = ensure_user(&mut pml4[i4], PRESENT | WRITE | USER, HUGE);
        let pd = ensure_user(&mut (*pdpt)[i3], PRESENT | WRITE | USER, HUGE);
        let pt = ensure_user(&mut (*pd)[i2], PRESENT | WRITE | USER, HUGE);
        (*pt)[i1] = (pa & !0xfff) | flags;
    }
}

#[cfg(target_arch = "x86_64")]
fn ensure_user(entry: &mut u64, table_flags: u64, huge: u64) -> *mut [u64; 512] {
    if *entry & 1 != 0 {
        assert!(*entry & huge == 0, "user map: huge page in the way");
        return mm::table(*entry);
    }
    let phys = mm::alloc_frame();
    *entry = phys | table_flags;
    mm::table(phys)
}

#[cfg(target_arch = "aarch64")]
fn create_aspace_aarch64(code: &[u64], stack_phys: u64, _base: u64, stack_off: u64) -> u64 {
    const TABLE: u64 = 0b11;
    const PAGE_DESC: u64 = 0b11;
    const SH_INNER: u64 = 0b11 << 8;
    const AF: u64 = 1 << 10;
    const AP_RW: u64 = 0b01 << 6; // EL1 RW, EL0 RW
    const PXN: u64 = 1 << 53;
    const UXN: u64 = 1 << 54;
    const PA: u64 = 0x0000_FFFF_FFFF_F000;

    let k_l0 = task::kernel_aspace() & PA;
    let k_l0_t = unsafe { &*mm::table(k_l0) };
    let k_l1_phys = k_l0_t[0] & PA;
    let k_l1 = unsafe { &*mm::table(k_l1_phys) };
    let device = k_l1[0];

    let l0 = mm::alloc_frame();
    let l1 = mm::alloc_frame();
    let l2 = mm::alloc_frame();
    let l3 = mm::alloc_frame();

    unsafe {
        let l0_t = &mut *mm::table(l0);
        let l1_t = &mut *mm::table(l1);
        let l2_t = &mut *mm::table(l2);
        let l3_t = &mut *mm::table(l3);
        l0_t[0] = l1 | TABLE;
        l1_t[0] = device;
        l1_t[1] = l2 | TABLE;
        l2_t[0] = l3 | TABLE;
        // AP_RW: EL1 sys_read copies into PT_LOAD. PXN: EL1 cannot execute it.
        for (i, &phys) in code.iter().enumerate() {
            l3_t[i] = PAGE_DESC | (phys & PA) | SH_INNER | AF | AP_RW | PXN;
        }
        let stack_i = (stack_off as usize) / PAGE;
        l3_t[stack_i] = PAGE_DESC | (stack_phys & PA) | SH_INNER | AF | AP_RW | PXN | UXN;
    }
    l0
}

fn sync_icache(start: usize, size: usize) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        if size == 0 {
            return;
        }
        let mut addr = start & !63;
        let end = start + size;
        while addr < end {
            core::arch::asm!("dc cvau, {x}", x = in(reg) addr, options(nostack));
            addr += 64;
        }
        core::arch::asm!("dsb ish", options(nostack));
        addr = start & !63;
        while addr < end {
            core::arch::asm!("ic ivau, {x}", x = in(reg) addr, options(nostack));
            addr += 64;
        }
        core::arch::asm!("dsb ish; isb", options(nostack));
        // Execute VA != HHDM VA; drop the whole I-cache so EL0 sees the image.
        core::arch::asm!("ic ialluis; dsb ish; isb", options(nostack));
    }
    let _ = (start, size);
}
