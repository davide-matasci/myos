//! x86_64: save rbx, rbp, r12-r15 and rsp; swap rsp; ret.

use core::arch::global_asm;

global_asm!(
    r#"
    .global task_switch
task_switch:
    push %rbx
    push %rbp
    push %r12
    push %r13
    push %r14
    push %r15
    movq %rsp, (%rdi)
    movq %rsi, %rsp
    pop %r15
    pop %r14
    pop %r13
    pop %r12
    pop %rbp
    pop %rbx
    ret
    "#
);

unsafe extern "C" {
    pub fn task_switch(old_sp: *mut usize, new_sp: usize);
}

/// Seed a new stack so the first `task_switch` `ret`s into `entry`.
///
/// Frame is 64 bytes (6 callee-saved + RIP + pad) so saved SP is 16-byte
/// aligned and SysV sees RSP % 16 == 8 on function entry.
pub unsafe fn seed_stack(stack: *mut u8, size: usize, entry: usize) -> usize {
    let top = stack as usize + size;
    let mut sp = top;
    sp -= 8; // pad
    sp -= 8;
    unsafe {
        core::ptr::write(sp as *mut usize, entry);
        for _ in 0..6 {
            sp -= 8;
            core::ptr::write(sp as *mut usize, 0);
        }
    }
    debug_assert_eq!(sp % 16, 0);
    sp
}
