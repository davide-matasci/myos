//! AArch64: save x19-x28, x29, x30 and sp; restore; ret.

use core::arch::global_asm;

global_asm!(
    r#"
    .global task_switch
task_switch:
    stp x19, x20, [sp, #-16]!
    stp x21, x22, [sp, #-16]!
    stp x23, x24, [sp, #-16]!
    stp x25, x26, [sp, #-16]!
    stp x27, x28, [sp, #-16]!
    stp x29, x30, [sp, #-16]!
    mov x2, sp
    str x2, [x0]
    mov sp, x1
    ldp x29, x30, [sp], #16
    ldp x27, x28, [sp], #16
    ldp x25, x26, [sp], #16
    ldp x23, x24, [sp], #16
    ldp x21, x22, [sp], #16
    ldp x19, x20, [sp], #16
    ret
    "#
);

unsafe extern "C" {
    pub fn task_switch(old_sp: *mut usize, new_sp: usize);
}

/// Seed a new stack so the first `task_switch` `ret`s into `entry` via x30.
pub unsafe fn seed_stack(stack: *mut u8, size: usize, entry: usize) -> usize {
    let top = stack as usize + size;
    let sp = top - 16 * 6;
    let p = sp as *mut u64;
    // Lowest pair is the last `stp` (x29, x30); first restore is `ldp x29, x30`.
    unsafe {
        p.add(0).write(0);
        p.add(1).write(entry as u64);
        for i in 2..12 {
            p.add(i).write(0);
        }
    }
    debug_assert_eq!(sp % 16, 0);
    sp
}
