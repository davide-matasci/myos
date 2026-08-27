//! QEMU `virt` entry.
//!
//! RAM starts at 0x4000_0000. On a bare-metal (ELF) boot QEMU parks the DTB
//! at the start of RAM, so the kernel is linked at 0x4008_0000 — the usual
//! ARM64 Image offset — via `link.ld`.

use core::arch::naked_asm;

#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.boot")]
unsafe extern "C" fn _start() -> ! {
    naked_asm!(
        // Zero BSS. QEMU's ELF loader usually does this; don't depend on it.
        "ldr x0, =__bss_start",
        "ldr x1, =__bss_end",
        "2:",
        "cmp x0, x1",
        "b.hs 3f",
        "str xzr, [x0], #8",
        "b 2b",
        "3:",
        "ldr x0, =__stack_top",
        "mov sp, x0",
        "b {main}",
        main = sym crate::kernel_main_aarch64,
    );
}
