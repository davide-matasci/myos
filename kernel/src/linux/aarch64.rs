//! aarch64: the `asm-generic` syscall table, the signal frame (with an
//! `fpsimd_context`), q0-q31/FPSR/FPCR and `tpidr_el0` (musl's thread
//! pointer, which userspace sets itself).

use alloc::vec;

use super::generic::{uc_head, SIGINFO_BYTES, UC_MCONTEXT};
use super::signal::{siginfo, Frame};
use super::sys;
use crate::user::SyscallRegs;

pub use super::generic::{stat_bytes, syscall};

pub const MACHINE: &[u8] = b"aarch64";

/// `struct sigaction` carries `sa_restorer` (musl sets it; without it the
/// kernel trampoline page stands in for the vDSO's).
pub const SIGACTION_HAS_RESTORER: bool = true;

/// `mov x8, #139 (rt_sigreturn); svc #0`.
pub const TRAMP_CODE: &[u8] = &[0x68, 0x11, 0x80, 0xd2, 0x01, 0x00, 0x00, 0xd4];

// Trap frame words (`lower_sync`): x0..x30, elr, spsr, sp_el0.
const R_PC: usize = 32;
const R_SPSR: usize = 33;
const R_SP: usize = 34;
/// SPSR bits userspace may change: NZCV.
const NZCV: u64 = 0xf000_0000;

/// Syscall arguments: x0..x5.
pub fn args(regs: &SyscallRegs, a0: usize, a1: usize, a2: usize) -> [usize; 6] {
    [a0, a1, a2, regs.word(3) as usize, regs.word(4) as usize, regs.word(5) as usize]
}

// ---- thread pointer ---------------------------------------------------------

pub fn tls_read() -> Option<u64> {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, tpidr_el0", out(reg) v, options(nomem, nostack)) };
    Some(v)
}

pub fn tls_write(v: u64) {
    unsafe { core::arch::asm!("msr tpidr_el0, {}", in(reg) v, options(nostack)) };
}

// ---- FP / SIMD ------------------------------------------------------------

/// Laid out as the frame's `fpsimd_context`: head (magic, size), FPSR,
/// FPCR, then v0-v31.
pub const FP_BYTES: usize = 528;
const FPSIMD_MAGIC: u32 = 0x4650_8001;

/// # Safety
/// `buf` is 16-byte aligned and `FP_BYTES` long.
pub unsafe fn fp_save(buf: *mut u8) {
    unsafe {
        core::arch::asm!(
            ".arch_extension fp",
            ".arch_extension simd",
            "stp q0, q1, [{p}, #16]",
            "stp q2, q3, [{p}, #48]",
            "stp q4, q5, [{p}, #80]",
            "stp q6, q7, [{p}, #112]",
            "stp q8, q9, [{p}, #144]",
            "stp q10, q11, [{p}, #176]",
            "stp q12, q13, [{p}, #208]",
            "stp q14, q15, [{p}, #240]",
            "stp q16, q17, [{p}, #272]",
            "stp q18, q19, [{p}, #304]",
            "stp q20, q21, [{p}, #336]",
            "stp q22, q23, [{p}, #368]",
            "stp q24, q25, [{p}, #400]",
            "stp q26, q27, [{p}, #432]",
            "stp q28, q29, [{p}, #464]",
            "stp q30, q31, [{p}, #496]",
            "mrs {t}, fpsr",
            "str {t:w}, [{p}, #8]",
            "mrs {t}, fpcr",
            "str {t:w}, [{p}, #12]",
            p = in(reg) buf,
            t = out(reg) _,
            options(nostack),
        );
    }
}

/// # Safety
/// As [`fp_save`].
pub unsafe fn fp_restore(buf: *const u8) {
    unsafe {
        core::arch::asm!(
            ".arch_extension fp",
            ".arch_extension simd",
            "ldp q0, q1, [{p}, #16]",
            "ldp q2, q3, [{p}, #48]",
            "ldp q4, q5, [{p}, #80]",
            "ldp q6, q7, [{p}, #112]",
            "ldp q8, q9, [{p}, #144]",
            "ldp q10, q11, [{p}, #176]",
            "ldp q12, q13, [{p}, #208]",
            "ldp q14, q15, [{p}, #240]",
            "ldp q16, q17, [{p}, #272]",
            "ldp q18, q19, [{p}, #304]",
            "ldp q20, q21, [{p}, #336]",
            "ldp q22, q23, [{p}, #368]",
            "ldp q24, q25, [{p}, #400]",
            "ldp q26, q27, [{p}, #432]",
            "ldp q28, q29, [{p}, #464]",
            "ldp q30, q31, [{p}, #496]",
            "ldr {t:w}, [{p}, #8]",
            "msr fpsr, {t}",
            "ldr {t:w}, [{p}, #12]",
            "msr fpcr, {t}",
            p = in(reg) buf,
            t = out(reg) _,
            options(nostack),
        );
    }
}

