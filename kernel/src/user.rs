//! Usermode: nested `user/init` ELF, per-process page tables, syscalls.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

#[cfg(target_arch = "riscv64")]
use crate::arch::paging;
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
const SYS_PIPE: usize = 10;
const SYS_DUP2: usize = 11;
pub const SYS_STAT: usize = 12;
const SYS_EXECNAME: usize = 13;
const SYS_DUPFD: usize = 14;
const SYS_CHDIR: usize = 15;
const SYS_GETCWD: usize = 16;
const SYS_MKDIR: usize = 17;
const SYS_RMDIR: usize = 18;
const SYS_UNLINK: usize = 19;
const SYS_RENAME: usize = 20;
const SYS_SYMLINK: usize = 21;
const SYS_READLINK: usize = 22;
const SYS_MMAP: usize = 23;
const SYS_MUNMAP: usize = 24;
const SYS_MPROTECT: usize = 25;
const SYS_LSEEK: usize = 26;
const SYS_MOUNT: usize = 27;

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
/// Per-process brk heap. uutils/clap init needs well over 512 KiB on x86_64.
#[cfg(target_arch = "aarch64")]
const HEAP_PAGES: usize = 256;
#[cfg(not(target_arch = "aarch64"))]
const HEAP_PAGES: usize = 256;
/// Largest PT_LOAD span we map for a fresh `load_user_elf` / fork copy.
/// release `uutils-coreutils` ≈366 pages; ripgrep+PCRE2 (opt-level=z, LTO) ≈721.
const MAX_INIT_PAGES: usize = 1024;
/// In-place `reload_user_elf` scratch and mapping cap (sbase-cat scale).
const MAX_RELOAD_PAGES: usize = 40;
/// Minimum code pages reserved below the user stack so post-fork `exec` can
/// `reload_user_elf` the largest newlib/sbase ELFs (today `sbase-cat`).
const USER_EXEC_RELOAD_PAGES: usize = 36;
const MAX_PATH: usize = 64;
const MAX_ARGC: usize = 16;
const MAX_ARG_LEN: usize = 128;
const MAX_ENVC: usize = 32;
const MAX_ENV_LEN: usize = 128;
const SYSERR: usize = usize::MAX;

const INIT_ELF: &[u8] = include_bytes!(env!("USER_INIT_PATH"));

static USERS_ALIVE: AtomicUsize = AtomicUsize::new(0);
static DID_SPAWN: AtomicBool = AtomicBool::new(false);

#[cfg(target_arch = "x86_64")]
const DEFAULT_USER_BASE: u64 = 0x0000_0080_0000_0000; // PML4[1]
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
const DEFAULT_USER_BASE: u64 = 0x4000_0000; // Sv39 root[1] / L1[1] on QEMU virt RAM

static USER_BASE: AtomicU64 = AtomicU64::new(DEFAULT_USER_BASE);

#[cfg(target_arch = "x86_64")]
static mut KERNEL_RSP0: usize = 0;

/// User callee-saved regs at syscall entry (before Rust can clobber them).
/// Copied into `ForkRegs` on SYS_FORK so fork-continue children resume correctly.
#[cfg(target_arch = "x86_64")]
#[repr(C)]
struct ForkCalleeSaved {
    rbx: u64,
    rbp: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
}

#[cfg(target_arch = "x86_64")]
static mut FORK_CALLEE: ForkCalleeSaved = ForkCalleeSaved {
    rbx: 0,
    rbp: 0,
    r12: 0,
    r13: 0,
    r14: 0,
    r15: 0,
};

#[cfg(target_arch = "riscv64")]
static mut KERNEL_SSCRATCH: usize = 0;

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    r#"
    .global syscall_entry
syscall_entry:
    cli
    mov r10, rsp
    mov rsp, [rip + {kernel_rsp0}]
    # Snapshot user callee-saved before any Rust prologue can reuse them.
    mov [rip + {fork_callee}], rbx
    mov [rip + {fork_callee} + 8], rbp
    mov [rip + {fork_callee} + 16], r12
    mov [rip + {fork_callee} + 24], r13
    mov [rip + {fork_callee} + 32], r14
    mov [rip + {fork_callee} + 40], r15
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
    kernel_rsp0 = sym KERNEL_RSP0,
    fork_callee = sym FORK_CALLEE,
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
    let (rsp, argv) = build_argv_stack(aspace, base, off, &[], &[]).expect("init stack");
    task::spawn_user(aspace, entry, rsp, base, span, off, 0, argv);
    USERS_ALIVE.fetch_add(1, Ordering::SeqCst);
    DID_SPAWN.store(true, Ordering::SeqCst);
}

/// Realize `bytes` at USER_BASE: frames, stack page, aspace.
/// Old frames are leaked on exec (ok).
fn load_user_elf(bytes: &[u8]) -> Option<(u64, usize, usize, u64)> {
    let info = elf::image_span(bytes).ok()?;
    let code_pages = info.span.div_ceil(PAGE);
    if code_pages == 0 || code_pages > MAX_INIT_PAGES {
        return None;
    }
    let n_pages = code_pages.max(USER_EXEC_RELOAD_PAGES).min(MAX_INIT_PAGES);

    let base = USER_BASE.load(Ordering::SeqCst);
    let stack_off = (n_pages * PAGE) as u64;

    if info.span > ELF_SCRATCH_BYTES {
        return None;
    }
    let buf = elf_scratch_mut(info.span)?;
    unsafe {
        core::ptr::write_bytes(buf.as_mut_ptr(), 0, info.span);
    }
    let load_bias = base - info.min_vaddr;
    let entry = match elf::realize(bytes, buf.as_mut_ptr(), load_bias) {
        Ok(e) => e,
        Err(_) => return None,
    };

    let mut frames = [0u64; MAX_INIT_PAGES];
    for i in 0..n_pages {
        frames[i] = mm::alloc_frame();
        if i < code_pages {
            let off = i * PAGE;
            let len = core::cmp::min(PAGE, info.span - off);
            unsafe {
                core::ptr::copy_nonoverlapping(buf.as_ptr().add(off), mm::hhdm(frames[i]), len);
                if len < PAGE {
                    core::ptr::write_bytes(mm::hhdm(frames[i]).add(len), 0, PAGE - len);
                }
            }
            sync_icache(mm::hhdm(frames[i]) as usize, PAGE);
        } else {
            unsafe {
                core::ptr::write_bytes(mm::hhdm(frames[i]), 0, PAGE);
            }
            sync_icache(mm::hhdm(frames[i]) as usize, PAGE);
        }
    }
    let mut stack_frames = [0u64; USER_STACK_PAGES];
    for frame in &mut stack_frames {
        *frame = mm::alloc_frame();
    }
    let aspace = create_aspace(&frames[..n_pages], &stack_frames, base, stack_off);
    map_initial_heap_pages(aspace, base, stack_off);
    let mapped_span = n_pages * PAGE;
    Some((aspace, entry as usize, mapped_span, stack_off))
}

fn map_initial_heap_pages(aspace: u64, base: u64, stack_off: u64) {
    let heap_base = heap_base_va(base, stack_off);
    for i in 0..HEAP_PAGES {
        let va = heap_base + (i * PAGE) as u64;
        let frame = reuse_or_alloc_frame(aspace, va);
        map_heap_page(aspace, va, frame);
    }
}

fn reuse_or_alloc_frame(aspace: u64, va: u64) -> u64 {
    if let Some(phys) = virt_to_phys(aspace, va) {
        phys
    } else {
        let frame = mm::alloc_frame();
        unsafe {
            core::ptr::write_bytes(mm::hhdm(frame), 0, PAGE);
        }
        frame
    }
}

/// Scratch for `load_user_elf` / `reload_user_elf` / `expand_user_elf`.
///
/// Backed by bump-allocator frames (HHDM-contiguous), not kernel `.bss`.
/// Putting `MAX_INIT_PAGES` pages in BSS grew the Limine-loaded image by ~1.5
/// MiB and let the frame bump walk into the kernel physical range on AArch64.
const ELF_SCRATCH_BYTES: usize = MAX_INIT_PAGES * PAGE;
static ELF_SCRATCH_PHYS: AtomicU64 = AtomicU64::new(0);

fn elf_scratch_mut(len: usize) -> Option<&'static mut [u8]> {
    if len == 0 || len > ELF_SCRATCH_BYTES {
        return None;
    }
    let mut phys = ELF_SCRATCH_PHYS.load(Ordering::SeqCst);
    if phys == 0 {
        let first = mm::alloc_frame();
        let mut expect = first.wrapping_add(PAGE as u64);
        for _ in 1..MAX_INIT_PAGES {
            let p = mm::alloc_frame();
            if p != expect {
                return None;
            }
            expect = expect.wrapping_add(PAGE as u64);
        }
        ELF_SCRATCH_PHYS.store(first, Ordering::SeqCst);
        phys = first;
    }
    Some(unsafe { core::slice::from_raw_parts_mut(mm::hhdm(phys), len) })
}

