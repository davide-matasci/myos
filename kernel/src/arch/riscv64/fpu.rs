//! User FP/SIMD register file: save/restore for `task::fpu`.


    /// `sstatus.FS = Initial`: FP instructions allowed.
    pub const SSTATUS_FS_INITIAL: u64 = 1 << 13;

/// f0-f31 then fcsr: the `__riscv_d_ext_state` layout.
pub const BYTES: usize = 264;

/// # Safety
/// `buf` is 8-byte aligned and `BYTES` long.
pub unsafe fn save(buf: *mut u8) {
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
/// As [`save`].
pub unsafe fn restore(buf: *const u8) {
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


    /// Native riscv64 programs are soft-float and run with `sstatus.FS`
    /// off; only tasks that have the FPU on (Linux tasks) need saving.
    pub fn live() -> bool {
        let s: u64;
        unsafe { core::arch::asm!("csrr {}, sstatus", out(reg) s, options(nomem, nostack)) };
        s & (3 << 13) != 0
    }
