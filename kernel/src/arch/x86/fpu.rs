//! User FP/SIMD register file: save/restore for `task::fpu`.
//!
//! On a CPU with XSAVE and AVX the kernel turns AVX state on ([`enable`]:
//! CR4.OSXSAVE, XCR0 = x87 | SSE | AVX), as a program that finds AVX in
//! CPUID expects (JavaScriptCore's JIT emits AVX on that alone: without the
//! state on, its first VEX instruction is an invalid opcode), and the image
//! is XSAVE's. Elsewhere it is FXSAVE's, the image's first 512 bytes.

use core::arch::x86_64::{__cpuid, __cpuid_count};
use core::sync::atomic::{AtomicBool, Ordering};

/// The XSAVE image of x87, SSE and AVX (standard format): the FXSAVE
/// region (x87, MXCSR, xmm0-15), the XSAVE header, ymm0-15's upper halves.
pub const BYTES: usize = 832;

/// The XSAVE header's offset in the image.
const HEADER: usize = 512;

/// The registers' initial state as an image: x87's control word 0x37f (all
/// exceptions masked, every register tag empty), MXCSR 0x1f80 (the same
/// for SSE; XRSTOR loads it from here whatever the header says), every
/// register zero, and a zero XSAVE header, on which XRSTOR puts every
/// component in its initial state.
pub const INIT: [u8; BYTES] = {
    let mut image = [0u8; BYTES];
    image[0] = 0x7f;
    image[1] = 0x03;
    image[24] = 0x80;
    image[25] = 0x1f;
    image
};

/// XCR0's x87, SSE and AVX state bits.
const XCR0: u64 = 0b111;

/// AVX state is on: the images are XSAVE's.
static XSAVE: AtomicBool = AtomicBool::new(false);

/// Turn AVX state on for this CPU when it has XSAVE and AVX (the boot CPU
/// decides, before any task switches; the others follow it as they start).
pub fn enable(boot: bool) {
    if boot {
        let leaf1 = __cpuid(1);
        let (xsave, avx) = (leaf1.ecx & (1 << 26) != 0, leaf1.ecx & (1 << 28) != 0);
        // The components XCR0 may hold (CPUID 0xd), and where AVX's lies in
        // the image, which must fit BYTES.
        if !xsave || !avx || __cpuid_count(0xd, 0).eax as u64 & XCR0 != XCR0 {
            return;
        }
        let ymm = __cpuid_count(0xd, 2);
        if (ymm.ebx + ymm.eax) as usize > BYTES {
            return;
        }
        set_xcr0();
        XSAVE.store(true, Ordering::Relaxed);
    } else if XSAVE.load(Ordering::Relaxed) {
        set_xcr0();
    }
}

fn set_xcr0() {
    const CR4_OSXSAVE: u64 = 1 << 18;
    unsafe {
        let mut cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4);
        core::arch::asm!("mov cr4, {}", in(reg) cr4 | CR4_OSXSAVE);
        core::arch::asm!("xsetbv", in("ecx") 0, in("eax") XCR0 as u32, in("edx") 0);
    }
}

/// # Safety
/// `buf` is 64-byte aligned and `BYTES` long.
pub unsafe fn save(buf: *mut u8) {
    if XSAVE.load(Ordering::Relaxed) {
        unsafe {
            core::arch::asm!("xsave64 [{}]", in(reg) buf, in("eax") XCR0 as u32, in("edx") 0,
                options(nostack, preserves_flags))
        };
    } else {
        unsafe { core::arch::asm!("fxsave64 [{}]", in(reg) buf, options(nostack, preserves_flags)) };
    }
}

/// # Safety
/// As [`save`]; `buf` holds an image whose MXCSR is valid for this CPU and,
/// with XSAVE, a header XRSTOR accepts (an image [`save`] wrote, or one
/// [`sanitize`] went over).
pub unsafe fn restore(buf: *const u8) {
    if XSAVE.load(Ordering::Relaxed) {
        unsafe {
            core::arch::asm!("xrstor64 [{}]", in(reg) buf, in("eax") XCR0 as u32, in("edx") 0,
                options(nostack, preserves_flags))
        };
    } else {
        unsafe { core::arch::asm!("fxrstor64 [{}]", in(reg) buf, options(nostack, preserves_flags)) };
    }
}

/// Make an image from user memory (a Linux signal frame's) one XRSTOR
/// takes instead of faulting in the kernel: the header's state bits within
/// XCR0, the standard format, its reserved bytes zero.
pub fn sanitize(image: &mut [u8; BYTES]) {
    if XSAVE.load(Ordering::Relaxed) {
        let bv = u64::from_le_bytes(image[HEADER..HEADER + 8].try_into().unwrap()) & XCR0;
        image[HEADER..HEADER + 64].fill(0);
        image[HEADER..HEADER + 8].copy_from_slice(&bv.to_le_bytes());
    }
}

/// Whether the outgoing task's registers need saving.
pub fn live() -> bool {
    true
}