/// Overwrite the current user image in an existing aspace (no new frames).
/// Keeps `stack_off` so the mapped stack page stays valid. Fails if the new
/// image needs more bytes than the current mapping span.
fn reload_user_elf(
    aspace: u64,
    bytes: &[u8],
    base: u64,
    stack_off: u64,
    _mapped_span: usize,
) -> Option<(usize, usize, u64)> {
    let info = elf::image_span(bytes).ok()?;
    let image_pages = info.span.div_ceil(PAGE);
    if image_pages == 0 || image_pages > MAX_RELOAD_PAGES {
        return None;
    }
    let n_pages = image_pages
        .max(USER_EXEC_RELOAD_PAGES)
        .min(MAX_RELOAD_PAGES);
    // Code must sit below the mapped stack. Otherwise PT_LOAD pages overlap stack
    // slots (reload saw stack PTEs as "mapped" and clobbered them — heap #10).
    if n_pages * PAGE > stack_off as usize {
        return None;
    }
    for i in 0..n_pages {
        if virt_to_phys(aspace, base + (i * PAGE) as u64).is_none() {
            return None;
        }
    }
    if info.span > MAX_RELOAD_PAGES * PAGE {
        return None;
    }
    let buf = elf_scratch_mut(info.span)?;
    unsafe {
        core::ptr::write_bytes(buf.as_mut_ptr(), 0, info.span);
    }
    let load_bias = base - info.min_vaddr;
    let entry = match elf::realize(bytes, buf.as_mut_ptr(), load_bias) {
        Ok(e) => e,
        Err(_) => return None,
    };
    for i in 0..n_pages {
        let va = base + (i * PAGE) as u64;
        let Some(phys) = virt_to_phys(aspace, va) else {
            return None;
        };
        let off = i * PAGE;
        if i < image_pages {
            let len = core::cmp::min(PAGE, info.span.saturating_sub(off));
            unsafe {
                if len > 0 {
                    core::ptr::copy_nonoverlapping(buf.as_ptr().add(off), mm::hhdm(phys), len);
                }
                if len < PAGE {
                    core::ptr::write_bytes(mm::hhdm(phys).add(len), 0, PAGE - len);
                }
            }
        } else {
            unsafe {
                core::ptr::write_bytes(mm::hhdm(phys), 0, PAGE);
            }
        }
        sync_icache(mm::hhdm(phys) as usize, PAGE);
    }
    Some((entry as usize, n_pages * PAGE, stack_off))
}

/// Grow the current aspace and load a large ELF (up to [`MAX_INIT_PAGES`]).
/// Used when [`reload_user_elf`] is too small but we already have an aspace
/// (post-fork exec of release uutils ≈197 pages).
fn expand_user_elf(
    aspace: u64,
    bytes: &[u8],
    base: u64,
    _old_stack_off: u64,
) -> Option<(usize, usize, u64)> {
    let info = elf::image_span(bytes).ok()?;
    let image_pages = info.span.div_ceil(PAGE);
    if image_pages == 0 || image_pages > MAX_INIT_PAGES {
        return None;
    }
    let n_pages = image_pages.max(USER_EXEC_RELOAD_PAGES).min(MAX_INIT_PAGES);
    let new_stack_off = (n_pages * PAGE) as u64;
    if info.span > ELF_SCRATCH_BYTES {
        return None;
    }

    // Remap code/stack PTEs with correct flags. Reuse existing frames when the
    // VA is already mapped so post-fork exec of large ELFs does not leak hundreds
    // of physical pages and walk the bump allocator into the kernel image.
    for i in 0..n_pages {
        let va = base + (i * PAGE) as u64;
        let frame = reuse_or_alloc_frame(aspace, va);
        map_user_code_page(aspace, va, frame);
        sync_icache(mm::hhdm(frame) as usize, PAGE);
    }
    for i in 0..USER_STACK_PAGES {
        let va = base + new_stack_off + (i * PAGE) as u64;
        let frame = reuse_or_alloc_frame(aspace, va);
        map_user_stack_page(aspace, va, frame);
    }
    map_initial_heap_pages(aspace, base, new_stack_off);
    flush_user_tlb();

    let buf = elf_scratch_mut(info.span)?;
    unsafe {
        core::ptr::write_bytes(buf.as_mut_ptr(), 0, info.span);
    }
    let load_bias = base - info.min_vaddr;
    let entry = match elf::realize(bytes, buf.as_mut_ptr(), load_bias) {
        Ok(e) => e,
        Err(_) => return None,
    };
    for i in 0..n_pages {
        let va = base + (i * PAGE) as u64;
        let Some(phys) = virt_to_phys(aspace, va) else {
            return None;
        };
        let off = i * PAGE;
        if i < image_pages {
            let len = core::cmp::min(PAGE, info.span.saturating_sub(off));
            unsafe {
                if len > 0 {
                    core::ptr::copy_nonoverlapping(buf.as_ptr().add(off), mm::hhdm(phys), len);
                }
                if len < PAGE {
                    core::ptr::write_bytes(mm::hhdm(phys).add(len), 0, PAGE - len);
                }
            }
        } else {
            unsafe {
                core::ptr::write_bytes(mm::hhdm(phys), 0, PAGE);
            }
        }
        sync_icache(mm::hhdm(phys) as usize, PAGE);
    }
    Some((entry as usize, n_pages * PAGE, new_stack_off))
}

/// SysV-style user stack at process entry (`rsp % 16 == 0`):
/// `[argc][argv…][NULL][envp…][NULL][AT_NULL auxv][gap][strings…]`.
fn build_argv_stack(
    aspace: u64,
    user_base: u64,
    stack_off: u64,
    args: &[&[u8]],
    env: &[&[u8]],
) -> Option<(usize, usize)> {
    if args.len() > MAX_ARGC || env.len() > MAX_ENVC {
        return None;
    }
    let stack_top = (user_base + stack_off + (USER_STACK_PAGES * PAGE) as u64) as usize;
    let stack_bot = (user_base + stack_off) as usize;
    // Leave bytes below stack_top: argv strings must not end at stack_top (unmapped).
    let mut sp = stack_top.checked_sub(64)?;
    let mut arg_ptrs = [0usize; MAX_ARGC];
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
        arg_ptrs[i] = sp;
    }
    let mut env_ptrs = [0usize; MAX_ENVC];
    for (i, item) in env.iter().enumerate() {
        if item.len() > MAX_ENV_LEN {
            return None;
        }
        let slen = item.len() + 1;
        sp = sp.checked_sub(slen)?;
        if sp < stack_bot {
            return None;
        }
        if !write_user_bytes(aspace, sp, item) {
            return None;
        }
        if !write_user_byte(aspace, sp + item.len(), 0) {
            return None;
        }
        env_ptrs[i] = sp;
    }
    // Gap so the argv pointer table cannot overlap the copied strings (x86 std
    // `_start` reads argv[] immediately; a tight layout can alias string bytes).
    sp = sp.checked_sub(16)?;
    if sp < stack_bot {
        return None;
    }
    // argc + argv + NULL + envp + NULL + auxv AT_NULL (type,val).
    let words = 1 + args.len() + 1 + env.len() + 1 + 2;
    sp = sp.checked_sub(words * core::mem::size_of::<usize>())?;
    // Linux/SysV AMD64: %rsp ≡ 0 (mod 16) at _start. crt0/`call` then yields
    // callee %rsp ≡ 8. fe50282 forced ≡8 with an extra `sp -= 8`, which inverted
    // every frame (oksh pipe #GP) and required a matching dummy `push` in the
    // Rust PAL; keep ≡0 here and leave PAL without that pad.
    let pad = sp & 15;
    if pad != 0 {
        sp = sp.checked_sub(pad)?;
    }
    debug_assert_eq!(sp & 15, 0);
    if sp < stack_bot {
        return None;
    }
    let argc_sp = sp;
    if !write_user_usize(aspace, sp, args.len()) {
        return None;
    }
    sp += core::mem::size_of::<usize>();
    for i in 0..args.len() {
        if !write_user_usize(aspace, sp, arg_ptrs[i]) {
            return None;
        }
        sp += core::mem::size_of::<usize>();
    }
    if !write_user_usize(aspace, sp, 0) {
        return None;
    }
    sp += core::mem::size_of::<usize>();
    for i in 0..env.len() {
        if !write_user_usize(aspace, sp, env_ptrs[i]) {
            return None;
        }
        sp += core::mem::size_of::<usize>();
    }
    if !write_user_usize(aspace, sp, 0) {
        return None;
    }
    sp += core::mem::size_of::<usize>();
    // Minimal auxv terminator (AT_NULL). Fresh stacks are zeroed, but write it
    // explicitly so crt0/std walkers never read string bytes as aux entries.
    if !write_user_usize(aspace, sp, 0) {
        return None;
    }
    sp += core::mem::size_of::<usize>();
    if !write_user_usize(aspace, sp, 0) {
        return None;
    }
    Some((argc_sp, argc_sp + core::mem::size_of::<usize>()))
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

/// Copy bytes from the current user address space into a kernel buffer.
pub fn copy_from_user(aspace: u64, va: usize, dst: &mut [u8]) -> bool {
    read_user_bytes(aspace, va, dst)
}

/// Copy bytes from a kernel buffer into the current user address space.
pub fn copy_to_user(aspace: u64, va: usize, src: &[u8]) -> bool {
    write_user_bytes(aspace, va, src)
}

fn write_user_usize(aspace: u64, va: usize, val: usize) -> bool {
    write_user_bytes(aspace, va, &val.to_le_bytes())
}

pub fn try_read_user_u8(aspace: u64, va: usize) -> Option<u8> {
    read_user_byte(aspace, va)
}