#[repr(C, align(16))]
struct Fp([u8; FP_BYTES]);

// ---- signal frame ---------------------------------------------------------

/// `uc_mcontext`: fault_address, regs[31], sp, pc, pstate, then the
/// 4096-byte `__reserved` area (16-byte aligned) holding the contexts.
const MC_REGS: usize = UC_MCONTEXT + 8;
const MC_SP: usize = UC_MCONTEXT + 256;
const MC_PC: usize = UC_MCONTEXT + 264;
const MC_PSTATE: usize = UC_MCONTEXT + 272;
const MC_RESERVED: usize = UC_MCONTEXT + 288;
const UC_BYTES: usize = MC_RESERVED + 4096;

fn put_u64(b: &mut [u8], off: usize, v: u64) {
    b[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

fn get_u64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}

/// Linux `rt_sigframe`: `[siginfo][ucontext]` then a frame record
/// `{x29, pc}` that x29 points at, as the arm64 kernel lays it out.
pub fn deliver(regs: &mut SyscallRegs, f: &Frame) -> Option<usize> {
    let sp = regs.word(R_SP) as usize;
    let frame_va = sp.checked_sub(SIGINFO_BYTES + UC_BYTES + 16)? & !15;
    let uc_va = frame_va + SIGINFO_BYTES;
    let rec_va = uc_va + UC_BYTES;

    let mut uc = vec![0u8; UC_BYTES];
    uc_head(&mut uc, f.mask);
    for i in 0..31 {
        let v = if i == 0 { f.ret as u64 } else { regs.word(i) };
        put_u64(&mut uc, MC_REGS + i * 8, v);
    }
    put_u64(&mut uc, MC_SP, sp as u64);
    put_u64(&mut uc, MC_PC, f.pc as u64);
    put_u64(&mut uc, MC_PSTATE, regs.word(R_SPSR));
    let mut fp = Fp([0; FP_BYTES]);
    unsafe { fp_save(fp.0.as_mut_ptr()) };
    fp.0[0..4].copy_from_slice(&FPSIMD_MAGIC.to_le_bytes());
    fp.0[4..8].copy_from_slice(&(FP_BYTES as u32).to_le_bytes());
    uc[MC_RESERVED..MC_RESERVED + FP_BYTES].copy_from_slice(&fp.0);
    // A zero head after it ends the context list.

    let mut rec = [0u8; 16];
    put_u64(&mut rec, 0, regs.word(29));
    put_u64(&mut rec, 8, f.pc as u64);
    sys::put(frame_va, &siginfo(f.sig)).ok()?;
    sys::put(uc_va, &uc).ok()?;
    sys::put(rec_va, &rec).ok()?;

    regs.set_word(1, frame_va as u64);
    regs.set_word(2, uc_va as u64);
    regs.set_word(29, rec_va as u64);
    regs.set_word(30, f.restorer as u64);
    regs.set_word(R_SP, frame_va as u64);
    regs.set_word(R_PC, f.handler as u64);
    // x0 (the result register) carries the signal number.
    Some(f.sig)
}

/// Undo [`deliver`]: `sp` is back at the frame. Returns `(x0, mask)`.
pub fn sigreturn(regs: &mut SyscallRegs) -> Option<(usize, u64)> {
    let frame_va = regs.word(R_SP) as usize;
    let mut uc = vec![0u8; UC_BYTES];
    sys::get_bytes(frame_va + SIGINFO_BYTES, &mut uc).ok()?;
    let head = u32::from_le_bytes(uc[MC_RESERVED..MC_RESERVED + 4].try_into().unwrap());
    if head == FPSIMD_MAGIC {
        let mut fp = Fp([0; FP_BYTES]);
        fp.0.copy_from_slice(&uc[MC_RESERVED..MC_RESERVED + FP_BYTES]);
        unsafe { fp_restore(fp.0.as_ptr()) };
    }
    for i in 1..31 {
        regs.set_word(i, get_u64(&uc, MC_REGS + i * 8));
    }
    regs.set_word(R_SP, get_u64(&uc, MC_SP));
    regs.set_word(R_PC, get_u64(&uc, MC_PC));
    let pstate = get_u64(&uc, MC_PSTATE);
    regs.set_word(R_SPSR, (regs.word(R_SPSR) & !NZCV) | (pstate & NZCV));
    Some((get_u64(&uc, MC_REGS) as usize, get_u64(&uc, super::generic::UC_SIGMASK)))
}
