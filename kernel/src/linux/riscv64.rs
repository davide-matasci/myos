//! riscv64: the `asm-generic` syscall table, the signal frame, f0-f31/fcsr
//! and the FPU enable. musl (rv64gc) uses the D extension; native myos
//! programs are soft-float, so only Linux tasks run with `sstatus.FS` on.
//! `tp` is a general register, kept in the trap frame like the others.

use alloc::vec;

use super::generic::{uc_head, SIGINFO_BYTES, UC_MCONTEXT};
use super::signal::{siginfo, Frame};
use super::sys;
use crate::user::SyscallRegs;

pub use super::generic::{stat_bytes, syscall};

pub const MACHINE: &[u8] = b"riscv64";

/// riscv64's `struct sigaction` has no `sa_restorer` (Linux uses its vDSO);
/// handlers return through the kernel trampoline page.
pub const SIGACTION_HAS_RESTORER: bool = false;

/// `li a7, 139 (rt_sigreturn); ecall`.
pub const TRAMP_CODE: &[u8] = &[0x93, 0x08, 0xb0, 0x08, 0x73, 0x00, 0x00, 0x00];

/// `sstatus.FS = Initial`: user FP instructions allowed (Linux tasks).
pub const SSTATUS_FS_INITIAL: u64 = 1 << 13;

// Trap frame words: x0..x31, sepc, sstatus, user sp (restored into x2).
const R_PC: usize = 32;
const R_SP: usize = 34;

/// Syscall arguments: a0..a5 (x10..x15).
pub fn args(regs: &SyscallRegs, a0: usize, a1: usize, a2: usize) -> [usize; 6] {
    [a0, a1, a2, regs.word(13) as usize, regs.word(14) as usize, regs.word(15) as usize]
}

// ---- thread pointer: `tp` lives in the trap frame -------------------------

pub fn tls_read() -> Option<u64> {
    None
}

pub fn tls_write(_v: u64) {}

// ---- FP -------------------------------------------------------------------

/// f0-f31 then fcsr: the `__riscv_d_ext_state` layout.
pub const FP_BYTES: usize = 264;

/// # Safety
/// `buf` is 8-byte aligned and `FP_BYTES` long.
pub unsafe fn fp_save(buf: *mut u8) {
    unsafe {
        core::arch::asm!(
            ".option push",
            ".option arch, +d",
            // The kernel runs with whatever FS the user had: make sure FP
            // instructions are allowed here.
            "csrs sstatus, {fs}",
            "fsd f0, 0({p})",
            "fsd f1, 8({p})",
            "fsd f2, 16({p})",
            "fsd f3, 24({p})",
            "fsd f4, 32({p})",
            "fsd f5, 40({p})",
            "fsd f6, 48({p})",
            "fsd f7, 56({p})",
            "fsd f8, 64({p})",
            "fsd f9, 72({p})",
            "fsd f10, 80({p})",
            "fsd f11, 88({p})",
            "fsd f12, 96({p})",
            "fsd f13, 104({p})",
            "fsd f14, 112({p})",
            "fsd f15, 120({p})",
            "fsd f16, 128({p})",
            "fsd f17, 136({p})",
            "fsd f18, 144({p})",
            "fsd f19, 152({p})",
            "fsd f20, 160({p})",
            "fsd f21, 168({p})",
            "fsd f22, 176({p})",
            "fsd f23, 184({p})",
            "fsd f24, 192({p})",
            "fsd f25, 200({p})",
            "fsd f26, 208({p})",
            "fsd f27, 216({p})",
            "fsd f28, 224({p})",
            "fsd f29, 232({p})",
            "fsd f30, 240({p})",
            "fsd f31, 248({p})",
            "frcsr {t}",
            "sw {t}, 256({p})",
            ".option pop",
            p = in(reg) buf,
            fs = in(reg) SSTATUS_FS_INITIAL,
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
            ".option push",
            ".option arch, +d",
            "csrs sstatus, {fs}",
            "fld f0, 0({p})",
            "fld f1, 8({p})",
            "fld f2, 16({p})",
            "fld f3, 24({p})",
            "fld f4, 32({p})",
            "fld f5, 40({p})",
            "fld f6, 48({p})",
            "fld f7, 56({p})",
            "fld f8, 64({p})",
            "fld f9, 72({p})",
            "fld f10, 80({p})",
            "fld f11, 88({p})",
            "fld f12, 96({p})",
            "fld f13, 104({p})",
            "fld f14, 112({p})",
            "fld f15, 120({p})",
            "fld f16, 128({p})",
            "fld f17, 136({p})",
            "fld f18, 144({p})",
            "fld f19, 152({p})",
            "fld f20, 160({p})",
            "fld f21, 168({p})",
            "fld f22, 176({p})",
            "fld f23, 184({p})",
            "fld f24, 192({p})",
            "fld f25, 200({p})",
            "fld f26, 208({p})",
            "fld f27, 216({p})",
            "fld f28, 224({p})",
            "fld f29, 232({p})",
            "fld f30, 240({p})",
            "fld f31, 248({p})",
            "lw {t}, 256({p})",
            "fscsr {t}",
            ".option pop",
            p = in(reg) buf,
            fs = in(reg) SSTATUS_FS_INITIAL,
            t = out(reg) _,
            options(nostack),
        );
    }
}

