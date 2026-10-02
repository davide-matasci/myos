//! User FP/SIMD register file: save/restore for `task::fpu`.


/// Laid out as the frame's `fpsimd_context`: head (magic, size), FPSR,
/// FPCR, then v0-v31.
pub const BYTES: usize = 528;

/// # Safety
/// `buf` is 16-byte aligned and `BYTES` long.
pub unsafe fn save(buf: *mut u8) {
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
/// As [`save`].
pub unsafe fn restore(buf: *const u8) {
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


    pub fn live() -> bool {
        true
    }
