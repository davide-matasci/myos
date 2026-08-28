//! Usermode: handwritten blobs, per-process page tables, write/exit syscalls.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::mm;
use crate::task;

const SYS_WRITE: usize = 0;
const SYS_EXIT: usize = 1;

static USERS_ALIVE: AtomicUsize = AtomicUsize::new(0);
static DID_SPAWN: AtomicBool = AtomicBool::new(false);

#[cfg(target_arch = "x86_64")]
const DEFAULT_USER_BASE: u64 = 0x0000_0080_0000_0000; // PML4[1]
#[cfg(target_arch = "aarch64")]
const DEFAULT_USER_BASE: u64 = 0x4000_0000; // L1[1] on QEMU virt RAM

static USER_BASE: AtomicU64 = AtomicU64::new(DEFAULT_USER_BASE);

#[cfg(target_arch = "x86_64")]
const USER_BLOB: &[u8] = &[
    0x48, 0xc7, 0xc0, 0x00, 0x00, 0x00, 0x00, // mov rax, 0
    0x48, 0x8d, 0x35, 0x12, 0x00, 0x00, 0x00, // lea rsi, [rip+0x12]
    0x48, 0xc7, 0xc2, 0x08, 0x00, 0x00, 0x00, // mov rdx, 8
    0x0f, 0x05,                               // syscall
    0x48, 0xc7, 0xc0, 0x01, 0x00, 0x00, 0x00, // mov rax, 1
    0x0f, 0x05,                               // syscall
    b'u', b's', b'e', b'r', b' ', b'o', b'k', b'\n',
];

#[cfg(target_arch = "aarch64")]
const USER_BLOB: &[u8] = &[
    0xc0, 0x00, 0x00, 0x10, // adr x0, #24
    0x01, 0x01, 0x80, 0xd2, // mov x1, #8
    0x08, 0x00, 0x80, 0xd2, // mov x8, #0
    0x01, 0x00, 0x00, 0xd4, // svc #0
    0x28, 0x00, 0x80, 0xd2, // mov x8, #1
    0x01, 0x00, 0x00, 0xd4, // svc #0
    b'u', b's', b'e', b'r', b' ', b'o', b'k', b'\n',
];

#[cfg(target_arch = "x86_64")]
static mut USER_RSP: usize = 0;
#[cfg(target_arch = "x86_64")]
static mut KERNEL_RSP0: usize = 0;

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    r#"
    .global syscall_entry
syscall_entry:
    mov [rip + {user_rsp}], rsp
    mov rsp, [rip + {kernel_rsp0}]
    push r11
    push rcx
    push rax
    mov rdi, rax
    call {dispatch}
    add rsp, 8
    pop rcx
    pop r11
    mov rsp, [rip + {user_rsp}]
    sysretq
    "#,
    user_rsp = sym USER_RSP,
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

pub fn spawn_two() {
    let code = mm::alloc_frame();
    unsafe {
        core::ptr::copy_nonoverlapping(USER_BLOB.as_ptr(), mm::hhdm(code), USER_BLOB.len());
    }
    sync_icache(mm::hhdm(code) as usize, USER_BLOB.len());

    for _ in 0..2 {
        let stack = mm::alloc_frame();
        let aspace = create_aspace(code, stack);
        let base = USER_BASE.load(Ordering::SeqCst);
        task::spawn_user(aspace, base as usize, (base + 0x3000) as usize);
        USERS_ALIVE.fetch_add(1, Ordering::SeqCst);
    }
    DID_SPAWN.store(true, Ordering::SeqCst);
}

pub fn both_exited() -> bool {
    DID_SPAWN.load(Ordering::SeqCst) && USERS_ALIVE.load(Ordering::SeqCst) == 0
}

pub fn note_exit() {
    USERS_ALIVE.fetch_sub(1, Ordering::SeqCst);
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

pub fn enter(user_rip: usize, user_rsp: usize) -> ! {
    let a = task::current_aspace();
    if a != 0 {
        switch_aspace(a);
    }
    #[cfg(target_arch = "x86_64")]
    enter_x86(user_rip, user_rsp);
    #[cfg(target_arch = "aarch64")]
    enter_aarch64(user_rip, user_rsp);
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
            options(noreturn),
        );
    }
}