fn read_user_byte(aspace: u64, va: usize) -> Option<u8> {
    let page = va & !0xfff;
    let off = va & 0xfff;
    let phys = virt_to_phys(aspace, page as u64)?;
    Some(unsafe { *mm::hhdm(phys).add(off) })
}

fn read_user_bytes(aspace: u64, va: usize, dst: &mut [u8]) -> bool {
    for (i, b) in dst.iter_mut().enumerate() {
        *b = match read_user_byte(aspace, va + i) {
            Some(v) => v,
            None => return false,
        };
    }
    true
}

fn read_user_usize(aspace: u64, va: usize) -> Option<usize> {
    let mut buf = [0u8; core::mem::size_of::<usize>()];
    if !read_user_bytes(aspace, va, &mut buf) {
        return None;
    }
    Some(usize::from_le_bytes(buf))
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
    let mut stack_frames = [0u64; USER_STACK_PAGES];
    for i in 0..USER_STACK_PAGES {
        let phys = virt_to_phys(src, stack_va + (i * PAGE) as u64)?;
        stack_frames[i] = mm::alloc_frame();
        unsafe {
            core::ptr::copy_nonoverlapping(mm::hhdm(phys), mm::hhdm(stack_frames[i]), PAGE);
        }
    }
    let aspace = create_aspace(&frames[..n_pages], &stack_frames, base, stack_off);
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
    copy_mmap_pages(src, aspace);
    Some(aspace)
}

fn heap_base_va(base: u64, stack_off: u64) -> u64 {
    base + stack_off + (USER_STACK_PAGES * PAGE) as u64
}

fn heap_limit_va(base: u64, stack_off: u64) -> u64 {
    heap_base_va(base, stack_off) + (HEAP_PAGES * PAGE) as u64
}

fn mmap_base_va(base: u64, stack_off: u64) -> u64 {
    heap_limit_va(base, stack_off)
}

fn mmap_limit_va(base: u64, stack_off: u64) -> u64 {
    mmap_base_va(base, stack_off) + (MMAP_AREA_PAGES * PAGE) as u64
}

fn copy_mmap_pages(src: u64, dst: u64) {
    let regions = task::mmap_regions();
    for r in regions.iter() {
        if r.pages == 0 {
            continue;
        }
        let mut va = r.va;
        let end = r.va.saturating_add(r.pages as u64 * PAGE as u64);
        while va < end {
            if let Some(phys) = virt_to_phys(src, va) {
                let frame = mm::alloc_frame();
                unsafe {
                    core::ptr::copy_nonoverlapping(mm::hhdm(phys), mm::hhdm(frame), PAGE);
                }
                if r.prot & PROT_EXEC as u32 != 0 {
                    sync_icache(mm::hhdm(frame) as usize, PAGE);
                }
                map_user_page_prot(dst, va, frame, r.prot as usize);
            }
            va += PAGE as u64;
        }
    }
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
    #[cfg(target_arch = "riscv64")]
    {
        virt_to_phys_riscv64(aspace, va)
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
const AARCH64_USER_L3_PAGES: usize = 512;

#[cfg(target_arch = "aarch64")]
fn aarch64_user_page_idx(va: u64) -> usize {
    let base = USER_BASE.load(Ordering::SeqCst);
    va.saturating_sub(base) as usize / PAGE
}

/// Return the L3 table for `page` (allocating L2/L3 spill slots as needed).
#[cfg(target_arch = "aarch64")]
fn aarch64_l3_table_mut(l0_phys: u64, page: usize) -> Option<*mut [u64; 512]> {
    const TABLE: u64 = 0b11;
    const PA: u64 = 0x0000_FFFF_FFFF_F000;
    let l2_idx = page / AARCH64_USER_L3_PAGES;
    if l2_idx >= 4 {
        return None;
    }
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
        let l2 = &mut *mm::table(l2_phys);
        if l2[l2_idx] & 0b11 != TABLE {
            let l3 = mm::alloc_frame();
            l2[l2_idx] = l3 | TABLE;
        }
        Some(mm::table(l2[l2_idx] & PA))
    }
}

#[cfg(target_arch = "aarch64")]
fn virt_to_phys_aarch64(l0_phys: u64, va: u64) -> Option<u64> {
    const PA: u64 = 0x0000_FFFF_FFFF_F000;
    let page = aarch64_user_page_idx(va);
    let l3_idx = page % AARCH64_USER_L3_PAGES;
    let l2_idx = page / AARCH64_USER_L3_PAGES;
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
        let l3_phys = l2[l2_idx] & PA;
        if l3_phys == 0 {
            return None;
        }
        let l3 = &*mm::table(l3_phys);
        let pte = l3[l3_idx];
        if pte & 0b11 != 0b11 {
            return None;
        }
        Some(pte & PA)
    }
}

#[cfg(target_arch = "riscv64")]
fn satp_ppn(satp: u64) -> u64 {
    paging::satp_root_phys(satp)
}

#[cfg(target_arch = "riscv64")]
fn make_satp(root_phys: u64) -> u64 {
    paging::make_satp(root_phys)
}