#[repr(C, align(16))]
struct Fp([u8; FP_BYTES]);

// ---- signal frame ---------------------------------------------------------

/// `uc_mcontext`: `__gregs[32]` (pc, then x1..x31), then the FP state
/// union (528 bytes; the D layout fits at its start).
const MC_GREGS: usize = UC_MCONTEXT;
const MC_FP: usize = UC_MCONTEXT + 256;
const UC_BYTES: usize = MC_FP + 528;

fn put_u64(b: &mut [u8], off: usize, v: u64) {
    b[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

fn get_u64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}

/// Linux `rt_sigframe`: `[siginfo][ucontext]`.
pub fn deliver(regs: &mut SyscallRegs, f: &Frame) -> Option<usize> {
    let sp = regs.word(R_SP) as usize;
    let frame_va = sp.checked_sub(SIGINFO_BYTES + UC_BYTES)? & !15;
    let uc_va = frame_va + SIGINFO_BYTES;

    let mut uc = vec![0u8; UC_BYTES];
    uc_head(&mut uc, f.mask);
    put_u64(&mut uc, MC_GREGS, f.pc as u64);
    for i in 1..32 {
        let v = match i {
            2 => sp as u64,
            10 => f.ret as u64,
            _ => regs.word(i),
        };
        put_u64(&mut uc, MC_GREGS + i * 8, v);
    }
    let mut fp = Fp([0; FP_BYTES]);
    unsafe { fp_save(fp.0.as_mut_ptr()) };
    uc[MC_FP..MC_FP + FP_BYTES].copy_from_slice(&fp.0);

    sys::put(frame_va, &siginfo(f.sig)).ok()?;
    sys::put(uc_va, &uc).ok()?;

    regs.set_word(1, f.restorer as u64); // ra
    regs.set_word(11, frame_va as u64); // a1 = &siginfo
    regs.set_word(12, uc_va as u64); // a2 = &ucontext
    regs.set_word(2, frame_va as u64);
    regs.set_word(R_SP, frame_va as u64);
    regs.set_word(R_PC, f.handler as u64);
    // a0 (the result register) carries the signal number.
    Some(f.sig)
}

/// Undo [`deliver`]: `sp` is back at the frame. Returns `(a0, mask)`.
pub fn sigreturn(regs: &mut SyscallRegs) -> Option<(usize, u64)> {
    let frame_va = regs.word(R_SP) as usize;
    let mut uc = vec![0u8; UC_BYTES];
    sys::get_bytes(frame_va + SIGINFO_BYTES, &mut uc).ok()?;
    let mut fp = Fp([0; FP_BYTES]);
    fp.0.copy_from_slice(&uc[MC_FP..MC_FP + FP_BYTES]);
    unsafe { fp_restore(fp.0.as_ptr()) };
    for i in 1..32 {
        let v = get_u64(&uc, MC_GREGS + i * 8);
        regs.set_word(if i == 2 { R_SP } else { i }, v);
        if i == 2 {
            regs.set_word(2, v);
        }
    }
    regs.set_word(R_PC, get_u64(&uc, MC_GREGS));
    Some((get_u64(&uc, MC_GREGS + 10 * 8) as usize, get_u64(&uc, super::generic::UC_SIGMASK)))
}