#[cfg(target_arch = "aarch64")]
fn enter_aarch64(user_rip: usize, user_rsp: usize) -> ! {
    unsafe {
        if current_el() >= 2 {
            core::arch::asm!(
                "msr elr_el2, {rip}",
                "msr sp_el0, {rsp}",
                "msr spsr_el2, xzr",
                "isb",
                "eret",
                rip = in(reg) user_rip,
                rsp = in(reg) user_rsp,
                options(noreturn),
            );
        } else {
            core::arch::asm!(
                "msr elr_el1, {rip}",
                "msr sp_el0, {rsp}",
                "msr spsr_el1, xzr",
                "isb",
                "eret",
                rip = in(reg) user_rip,
                rsp = in(reg) user_rsp,
                options(noreturn),
            );
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn syscall_dispatch(nr: usize, ptr: usize, len: usize) -> usize {
    match nr {
        SYS_WRITE => sys_write(ptr, len),
        SYS_EXIT => task::die(),
        _ => usize::MAX,
    }
}

fn sys_write(ptr: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let n = len.min(128);
    if !user_range_ok(ptr, n) {
        return usize::MAX;
    }
    let mut buf = [0u8; 128];
    unsafe {
        core::ptr::copy_nonoverlapping(ptr as *const u8, buf.as_mut_ptr(), n);
    }
    task::print_bytes(&buf[..n]);
    len
}

fn user_range_ok(ptr: usize, len: usize) -> bool {
    let base = USER_BASE.load(Ordering::SeqCst) as usize;
    let end = match ptr.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    let in_code = ptr >= base && end <= base + 0x1000;
    let in_stack = ptr >= base + 0x2000 && end <= base + 0x3000;
    in_code || in_stack
}

fn create_aspace(code_phys: u64, stack_phys: u64) -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        create_aspace_x86(code_phys, stack_phys)
    }
    #[cfg(target_arch = "aarch64")]
    {
        create_aspace_aarch64(code_phys, stack_phys)
    }
}

#[cfg(target_arch = "x86_64")]
fn create_aspace_x86(code_phys: u64, stack_phys: u64) -> u64 {
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

    let slot = {
        let pml4 = unsafe { &*mm::table(pml4_phys) };
        if pml4[1] == 0 {
            1usize
        } else {
            let mut found = None;
            for i in 1..256 {
                if pml4[i] == 0 {
                    found = Some(i);
                    break;
                }
            }
            let i = found.expect("no free PML4 slot for user");
            USER_BASE.store((i as u64) << 39, Ordering::SeqCst);
            i
        }
    };
    let base = (slot as u64) << 39;
    USER_BASE.store(base, Ordering::SeqCst);

    map_page_x86(pml4_phys, base, code_phys, PRESENT | USER);
    map_page_x86(
        pml4_phys,
        base + 0x2000,
        stack_phys,
        PRESENT | WRITE | USER | NX,
    );
    pml4_phys
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
fn create_aspace_aarch64(code_phys: u64, stack_phys: u64) -> u64 {
    const TABLE: u64 = 0b11;
    const PAGE: u64 = 0b11;
    const SH_INNER: u64 = 0b11 << 8;
    const AF: u64 = 1 << 10;
    const AP_RW: u64 = 0b01 << 6; // EL1 RW, EL0 RW
    const AP_RO: u64 = 0b11 << 6; // EL1 RO, EL0 RO
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
        l3_t[0] = PAGE | (code_phys & PA) | SH_INNER | AF | AP_RO | PXN;
        l3_t[2] = PAGE | (stack_phys & PA) | SH_INNER | AF | AP_RW | PXN | UXN;
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
        // Execute VA != HHDM VA; drop the whole I-cache so EL0 sees the blob.
        core::arch::asm!("ic ialluis; dsb ish; isb", options(nostack));
    }
    let _ = (start, size);
}