#[cfg(target_arch = "riscv64")]
fn virt_to_phys_riscv64(satp: u64, va: u64) -> Option<u64> {
    let root_phys = satp_ppn(satp);
    let i2 = ((va >> 30) & 0x1ff) as usize;
    let i1 = ((va >> 21) & 0x1ff) as usize;
    let i0 = ((va >> 12) & 0x1ff) as usize;
    unsafe {
        let root = &*mm::table(root_phys);
        let mid_pte = root[i2];
        if mid_pte & paging::PTE_V == 0 {
            return None;
        }
        if mid_pte & (paging::PTE_R | paging::PTE_W | paging::PTE_X) != 0 {
            return Some(paging::pte_phys(mid_pte) | (va & 0x1F_FFFF));
        }
        let mid = &*mm::table(paging::pte_phys(mid_pte));
        let leaf_pte = mid[i1];
        if leaf_pte & paging::PTE_V == 0 {
            return None;
        }
        if leaf_pte & (paging::PTE_R | paging::PTE_W | paging::PTE_X) != 0 {
            return Some(paging::pte_phys(leaf_pte) | (va & 0xFFF));
        }
        let leaf = &*mm::table(paging::pte_phys(leaf_pte));
        let pte = leaf[i0];
        if pte & paging::PTE_V == 0 {
            return None;
        }
        Some(paging::pte_phys(pte))
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
    #[cfg(target_arch = "riscv64")]
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
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::ptr::addr_of_mut!(KERNEL_SSCRATCH).write(top);
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
    #[cfg(target_arch = "riscv64")]
    unsafe {
        let t: u64;
        core::arch::asm!(
            "csrr {t}, satp",
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
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!(
            "csrw satp, {a}",
            "sfence.vma",
            a = in(reg) aspace,
            options(nostack),
        );
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
    #[cfg(target_arch = "riscv64")]
    enter_riscv64(user_rip, user_rsp, user_argc, user_argv);
}

#[cfg(target_arch = "x86_64")]
fn enter_x86(user_rip: usize, user_rsp: usize) -> ! {
    let cs = (crate::arch::gdt::user_cs() | 3) as u64;
    let ss = (crate::arch::gdt::user_ss() | 3) as u64;
    let rflags: u64 = 0x202;
    // Iret frame in memory (RIP, CS, RFLAGS, RSP, SS). Do not feed five `in(reg)`
    // operands into one asm block: LLVM can reuse a register for CS and corrupt
    // iretq (post-fork exec of large ELFs → #GP on BIOS).
    let frame = [user_rip as u64, cs, rflags, user_rsp as u64, ss];
    const CR0_TS: u64 = 1 << 3;
    unsafe {
        let mut cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0);
        cr0 &= !CR0_TS;
        core::arch::asm!("mov cr0, {}", in(reg) cr0);
        core::arch::asm!(
            "cli",
            "mov rsp, {f}",
            "iretq",
            f = in(reg) frame.as_ptr(),
            options(noreturn),
        );
    }
}

#[cfg(target_arch = "aarch64")]
fn enter_aarch64(user_rip: usize, user_rsp: usize, user_argc: usize, user_argv: usize) -> ! {
    // Load from a stack slot so LLVM cannot reuse rip/argc in one asm block
    // and reorder mov before msr (CI: exec eret with elr=0).
    let args = [
        user_rip as u64,
        user_rsp as u64,
        user_argc as u64,
        user_argv as u64,
    ];
    let p = args.as_ptr();
    unsafe {
        let rip: u64;
        let rsp: u64;
        core::arch::asm!(
            "ldr {rip}, [{p}]",
            "ldr {rsp}, [{p}, #8]",
            "msr elr_el1, {rip}",
            "msr sp_el0, {rsp}",
            "msr spsr_el1, xzr",
            p = in(reg) p,
            rip = lateout(reg) rip,
            rsp = lateout(reg) rsp,
            options(nostack, preserves_flags),
        );
        core::arch::asm!(
            "ldr x0, [{p}, #16]",
            "ldr x1, [{p}, #24]",
            "isb",
            "eret",
            p = in(reg) p,
            options(noreturn, nostack),
        );
    }
}

#[cfg(target_arch = "riscv64")]
const USER_SSTATUS: u64 = (2 << 32) | (1 << 5); // UXL=64-bit user, SPIE, SPP=0

#[cfg(target_arch = "riscv64")]
fn enter_riscv64(user_rip: usize, user_rsp: usize, user_argc: usize, user_argv: usize) -> ! {
    let ksp = unsafe { KERNEL_SSCRATCH };
    unsafe {
        core::arch::asm!(
            "csrw sscratch, {ksp}",
            "mv sp, {usp}",
            "mv a0, {argc}",
            "mv a1, {argv}",
            "csrw sepc, {rip}",
            "csrw sstatus, {s}",
            "sret",
            ksp = in(reg) ksp,
            usp = in(reg) user_rsp,
            argc = in(reg) user_argc,
            argv = in(reg) user_argv,
            rip = in(reg) user_rip,
            s = in(reg) USER_SSTATUS,
            options(noreturn, nostack),
        );
    }
}

/// Exec from a syscall: copy the saved frame and sret through `fork_sret_from_frame`.
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
fn try_resume_exec_via_syscall_frame(entry: usize, rsp: usize, argc: usize, argv: usize) {
    let frame_ptr = unsafe { SYSCALL_FRAME };
    if frame_ptr.is_null() {
        return;
    }
    unsafe {
        #[cfg(target_arch = "aarch64")]
        {
            let frame = frame_ptr as *mut u64;
            *frame.add(0) = argc as u64;
            *frame.add(1) = argv as u64;
            *frame.add(32) = entry as u64;
            *frame.add(34) = rsp as u64;
            crate::arch::fork_eret_to_user(frame);
        }
        #[cfg(target_arch = "riscv64")]
        {
            let frame = frame_ptr as *mut u64;
            *frame.add(10) = argc as u64;
            *frame.add(11) = argv as u64;
            *frame.add(32) = entry as u64;
            *frame.add(33) = USER_SSTATUS;
            *frame.add(34) = rsp as u64;
            crate::arch::fork_sret_to_user(frame);
        }
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
    xor rax, rax
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
    frame[33] = USER_SSTATUS;
    frame[34] = regs.rsp as u64;
    let ksp = unsafe { KERNEL_SSCRATCH };
    unsafe {
        // Preserve kernel stack top in sscratch across sret (enter_riscv64
        // invariant). The old child stub left sscratch at frame+280 and the
        // next user trap smashed the stack — pipe/fork then jumped to garbage.
        core::arch::asm!("csrw sscratch, {ksp}", ksp = in(reg) ksp, options(nostack));
        crate::arch::fork_sret_child_to_user(frame.as_mut_ptr());
    }
}

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
static mut SYSCALL_FRAME: *mut usize = core::ptr::null_mut();

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
pub fn set_syscall_frame(frame: *mut u64) {
    unsafe {
        SYSCALL_FRAME = frame as *mut usize;
    }
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
        SYS_WRITE => sys_write(a0, a1, a2),
        SYS_EXIT => sys_exit(a0),
        SYS_OPEN => sys_open(a0, a1, a2),
        SYS_READ => sys_read(a0, a1, a2),
        SYS_CLOSE => sys_close(a0),
        SYS_EXEC => sys_exec(a0, a1, a2),
        SYS_FORK => sys_fork(user_rip, user_rsp),
        SYS_WAIT => sys_wait(a0),
        SYS_LISTDIR => sys_listdir(a0, a1, a2),
        SYS_BRK => sys_brk(a0),
        SYS_PIPE => sys_pipe(a0),
        SYS_DUP2 => sys_dup2(a0, a1),
        SYS_DUPFD => sys_dupfd(a0, a1),
        SYS_STAT => sys_stat(a0, a1, a2),
        SYS_EXECNAME => sys_exec_name(a0, a1),
        SYS_CHDIR => sys_chdir(a0, a1),
        SYS_GETCWD => sys_getcwd(a0, a1),
        SYS_MKDIR => sys_mkdir(a0, a1, a2),
        SYS_RMDIR => sys_rmdir(a0, a1),
        SYS_UNLINK => sys_unlink(a0, a1),
        SYS_RENAME => sys_rename(a0, a1, a2),
        SYS_SYMLINK => sys_symlink(a0, a1, a2),
        SYS_READLINK => sys_readlink(a0, a1, a2),
        SYS_MMAP => sys_mmap(a0),
        SYS_MUNMAP => sys_munmap(a0, a1),
        SYS_MPROTECT => sys_mprotect(a0, a1, a2),
        SYS_LSEEK => sys_lseek(a0, a1, a2),
        SYS_MOUNT => sys_mount(a0),
        _ => SYSERR,
    }
}

fn sys_exit(code: usize) -> ! {
    task::user_exit(code as u8);
}

fn sys_write(fd: usize, ptr: usize, len: usize) -> usize {
    task::fd_write(fd, ptr, len)
}

fn resolve_copied_path(path: &str) -> Option<alloc::string::String> {
    let mut abs = [0u8; MAX_PATH];
    let n = fs::resolve_user_path(path, &mut abs)?;
    core::str::from_utf8(&abs[..n])
        .ok()
        .map(|s| alloc::string::String::from(s))
}

fn copy_user_path(ptr: usize, len: usize) -> Option<[u8; MAX_PATH]> {
    if len == 0 || len > MAX_PATH {
        return None;
    }
    if !user_range_ok(ptr, len) {
        return None;
    }
    let mut buf = [0u8; MAX_PATH];
    let aspace = task::current_aspace();
    if !read_user_bytes(aspace, ptr, &mut buf[..len]) {
        return None;
    }
    Some(buf)
}

fn sys_open(ptr: usize, path_len: usize, flags: usize) -> usize {
    let Some(buf) = copy_user_path(ptr, path_len) else {
        return SYSERR;
    };
    let Ok(path) = core::str::from_utf8(&buf[..path_len]) else {
        return SYSERR;
    };
    let Some(path) = resolve_copied_path(path) else {
        return SYSERR;
    };
    let Some(node) = fs::open(&path, flags as u32) else {
        return SYSERR;
    };
    match task::fd_open(node, flags as u32) {
        Some(fd) => fd,
        None => SYSERR,
    }
}

fn sys_read(fd: usize, buf: usize, len: usize) -> usize {
    task::fd_read(fd, buf, len)
}

fn sys_close(fd: usize) -> usize {
    if task::fd_close(fd) { 0 } else { SYSERR }
}

fn sys_exec(ptr: usize, path_len: usize, args_ptr: usize) -> usize {
    let Some(buf) = copy_user_path(ptr, path_len) else {
        return SYSERR;
    };
    let Ok(path) = core::str::from_utf8(&buf[..path_len]) else {
        return SYSERR;
    };
    let Some(path) = resolve_copied_path(path) else {
        return SYSERR;
    };
    let basename = path.rsplit('/').next().unwrap_or(path.as_str()).as_bytes();
    task::set_exec_name(basename);
    let Some(bytes) = fs::lookup(&path) else {
        return SYSERR;
    };
    let (arg_bufs, env_bufs) = match copy_user_exec_pack(args_ptr) {
        Ok(v) => v,
        Err(()) => return SYSERR,
    };
    let arg_refs: Vec<&[u8]> = arg_bufs.iter().map(|s| s.as_slice()).collect();
    let env_refs: Vec<&[u8]> = env_bufs.iter().map(|s| s.as_slice()).collect();
    let cur_aspace = task::current_aspace();
    let (base_u, _mapped_span, stack_off) = task::current_user_map();
    let (aspace, entry, span, off) = {
        #[cfg(target_arch = "riscv64")]
        {
            let _ = (cur_aspace, stack_off, _mapped_span);
            let Some(v) = load_user_elf(bytes) else {
                return SYSERR;
            };
            v
        }
        #[cfg(not(target_arch = "riscv64"))]
        {
            if cur_aspace != 0 {
                if let Some(v) = reload_user_elf(cur_aspace, bytes, base_u, stack_off, _mapped_span)
                    .map(|(entry, span, off)| (cur_aspace, entry, span, off))
                {
                    v
                } else if let Some(v) = expand_user_elf(cur_aspace, bytes, base_u, stack_off)
                    .map(|(entry, span, off)| (cur_aspace, entry, span, off))
                {
                    v
                } else if let Some(v) = load_user_elf(bytes) {
                    v
                } else {
                    return SYSERR;
                }
            } else {
                let Some(v) = load_user_elf(bytes) else {
                    return SYSERR;
                };
                v
            }
        }
    };
    let Some((rsp, argv)) = build_argv_stack(aspace, base_u, off, &arg_refs, &env_refs) else {
        return SYSERR;
    };
    let argc = arg_refs.len();
    task::replace_user(aspace, entry, rsp, base_u, span, off, argc, argv);
    #[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
    try_resume_exec_via_syscall_frame(entry, rsp, argc, argv);
    enter(entry, rsp, argc, argv);
}

fn copy_user_exec_pack(
    args_ptr: usize,
) -> Result<
    (
        alloc::vec::Vec<alloc::vec::Vec<u8>>,
        alloc::vec::Vec<alloc::vec::Vec<u8>>,
    ),
    (),
> {
    if args_ptr == 0 {
        return Ok((alloc::vec::Vec::new(), alloc::vec::Vec::new()));
    }
    if !user_range_ok(args_ptr, core::mem::size_of::<usize>()) {
        return Err(());
    }
    #[cfg(target_arch = "x86_64")]
    {
        copy_user_exec_pack_direct(args_ptr)
    }
    #[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
    {
        copy_user_exec_pack_via_aspace(args_ptr)
    }
}

#[cfg(target_arch = "x86_64")]
fn copy_user_exec_pack_direct(
    args_ptr: usize,
) -> Result<
    (
        alloc::vec::Vec<alloc::vec::Vec<u8>>,
        alloc::vec::Vec<alloc::vec::Vec<u8>>,
    ),
    (),
> {
    let argc = unsafe { *(args_ptr as *const usize) };
    if argc > MAX_ARGC {
        return Err(());
    }
    let mut args = alloc::vec::Vec::with_capacity(argc);
    let mut off = args_ptr + core::mem::size_of::<usize>();
    for _ in 0..argc {
        if !user_range_ok(off, 2 * core::mem::size_of::<usize>()) {
            return Err(());
        }
        let p = unsafe { *(off as *const usize) };
        let n = unsafe { *((off + core::mem::size_of::<usize>()) as *const usize) };
        off += 2 * core::mem::size_of::<usize>();
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
        args.push(v);
    }
    if !user_range_ok(off, core::mem::size_of::<usize>()) {
        return Err(());
    }
    let envc = unsafe { *(off as *const usize) };
    off += core::mem::size_of::<usize>();
    if envc > MAX_ENVC {
        return Err(());
    }
    let mut env = alloc::vec::Vec::with_capacity(envc);
    for _ in 0..envc {
        if !user_range_ok(off, 2 * core::mem::size_of::<usize>()) {
            return Err(());
        }
        let p = unsafe { *(off as *const usize) };
        let n = unsafe { *((off + core::mem::size_of::<usize>()) as *const usize) };
        off += 2 * core::mem::size_of::<usize>();
        if n > MAX_ENV_LEN {
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
        env.push(v);
    }
    Ok((args, env))
}

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
fn copy_user_exec_pack_via_aspace(
    args_ptr: usize,
) -> Result<
    (
        alloc::vec::Vec<alloc::vec::Vec<u8>>,
        alloc::vec::Vec<alloc::vec::Vec<u8>>,
    ),
    (),
> {
    let aspace = task::current_aspace();
    let argc = read_user_usize(aspace, args_ptr).ok_or(())?;
    if argc > MAX_ARGC {
        return Err(());
    }
    let mut args = alloc::vec::Vec::with_capacity(argc);
    let mut off = args_ptr + core::mem::size_of::<usize>();
    for _ in 0..argc {
        if !user_range_ok(off, 2 * core::mem::size_of::<usize>()) {
            return Err(());
        }
        let p = read_user_usize(aspace, off).ok_or(())?;
        let n = read_user_usize(aspace, off + core::mem::size_of::<usize>()).ok_or(())?;
        off += 2 * core::mem::size_of::<usize>();
        if n > MAX_ARG_LEN {
            return Err(());
        }
        if n != 0 && !user_range_ok(p, n) {
            return Err(());
        }
        let mut v = alloc::vec::Vec::with_capacity(n);
        if n != 0 {
            v.resize(n, 0);
            if !read_user_bytes(aspace, p, &mut v) {
                return Err(());
            }
        }
        args.push(v);
    }
    if !user_range_ok(off, core::mem::size_of::<usize>()) {
        return Err(());
    }
    let envc = read_user_usize(aspace, off).ok_or(())?;
    off += core::mem::size_of::<usize>();
    if envc > MAX_ENVC {
        return Err(());
    }
    let mut env = alloc::vec::Vec::with_capacity(envc);
    for _ in 0..envc {
        if !user_range_ok(off, 2 * core::mem::size_of::<usize>()) {
            return Err(());
        }
        let p = read_user_usize(aspace, off).ok_or(())?;
        let n = read_user_usize(aspace, off + core::mem::size_of::<usize>()).ok_or(())?;
        off += 2 * core::mem::size_of::<usize>();
        if n > MAX_ENV_LEN {
            return Err(());
        }
        if n != 0 && !user_range_ok(p, n) {
            return Err(());
        }
        let mut v = alloc::vec::Vec::with_capacity(n);
        if n != 0 {
            v.resize(n, 0);
            if !read_user_bytes(aspace, p, &mut v) {
                return Err(());
            }
        }
        env.push(v);
    }
    Ok((args, env))
}

fn sys_listdir(path_ptr: usize, path_len: usize, buf: usize) -> usize {
    // Match libgloss MYOS_DIRBUF / myos_user::LISTDIR_BUF so /s listings
    // (~100 names) are not truncated. Callers must pass a mapped buffer of
    // this size (user_range_ok); a 512-byte stack buf fails the check.
    const LISTDIR_CAP: usize = 4096;
    if buf == 0 || !user_range_ok(buf, LISTDIR_CAP) {
        return SYSERR;
    }
    let path = if path_len == 0 {
        alloc::string::String::from(".")
    } else {
        let Some(pbuf) = copy_user_path(path_ptr, path_len) else {
            return SYSERR;
        };
        match core::str::from_utf8(&pbuf[..path_len]) {
            Ok(p) => alloc::string::String::from(p),
            Err(_) => return SYSERR,
        }
    };
    let Some(path) = resolve_copied_path(&path) else {
        return SYSERR;
    };
    let mut kbuf = [0u8; LISTDIR_CAP];
    let n = fs::listdir(&path, &mut kbuf).min(LISTDIR_CAP);
    let aspace = task::current_aspace();
    if !write_user_bytes(aspace, buf, &kbuf[..n]) {
        return SYSERR;
    }
    n
}

#[repr(C)]
struct MyosStatBuf {
    st_mode: u32,
    st_size: u32,
    st_ino: u32,
    st_nlink: u32,
}

fn sys_stat(path_ptr: usize, path_len: usize, out_ptr: usize) -> usize {
    if out_ptr == 0 || !user_range_ok(out_ptr, core::mem::size_of::<MyosStatBuf>()) {
        return SYSERR;
    }
    let Some(buf) = copy_user_path(path_ptr, path_len) else {
        return SYSERR;
    };
    let Ok(path) = core::str::from_utf8(&buf[..path_len]) else {
        return SYSERR;
    };
    let Some(path) = resolve_copied_path(path) else {
        return SYSERR;
    };
    let Some(info) = fs::stat(&path) else {
        return SYSERR;
    };
    let out = MyosStatBuf {
        st_mode: info.mode,
        st_size: info.size,
        st_ino: info.ino,
        st_nlink: info.nlink,
    };
    if !write_user_bytes(task::current_aspace(), out_ptr, unsafe {
        core::slice::from_raw_parts(
            &out as *const MyosStatBuf as *const u8,
            core::mem::size_of::<MyosStatBuf>(),
        )
    }) {
        return SYSERR;
    }
    0
}

fn sys_fork(user_rip: usize, user_rsp: usize) -> usize {
    #[cfg(target_arch = "x86_64")]
    let child = {
        // Use the snapshot from syscall_entry — live rbx/rbp/r12–r15 here may
        // already be Rust scratch (prologues saved the user values on the stack).
        let c = unsafe { core::ptr::addr_of!(FORK_CALLEE).read() };
        task::ForkRegs {
            rip: user_rip,
            rsp: user_rsp,
            rbx: c.rbx,
            rbp: c.rbp,
            r12: c.r12,
            r13: c.r13,
            r14: c.r14,
            r15: c.r15,
        }
    };
    #[cfg(target_arch = "aarch64")]
    let child = {
        let frame = unsafe { SYSCALL_FRAME };
        if frame.is_null() {
            return SYSERR;
        }
        task::ForkRegs {
            rip: user_rip,
            rsp: user_rsp,
            frame: copy_fork_syscall_frame(frame as *const u64),
        }
    };
    #[cfg(target_arch = "riscv64")]
    let child = {
        let frame = unsafe { SYSCALL_FRAME };
        if frame.is_null() {
            return SYSERR;
        }
        task::ForkRegs {
            rip: user_rip,
            rsp: user_rsp,
            frame: copy_fork_syscall_frame(frame as *const u64),
        }
    };
    match task::fork_current(child) {
        Some(pid) => pid,
        None => SYSERR,
    }
}

fn sys_wait(status_ptr: usize) -> usize {
    let status_out = if status_ptr == 0 {
        core::ptr::null_mut()
    } else {
        if !user_range_ok(status_ptr, 1) {
            return SYSERR;
        }
        status_ptr as *mut u8
    };
    task::wait_child(status_out)
}

fn sys_pipe(fds_ptr: usize) -> usize {
    if !user_range_ok(fds_ptr, 2 * core::mem::size_of::<usize>()) {
        return SYSERR;
    }
    let Some((r, w)) = task::pipe_open() else {
        return SYSERR;
    };
    unsafe {
        *(fds_ptr as *mut usize) = r;
        *((fds_ptr as *mut usize).add(1)) = w;
    }
    0
}

fn sys_dup2(oldfd: usize, newfd: usize) -> usize {
    if task::fd_dup2(oldfd, newfd) {
        0
    } else {
        SYSERR
    }
}

fn sys_dupfd(oldfd: usize, minfd: usize) -> usize {
    match task::fd_dup_min(oldfd, minfd) {
        Some(fd) => fd,
        None => SYSERR,
    }
}

fn sys_chdir(path_ptr: usize, path_len: usize) -> usize {
    let Some(buf) = copy_user_path(path_ptr, path_len) else {
        return SYSERR;
    };
    let Ok(path) = core::str::from_utf8(&buf[..path_len]) else {
        return SYSERR;
    };
    let Some(path) = resolve_copied_path(path) else {
        return SYSERR;
    };
    let Some(info) = fs::stat(&path) else {
        return SYSERR;
    };
    const S_IFDIR: u32 = 0o040000;
    if (info.mode & S_IFDIR) == 0 {
        return SYSERR;
    }
    if !task::set_cwd(path.as_bytes()) {
        return SYSERR;
    }
    0
}

fn sys_getcwd(buf_ptr: usize, buf_len: usize) -> usize {
    if buf_ptr == 0 || buf_len == 0 {
        return SYSERR;
    }
    if !user_range_ok(buf_ptr, buf_len) {
        return SYSERR;
    }
    let mut cwd = [0u8; MAX_PATH];
    let n = task::cwd(&mut cwd);
    // POSIX getcwd needs room for the pathname and a trailing NUL.
    if n + 1 > buf_len {
        return SYSERR;
    }
    let mut tmp = [0u8; MAX_PATH + 1];
    tmp[..n].copy_from_slice(&cwd[..n]);
    tmp[n] = 0;
    if !write_user_bytes(task::current_aspace(), buf_ptr, &tmp[..n + 1]) {
        return SYSERR;
    }
    n
}

fn copy_resolved_user_path(ptr: usize, len: usize) -> Option<alloc::string::String> {
    let buf = copy_user_path(ptr, len)?;
    let path = core::str::from_utf8(&buf[..len]).ok()?;
    resolve_copied_path(path)
}

/// Pack two lengths into one register: `(len_a << 16) | len_b` (each <= MAX_PATH).
fn unpack_two_lens(packed: usize) -> Option<(usize, usize)> {
    let a = packed >> 16;
    let b = packed & 0xffff;
    if a == 0 || a > MAX_PATH || b == 0 || b > MAX_PATH {
        return None;
    }
    Some((a, b))
}

fn sys_mkdir(path_ptr: usize, path_len: usize, _mode: usize) -> usize {
    let Some(path) = copy_resolved_user_path(path_ptr, path_len) else {
        return SYSERR;
    };
    if fs::mkdir(&path) { 0 } else { SYSERR }
}

fn sys_rmdir(path_ptr: usize, path_len: usize) -> usize {
    let Some(path) = copy_resolved_user_path(path_ptr, path_len) else {
        return SYSERR;
    };
    if fs::rmdir(&path) { 0 } else { SYSERR }
}

fn sys_unlink(path_ptr: usize, path_len: usize) -> usize {
    let Some(path) = copy_resolved_user_path(path_ptr, path_len) else {
        return SYSERR;
    };
    if fs::unlink(&path) { 0 } else { SYSERR }
}

/// `a0`=old_ptr, `a1`=new_ptr, `a2`=(old_len<<16)|new_len
fn sys_rename(old_ptr: usize, new_ptr: usize, packed_lens: usize) -> usize {
    let Some((old_len, new_len)) = unpack_two_lens(packed_lens) else {
        return SYSERR;
    };
    let Some(old) = copy_resolved_user_path(old_ptr, old_len) else {
        return SYSERR;
    };
    let Some(new) = copy_resolved_user_path(new_ptr, new_len) else {
        return SYSERR;
    };
    if fs::rename(&old, &new) { 0 } else { SYSERR }
}

/// `a0`=target_ptr, `a1`=link_ptr, `a2`=(target_len<<16)|link_len
fn sys_symlink(target_ptr: usize, link_ptr: usize, packed_lens: usize) -> usize {
    let Some((target_len, link_len)) = unpack_two_lens(packed_lens) else {
        return SYSERR;
    };
    // Target is opaque text (may be relative); do not cwd-resolve it.
    let Some(tbuf) = copy_user_path(target_ptr, target_len) else {
        return SYSERR;
    };
    let Ok(target) = core::str::from_utf8(&tbuf[..target_len]) else {
        return SYSERR;
    };
    let Some(linkpath) = copy_resolved_user_path(link_ptr, link_len) else {
        return SYSERR;
    };
    if fs::symlink(target, &linkpath) {
        0
    } else {
        SYSERR
    }
}

/// `a0`=path_ptr, `a1`=buf_ptr, `a2`=(path_len<<16)|buf_len
fn sys_readlink(path_ptr: usize, buf_ptr: usize, packed: usize) -> usize {
    let Some((path_len, buf_len)) = unpack_two_lens(packed) else {
        return SYSERR;
    };
    if buf_ptr == 0 || buf_len == 0 || !user_range_ok(buf_ptr, buf_len) {
        return SYSERR;
    }
    let Some(path) = copy_resolved_user_path(path_ptr, path_len) else {
        return SYSERR;
    };
    let mut tmp = [0u8; MAX_PATH];
    let cap = buf_len.min(tmp.len());
    let Some(n) = fs::readlink(&path, &mut tmp[..cap]) else {
        return SYSERR;
    };
    if !write_user_bytes(task::current_aspace(), buf_ptr, &tmp[..n]) {
        return SYSERR;
    }
    n
}

fn sys_exec_name(buf: usize, len: usize) -> usize {
    if len == 0 || !user_range_ok(buf, len) {
        return SYSERR;
    }
    let mut tmp = [0u8; 32];
    let n = task::exec_name(&mut tmp).min(len).min(tmp.len());
    if n == 0 {
        return 0;
    }
    if !write_user_bytes(task::current_aspace(), buf, &tmp[..n]) {
        return SYSERR;
    }
    n
}

fn sys_brk(req: usize) -> usize {
    let (base, _span, stack_off) = task::current_user_map();
    let heap_base = heap_base_va(base, stack_off) as usize;
    let heap_limit = heap_limit_va(base, stack_off) as usize;
    let mut cur = task::current_brk() as usize;
    if cur == 0 {
        cur = heap_base;
        task::set_brk(cur as u64);
    }
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
                unsafe {
                    core::ptr::write_bytes(mm::hhdm(frame), 0, PAGE);
                }
                map_heap_page(aspace, va as u64, frame);
            }
            va += PAGE;
        }
    }
    task::set_brk(req as u64);
    req
}

