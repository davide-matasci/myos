//! Fault reporting for this arch's trap handlers (on top of `crate::exception`).

use alloc::format;
use alloc::string::String;

use crate::exception::{fatal_line, task_ctx, user_fault_kill};
use crate::task;

pub fn x86_page_fault(cr2: u64, rip: u64, rsp: u64, code: u64, user: bool) -> ! {
    // Dump a few stack words so the faulting frame's return address (caller)
    // is visible in the log — same idea as the #GP insn dump below.
    let mut stack = String::new();
    if user {
        let a = task::current_aspace();
        if a != 0 {
            for i in 0..8usize {
                let addr = rsp as usize + i * 8;
                let mut v: u64 = 0;
                let mut ok = true;
                for b in 0..8usize {
                    match crate::user::try_read_user_u8(a, addr + b) {
                        Some(byte) => v |= (byte as u64) << (8 * b),
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if !ok {
                    break;
                }
                stack.push_str(&format!(" [{:#x}]={:#x}", addr, v));
            }
        }
        // Match aarch64/riscv: a userspace SEGV (including an accidental
        // kernel-VA / HHDM deref) must kill the faulting task, not halt QEMU.
        // Halting turned UEFI boot-mini `cat | cat` into a machine-wide
        // `[ FAIL ] exception` (cr2 in HHDM, code=0x5) instead of a child SEGV.
        user_fault_kill(
            "page fault",
            &format!("cr2={cr2:#x} rip={rip:#x} rsp={rsp:#x} code={code:#x}{ctx} stack{stack}", ctx = task_ctx()),
        );
    }
    fatal_line(&format!(
        "page fault cr2={cr2:#x} rip={rip:#x} rsp={rsp:#x} code={code:#x} kernel{ctx} stack{stack}",
        ctx = task_ctx(),
    ));
}

pub fn x86_general_protection(rip: u64, rsp: u64, code: u64, rbp: u64, user: bool) -> ! {
    // Dump faulting bytes so pipe/#GP CI failures are diagnosable without artifacts.
    let mut insn = [0u8; 16];
    let mut n = 0usize;
    let aspace = task::current_aspace();
    if aspace != 0 {
        for i in 0..16 {
            match crate::user::try_read_user_u8(aspace, rip as usize + i) {
                Some(b) => {
                    insn[i] = b;
                    n = i + 1;
                }
                None => break,
            }
        }
    }
    let mut hex = String::new();
    for i in 0..n {
        if i > 0 {
            hex.push(' ');
        }
        hex.push_str(&format!("{:02x}", insn[i]));
    }
    if hex.is_empty() {
        hex.push_str("unreadable");
    }
    if user {
        // Ring-3 #GP (e.g. an `int3` through the DPL-0 breakpoint gate, a
        // non-canonical address): kill the task like a user page fault
        // instead of halting the machine (os-test basic/signal/sigismember
        // took the whole guest down with `[ FAIL ] exception`).
        user_fault_kill(
            "general protection",
            &format!(
                "rip={rip:#x} rsp={rsp:#x} code={code:#x} insn=[{hex}]{ctx}",
                ctx = task_ctx(),
            ),
        );
    }
    fatal_line(&format!(
        "general protection rip={rip:#x} rsp={rsp:#x} rbp={rbp:#x} rsp16={} rbp16={} code={code:#x} insn=[{hex}]{ctx}",
        rsp & 15,
        rbp & 15,
        ctx = task_ctx(),
    ));
}

/// Any other exception (`name`: `#UD`, `#DE`, `#MF`, ...): from ring 3 it
/// kills the faulting task, whose program ran a trapping instruction; from
/// the kernel it is fatal.
pub fn x86_exception(name: &str, rip: u64, rsp: u64, code: u64, user: bool) -> ! {
    if user {
        user_fault_kill(name, &format!("rip={rip:#x} rsp={rsp:#x} code={code:#x}{ctx}", ctx = task_ctx()));
    }
    fatal_line(&format!("{name} rip={rip:#x} rsp={rsp:#x} code={code:#x} kernel{ctx}", ctx = task_ctx()));
}

pub fn x86_double_fault(rip: u64, rsp: u64) -> ! {
    fatal_line(&format!(
        "double fault rip={rip:#x} rsp={rsp:#x}{ctx}",
        ctx = task_ctx(),
    ));
}
