//! Log CPU exceptions to serial before halting (used by arch interrupt handlers).

extern crate alloc;

use alloc::format;
use alloc::string::String;

use crate::arch;
use crate::console;
use crate::task;

pub fn fatal_line(line: &str) -> ! {
    console::status_fail(&format!("exception: {line}"));
    console::flush();
    arch::exit_qemu(arch::QEMU_FAILURE);
    arch::halt();
}

/// Terminate the current user task after a synchronous fault in EL0/U-mode.
///
/// Prefer this over [`fatal_line`] for translation/permission faults at FAR=0
/// and friends: a userspace null deref must not halt the whole machine (that
/// turned `cat | cat` + ^C into a CI `[ FAIL ] exception` on aarch64). Exit
/// status matches the shell convention for SIGSEGV (`128 + 11`).
/// Warn-only variant: print and return. Used before `user_fault_kill` when
/// the detail is split across two lines and the harness may kill QEMU on the
/// first WARN before the second line reaches serial.
#[allow(dead_code)]
pub fn user_fault_warn(kind: &str, detail: &str) {
    console::status_warn(&format!("user fault: {kind} {detail}"));
    console::flush();
}

pub fn user_fault_kill(kind: &str, detail: &str) -> ! {
    // Avoid the substring `exception:` so the boot test host, which ends the run on it,
    // treat any `exception:` as a hard fail stay quiet when a *child* faults.
    console::status_warn(&format!("user fault: {kind} {detail}"));
    console::flush();
    crate::user::set_syscall_frame(core::ptr::null_mut());
    task::user_exit(128u8.wrapping_add(11));
}


/// A synchronous CPU exception (`name`) taken on x86: a user one kills the
/// faulting task (its program ran a trapping instruction), a kernel one is
/// fatal. Used for the vectors that are otherwise left non-present and would
/// otherwise escalate to a double fault (`#UD`, `#DE`, `#MF`, ...).
pub fn x86_user_exception(name: &str, rip: u64, rsp: u64, code: u64, user: bool) -> ! {
    if user {
        user_fault_kill(name, &format!("rip={rip:#x} rsp={rsp:#x} code={code:#x}{}", task_ctx()));
    }
    fatal_line(&format!("{name} rip={rip:#x} rsp={rsp:#x} code={code:#x} kernel{}", task_ctx()));
}

pub fn task_ctx() -> String {
    let id = task::current_id();
    // A dead canary says the task ran off its kernel stack before the fault.
    let stack = match task::current_stack_intact() {
        Some(false) => " kstack overflowed",
        _ => "",
    };
    match task::current_user_pc_sp() {
        Some((rip, rsp)) => format!(" task={id} user rip={rip:#x} rsp={rsp:#x}{stack}"),
        None => format!(" task={id} kernel{stack}"),
    }
}