/// `a0` points at a user `myos_mmap_args` {addr,len,prot,flags,fd,offset}.
fn sys_mmap(args_ptr: usize) -> usize {
    const N: usize = 6 * core::mem::size_of::<u64>();
    if !user_range_ok(args_ptr, N) {
        return SYSERR;
    }
    let mut raw = [0u8; N];
    if !read_user_bytes(task::current_aspace(), args_ptr, &mut raw) {
        return SYSERR;
    }
    let mut words = [0u64; 6];
    for i in 0..6 {
        let o = i * 8;
        words[i] = u64::from_le_bytes(raw[o..o + 8].try_into().unwrap());
    }
    let hint = words[0] as usize;
    let len = words[1] as usize;
    let prot = words[2] as usize;
    let flags = words[3] as usize;
    let fd = words[4] as isize;
    let offset = words[5] as usize;
    do_mmap(hint, len, prot, flags, fd, offset)
}

fn do_mmap(hint: usize, len: usize, prot: usize, flags: usize, fd: isize, offset: usize) -> usize {
    if len == 0 {
        return SYSERR;
    }
    let anon = flags & MAP_ANON != 0;
    if !anon {
        // File-backed is optional; anonymous MAP_ANON|MAP_PRIVATE is the must-have.
        return SYSERR;
    }
    if flags & MAP_PRIVATE == 0 && flags & MAP_FIXED == 0 {
        // Require PRIVATE or FIXED; tcc uses MAP_PRIVATE|MAP_ANON.
        return SYSERR;
    }
    let (base, _span, stack_off) = task::current_user_map();
    let area_lo = mmap_base_va(base, stack_off) as usize;
    let area_hi = mmap_limit_va(base, stack_off) as usize;
    let pages = len.div_ceil(PAGE);
    if pages == 0 || pages > MMAP_AREA_PAGES {
        return SYSERR;
    }
    let map_len = pages * PAGE;
    let va = if flags & MAP_FIXED != 0 {
        if hint == 0 || hint % PAGE != 0 {
            return SYSERR;
        }
        if hint < area_lo || hint.saturating_add(map_len) > area_hi {
            return SYSERR;
        }
        hint
    } else {
        match task::mmap_alloc(area_lo, area_hi, map_len) {
            Some(v) => v,
            None => return SYSERR,
        }
    };
    if task::mmap_overlaps(va, map_len) {
        return SYSERR;
    }
    let aspace = task::current_aspace();
    let mut mapped = 0usize;
    while mapped < map_len {
        let page_va = (va + mapped) as u64;
        let frame = mm::alloc_frame();
        unsafe {
            core::ptr::write_bytes(mm::hhdm(frame), 0, PAGE);
        }
        map_user_page_prot(aspace, page_va, frame, prot);
        mapped += PAGE;
    }
    if prot & PROT_EXEC != 0 {
        let mut off = 0;
        while off < map_len {
            if let Some(phys) = virt_to_phys(aspace, (va + off) as u64) {
                // User VA (execute) + HHDM alias (the stores). ic ialluis inside.
                sync_icache((va + off) as usize, PAGE);
                sync_icache(mm::hhdm(phys) as usize, PAGE);
            }
            off += PAGE;
        }
    }
    let _ = (fd, offset);
    if !task::mmap_add(va as u64, pages as u32, prot as u32) {
        // Region table full: unmap what we just added.
        let _ = sys_munmap(va, map_len);
        return SYSERR;
    }
    flush_user_tlb();
    va
}

