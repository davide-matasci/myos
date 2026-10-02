//! Fault reporting for this arch's trap handlers (on top of `crate::exception`).

use alloc::format;

use crate::exception::{fatal_line, task_ctx};

pub fn riscv64_trap(code: u64, sepc: u64, stval: u64) -> ! {
    fatal_line(&format!(
        "trap scause={code:#x} sepc={sepc:#x} stval={stval:#x}{ctx}",
        ctx = task_ctx(),
    ));
}
