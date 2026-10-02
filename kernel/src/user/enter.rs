//! First entry to user mode after exec, and the fork-child / thread start paths.

use super::*;

pub fn enter(user_rip: usize, user_rsp: usize, user_argc: usize, user_argv: usize) -> ! {
    let a = task::current_aspace();
    if a != 0 {
        switch_aspace(a);
    }
    crate::arch::enter_user(user_rip, user_rsp, user_argc, user_argv)
}

/// Start a new task (forked child or thread) in user mode with `regs`.
pub fn enter_regs(regs: task::UserRegs) -> ! {
    let a = task::current_aspace();
    if a != 0 {
        switch_aspace(a);
    }
    crate::arch::enter_user_regs(regs)
}