fn sys_munmap(addr: usize, len: usize) -> usize {
    if addr % PAGE != 0 || len == 0 {
        return SYSERR;
    }
    let pages = len.div_ceil(PAGE);
    let map_len = pages * PAGE;
    if !task::mmap_contains(addr, map_len) && !user_range_ok(addr, map_len) {
        return SYSERR;
    }
    let aspace = task::current_aspace();
    let mut off = 0;
    while off < map_len {
        unmap_user_page(aspace, (addr + off) as u64);
        off += PAGE;
    }
    task::mmap_remove(addr as u64, pages as u32);
    flush_user_tlb();
    0
}

fn sys_mprotect(addr: usize, len: usize, prot: usize) -> usize {
    if addr % PAGE != 0 || len == 0 {
        return SYSERR;
    }
    let pages = len.div_ceil(PAGE);
    let map_len = pages * PAGE;
    let (base, _span, stack_off) = task::current_user_map();
    let lo = base as usize;
    let hi = mmap_limit_va(base, stack_off) as usize;
    if addr < lo || addr.saturating_add(map_len) > hi {
        return SYSERR;
    }
    let aspace = task::current_aspace();
    let mut off = 0;
    while off < map_len {
        let va = (addr + off) as u64;
        let Some(phys) = virt_to_phys(aspace, va) else {
            return SYSERR;
        };
        map_user_page_prot(aspace, va, phys, prot);
        if prot & PROT_EXEC != 0 {
            // mprotect RW→RX: clean D-cache, invalidate I-cache for this range.
            sync_icache(va as usize, PAGE);
            sync_icache(mm::hhdm(phys) as usize, PAGE);
        }
        off += PAGE;
    }
    task::mmap_set_prot(addr as u64, pages as u32, prot as u32);
    flush_user_tlb();
    0
}

