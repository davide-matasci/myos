//! User FP/SIMD register file: save/restore for `task::fpu`.


/// The FXSAVE image (x87, MXCSR, xmm0-15).
pub const BYTES: usize = 512;

/// # Safety
/// `buf` is 16-byte aligned and `BYTES` long.
pub unsafe fn save(buf: *mut u8) {
    unsafe { core::arch::asm!("fxsave64 [{}]", in(reg) buf, options(nostack, preserves_flags)) };
}

/// # Safety
/// As [`save`]; `buf` holds an image whose MXCSR is valid for this CPU.
pub unsafe fn restore(buf: *const u8) {
    unsafe { core::arch::asm!("fxrstor64 [{}]", in(reg) buf, options(nostack, preserves_flags)) };
}


    /// Whether the outgoing task's registers need saving.
    pub fn live() -> bool {
        true
    }
