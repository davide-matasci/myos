//! RISC-V: save s0-s11 and ra; restore; ret.

use core::arch::global_asm;

const FRAME: i32 = 16 * 7;

global_asm!(
    r#"
    .global task_switch
task_switch:
    addi sp, sp, -112
    sd s0, 0(sp)
    sd s1, 8(sp)
    sd s2, 16(sp)
    sd s3, 24(sp)
    sd s4, 32(sp)
    sd s5, 40(sp)
    sd s6, 48(sp)
    sd s7, 56(sp)
    sd s8, 64(sp)
    sd s9, 72(sp)
    sd s10, 80(sp)
    sd s11, 88(sp)
    sd ra, 96(sp)
    mv t0, sp
    sd t0, 0(a0)
    mv sp, a1
    ld s0, 0(sp)
    ld s1, 8(sp)
    ld s2, 16(sp)
    ld s3, 24(sp)
    ld s4, 32(sp)
    ld s5, 40(sp)
    ld s6, 48(sp)
    ld s7, 56(sp)
    ld s8, 64(sp)
    ld s9, 72(sp)
    ld s10, 80(sp)
    ld s11, 88(sp)
    ld ra, 96(sp)
    addi sp, sp, 112
    ret
    "#
);

unsafe extern "C" {
    pub fn task_switch(old_sp: *mut usize, new_sp: usize);
}

/// Seed a new stack so the first `task_switch` `ret`s into `entry` via ra.
pub unsafe fn seed_stack(stack: *mut u8, size: usize, entry: usize) -> usize {
    let top = stack as usize + size;
    let sp = top - FRAME as usize;
    let p = sp as *mut u64;
    unsafe {
        for i in 0..12 {
            p.add(i).write(0);
        }
        p.add(12).write(entry as u64);
    }
    debug_assert_eq!(sp % 16, 0);
    sp
}