fn sys_lseek(fd: usize, offset: usize, whence: usize) -> usize {
    task::fd_lseek(fd, offset as i64, whence)
}

/// `a0` points at `{src_ptr, src_len, tgt_ptr, tgt_len, fstype_ptr, fstype_len}`.
fn sys_mount(args_ptr: usize) -> usize {
    const N: usize = 6 * core::mem::size_of::<usize>();
    if !user_range_ok(args_ptr, N) {
        return SYSERR;
    }
    let mut raw = [0u8; N];
    if !read_user_bytes(task::current_aspace(), args_ptr, &mut raw) {
        return SYSERR;
    }
    let mut words = [0usize; 6];
    for i in 0..6 {
        let o = i * core::mem::size_of::<usize>();
        words[i] = usize::from_le_bytes(
            raw[o..o + core::mem::size_of::<usize>()]
                .try_into()
                .unwrap(),
        );
    }
    let Some(src_buf) = copy_user_path(words[0], words[1]) else {
        return SYSERR;
    };
    let Some(tgt_buf) = copy_user_path(words[2], words[3]) else {
        return SYSERR;
    };
    let Some(fs_buf) = copy_user_path(words[4], words[5]) else {
        return SYSERR;
    };
    let Ok(src) = core::str::from_utf8(&src_buf[..words[1]]) else {
        return SYSERR;
    };
    let Ok(tgt) = core::str::from_utf8(&tgt_buf[..words[3]]) else {
        return SYSERR;
    };
    let Ok(fstype) = core::str::from_utf8(&fs_buf[..words[5]]) else {
        return SYSERR;
    };
    let Some(src) = resolve_copied_path(src) else {
        return SYSERR;
    };
    let Some(tgt) = resolve_copied_path(tgt) else {
        return SYSERR;
    };
    let Some(st) = fs::stat(&src) else {
        return SYSERR;
    };
    if st.mode & fs::S_IFMT != fs::S_IFBLK {
        return SYSERR;
    }
    let Some(dev) = fs::blk_id_from_path(&src) else {
        return SYSERR;
    };
    let prefix = tgt.trim_start_matches('/');
    if prefix.is_empty() || prefix.contains('/') || fstype.is_empty() {
        return SYSERR;
    }
    if fs::mount_fstype(dev, prefix, fstype, &src) {
        0
    } else {
        SYSERR
    }
}

/// True when `ptr..ptr+len` lies in the current task's user code, stack, heap, or mmap.
pub fn buffer_ok(ptr: usize, len: usize) -> bool {
    user_range_ok(ptr, len)
}

fn user_range_ok(ptr: usize, len: usize) -> bool {
    let (base, image_span, stack_off) = task::current_user_map();
    let base = base as usize;
    let end = match ptr.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    let in_code = ptr >= base && end <= base + image_span;
    let stack_base = base + stack_off as usize;
    let in_stack = ptr >= stack_base && end <= stack_base + USER_STACK_PAGES * PAGE;
    let heap_base = heap_base_va(base as u64, stack_off) as usize;
    let brk = task::current_brk() as usize;
    let in_heap = brk > heap_base && ptr >= heap_base && end <= brk;
    if in_code || in_stack || in_heap {
        return true;
    }
    // mmap_contains takes TASKS; skip unless ptr is outside code/stack/heap.
    task::mmap_contains(ptr, len)
}

fn create_aspace(code: &[u64], stack: &[u64], base: u64, stack_off: u64) -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        create_aspace_x86(code, stack, base, stack_off)
    }
    #[cfg(target_arch = "aarch64")]
    {
        create_aspace_aarch64(code, stack, base, stack_off)
    }
    #[cfg(target_arch = "riscv64")]
    {
        create_aspace_riscv64(code, stack, base, stack_off)
    }
}

#[cfg(target_arch = "x86_64")]
fn create_aspace_x86(code: &[u64], stack: &[u64], base: u64, stack_off: u64) -> u64 {
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
    for (i, &phys) in stack.iter().enumerate() {
        map_page_x86(
            pml4_phys,
            stack_va + (i * PAGE) as u64,
            phys,
            PRESENT | WRITE | USER | NX,
        );
    }
    pml4_phys
}

fn map_user_page_prot(aspace: u64, va: u64, pa: u64, prot: usize) {
    let w = prot & PROT_WRITE != 0;
    let x = prot & PROT_EXEC != 0;
    #[cfg(target_arch = "x86_64")]
    {
        const PRESENT: u64 = 1;
        const WRITE: u64 = 1 << 1;
        const USER: u64 = 1 << 2;
        const NX: u64 = 1 << 63;
        let mut flags = PRESENT | USER;
        if w {
            flags |= WRITE;
        }
        if !x {
            flags |= NX;
        }
        map_page_x86(aspace, va, pa, flags);
    }
    #[cfg(target_arch = "aarch64")]
    {
        const PAGE_DESC: u64 = 0b11;
        const SH_INNER: u64 = 0b11 << 8;
        const AF: u64 = 1 << 10;
        const AP_RW: u64 = 0b01 << 6;
        const AP_RO: u64 = 0b11 << 6;
        const PXN: u64 = 1 << 53;
        const UXN: u64 = 1 << 54;
        const PA: u64 = 0x0000_FFFF_FFFF_F000;
        let page = aarch64_user_page_idx(va);
        let l3_idx = page % AARCH64_USER_L3_PAGES;
        let Some(l3) = aarch64_l3_table_mut(aspace, page) else {
            return;
        };
        unsafe {
            let l3_t = &mut *l3;
            // UXN=0 iff PROT_EXEC so EL0 can fetch (anonymous RW→RX JIT).
            let mut ent = PAGE_DESC | (pa & PA) | SH_INNER | AF | PXN;
            ent |= if w { AP_RW } else { AP_RO };
            if !x {
                ent |= UXN;
            }
            l3_t[l3_idx] = ent;
        }
    }
    #[cfg(target_arch = "riscv64")]
    {
        let mut flags =
            paging::PTE_V | paging::PTE_U | paging::PTE_A | paging::PTE_D | paging::PTE_R;
        if w {
            flags |= paging::PTE_W;
        }
        if x {
            flags |= paging::PTE_X;
        }
        let i0 = ((va >> 12) & 0x1ff) as usize;
        let leaf = ensure_riscv_leaf(aspace, va);
        unsafe {
            (*leaf)[i0] = paging::pte_leaf_4k(pa, flags);
        }
    }
}

