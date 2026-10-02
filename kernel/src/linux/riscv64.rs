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
pub use crate::arch::fpu::SSTATUS_FS_INITIAL;

// Trap frame words: x0..x31, sepc, sstatus, user sp (restored into x2).
const R_PC: usize = 32;
const R_SP: usize = 34;

/// Syscall arguments: a0..a5 (x10..x15).
pub fn args(regs: &SyscallRegs, a0: usize, a1: usize, a2: usize) -> [usize; 6] {
    [a0, a1, a2, regs.word(13) as usize, regs.word(14) as usize, regs.word(15) as usize]
}

// ---- FP -------------------------------------------------------------------

/// f0-f31 then fcsr: the `__riscv_d_ext_state` layout (see `task::fpu`).
pub use crate::task::fpu::{restore as fp_restore, save as fp_save, BYTES as FP_BYTES};

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