fn unmap_user_page(aspace: u64, va: u64) {
    #[cfg(target_arch = "x86_64")]
    {
        const PRESENT: u64 = 1;
        const HUGE: u64 = 1 << 7;
        let i4 = ((va >> 39) & 0x1ff) as usize;
        let i3 = ((va >> 30) & 0x1ff) as usize;
        let i2 = ((va >> 21) & 0x1ff) as usize;
        let i1 = ((va >> 12) & 0x1ff) as usize;
        unsafe {
            let pml4 = &*mm::table(aspace);
            if pml4[i4] & PRESENT == 0 {
                return;
            }
            let pdpt = &*mm::table(pml4[i4]);
            if pdpt[i3] & PRESENT == 0 || pdpt[i3] & HUGE != 0 {
                return;
            }
            let pd = &*mm::table(pdpt[i3]);
            if pd[i2] & PRESENT == 0 || pd[i2] & HUGE != 0 {
                return;
            }
            let pt = &mut *mm::table(pd[i2]);
            pt[i1] = 0;
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        let page = aarch64_user_page_idx(va);
        let l3_idx = page % AARCH64_USER_L3_PAGES;
        let Some(l3) = aarch64_l3_table_mut(aspace, page) else {
            return;
        };
        unsafe {
            (*l3)[l3_idx] = 0;
        }
    }
    #[cfg(target_arch = "riscv64")]
    {
        let i0 = ((va >> 12) & 0x1ff) as usize;
        let leaf = ensure_riscv_leaf(aspace, va);
        unsafe {
            (*leaf)[i0] = 0;
        }
    }
}

fn map_user_code_page(aspace: u64, va: u64, pa: u64) {
    #[cfg(target_arch = "x86_64")]
    {
        const PRESENT: u64 = 1;
        const WRITE: u64 = 1 << 1;
        const USER: u64 = 1 << 2;
        map_page_x86(aspace, va, pa, PRESENT | WRITE | USER);
    }
    #[cfg(target_arch = "aarch64")]
    {
        map_user_page_aarch64(aspace, va, pa, false);
    }
    #[cfg(target_arch = "riscv64")]
    {
        map_user_page_riscv64(aspace, va, pa, true);
    }
}

fn map_user_stack_page(aspace: u64, va: u64, pa: u64) {
    #[cfg(target_arch = "x86_64")]
    {
        const PRESENT: u64 = 1;
        const WRITE: u64 = 1 << 1;
        const USER: u64 = 1 << 2;
        const NX: u64 = 1 << 63;
        map_page_x86(aspace, va, pa, PRESENT | WRITE | USER | NX);
    }
    #[cfg(target_arch = "aarch64")]
    {
        map_user_page_aarch64(aspace, va, pa, true);
    }
    #[cfg(target_arch = "riscv64")]
    {
        map_user_page_riscv64(aspace, va, pa, false);
    }
}

#[cfg(target_arch = "aarch64")]
fn map_user_page_aarch64(l0_phys: u64, va: u64, pa: u64, stack: bool) {
    const PAGE_DESC: u64 = 0b11;
    const SH_INNER: u64 = 0b11 << 8;
    const AF: u64 = 1 << 10;
    const AP_RW: u64 = 0b01 << 6;
    const PXN: u64 = 1 << 53;
    const UXN: u64 = 1 << 54;
    const PA: u64 = 0x0000_FFFF_FFFF_F000;
    let page = aarch64_user_page_idx(va);
    let l3_idx = page % AARCH64_USER_L3_PAGES;
    let Some(l3) = aarch64_l3_table_mut(l0_phys, page) else {
        return;
    };
    unsafe {
        let l3_t = &mut *l3;
        let mut ent = PAGE_DESC | (pa & PA) | SH_INNER | AP_RW | AF | PXN;
        if stack {
            ent |= UXN;
        }
        l3_t[l3_idx] = ent;
    }
}

#[cfg(target_arch = "aarch64")]
fn flush_user_tlb() {
    unsafe {
        core::arch::asm!("dsb ishst", options(nostack));
        if current_el() >= 2 {
            // VHE EL2&0 uses ALLE2; also drop EL1&0 in case TGE/E2H is off.
            core::arch::asm!("tlbi alle2is", options(nostack));
            core::arch::asm!("tlbi vmalle1is", options(nostack));
        } else {
            core::arch::asm!("tlbi vmalle1is", options(nostack));
        }
        core::arch::asm!("dsb ish; isb", options(nostack));
    }
}

#[cfg(target_arch = "x86_64")]
fn flush_user_tlb() {
    unsafe {
        let cr3: u64;
        core::arch::asm!(
            "mov {cr3}, cr3",
            "mov cr3, {cr3}",
            cr3 = out(reg) cr3,
            options(nostack, preserves_flags),
        );
    }
}

#[cfg(target_arch = "riscv64")]
fn flush_user_tlb() {
    unsafe {
        core::arch::asm!("sfence.vma zero, zero", options(nostack));
    }
}

/// Allocate Sv39 mid/leaf tables as needed so user maps can spill past one
/// 2 MiB leaf (code + 256 stack + 256 heap pages exceeds 512 PTEs).
#[cfg(target_arch = "riscv64")]
fn ensure_riscv_leaf(satp: u64, va: u64) -> *mut [u64; 512] {
    let root_phys = paging::satp_root_phys(satp);
    let i2 = ((va >> 30) & 0x1ff) as usize;
    let i1 = ((va >> 21) & 0x1ff) as usize;
    unsafe {
        let root = &mut *mm::table(root_phys);
        let mid_pte = root[i2];
        if mid_pte & paging::PTE_V == 0 {
            let mid = mm::alloc_frame();
            root[i2] = paging::pte_table(mid);
        } else {
            assert!(
                mid_pte & (paging::PTE_R | paging::PTE_W | paging::PTE_X) == 0,
                "riscv user map: huge page in the way at L2"
            );
        }
        let mid = &mut *mm::table(paging::pte_phys(root[i2]));
        let leaf_pte = mid[i1];
        if leaf_pte & paging::PTE_V == 0 {
            let leaf = mm::alloc_frame();
            mid[i1] = paging::pte_table(leaf);
        } else {
            assert!(
                leaf_pte & (paging::PTE_R | paging::PTE_W | paging::PTE_X) == 0,
                "riscv user map: huge page in the way at L1"
            );
        }
        mm::table(paging::pte_phys(mid[i1]))
    }
}

#[cfg(target_arch = "riscv64")]
fn map_user_page_riscv64(satp: u64, va: u64, pa: u64, exec: bool) {
    let mut flags = paging::PTE_V
        | paging::PTE_R
        | paging::PTE_W
        | paging::PTE_U
        | paging::PTE_A
        | paging::PTE_D;
    if exec {
        flags |= paging::PTE_X;
    }
    let i0 = ((va >> 12) & 0x1ff) as usize;
    let leaf = ensure_riscv_leaf(satp, va);
    unsafe {
        (*leaf)[i0] = paging::pte_leaf_4k(pa, flags);
    }
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
    map_user_page_aarch64(l0_phys, va, pa, false);
}

#[cfg(target_arch = "riscv64")]
fn map_heap_page(satp: u64, va: u64, pa: u64) {
    let flags = paging::PTE_V
        | paging::PTE_R
        | paging::PTE_W
        | paging::PTE_U
        | paging::PTE_A
        | paging::PTE_D;
    let i0 = ((va >> 12) & 0x1ff) as usize;
    let leaf = ensure_riscv_leaf(satp, va);
    unsafe {
        (*leaf)[i0] = paging::pte_leaf_4k(pa, flags);
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
fn create_aspace_aarch64(code: &[u64], stack: &[u64], _base: u64, stack_off: u64) -> u64 {
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
        l0_t[0] = l1 | TABLE;
        l1_t[0] = device;
        l1_t[1] = l2 | TABLE;
        l2_t[0] = l3 | TABLE;
        // AP_RW: EL1 sys_read copies into PT_LOAD. PXN: EL1 cannot execute it.
        for (i, &phys) in code.iter().enumerate() {
            let Some(l3p) = aarch64_l3_table_mut(l0, i) else {
                break;
            };
            let l3_t = &mut *l3p;
            let slot = i % AARCH64_USER_L3_PAGES;
            l3_t[slot] = PAGE_DESC | (phys & PA) | SH_INNER | AF | AP_RW | PXN;
        }
        let stack_i = (stack_off as usize) / PAGE;
        for (i, &phys) in stack.iter().enumerate() {
            let page = stack_i + i;
            let Some(l3p) = aarch64_l3_table_mut(l0, page) else {
                break;
            };
            let l3_t = &mut *l3p;
            let slot = page % AARCH64_USER_L3_PAGES;
            l3_t[slot] = PAGE_DESC | (phys & PA) | SH_INNER | AF | AP_RW | PXN | UXN;
        }
    }
    l0
}

#[cfg(target_arch = "riscv64")]
fn create_aspace_riscv64(code: &[u64], stack: &[u64], base: u64, stack_off: u64) -> u64 {
    let k_root_phys = paging::satp_root_phys(task::kernel_aspace());
    let root = mm::alloc_frame();

    unsafe {
        let k_root = &*mm::table(k_root_phys);
        let root_t = &mut *mm::table(root);
        root_t.copy_from_slice(k_root);
        // User lives in Sv39 root[1] (0x4000_0000). Clear any stale kernel
        // entry so ensure_riscv_leaf owns mid/leaf allocation for this aspace.
        root_t[1] = 0;
    }
    let satp = make_satp(root);
    for (i, &phys) in code.iter().enumerate() {
        map_user_page_riscv64(satp, base + (i * PAGE) as u64, phys, true);
    }
    let stack_va = base + stack_off;
    for (i, &phys) in stack.iter().enumerate() {
        map_user_page_riscv64(satp, stack_va + (i * PAGE) as u64, phys, false);
    }
    satp
}

fn sync_icache(start: usize, size: usize) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let cr3: u64;
        core::arch::asm!(
            "mov {cr3}, cr3",
            "mov cr3, {cr3}",
            cr3 = out(reg) cr3,
            options(nostack, preserves_flags),
        );
    }
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
        core::arch::asm!("ic ialluis; dsb ish; isb", options(nostack));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        if size != 0 {
            core::arch::asm!("fence.i", options(nostack));
        }
    }
    let _ = (start, size);
}
