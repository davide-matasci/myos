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

fn task_ctx() -> String {
    let id = task::current_id();
    match task::current_user_pc_sp() {
        Some((rip, rsp)) => format!(" task={id} user rip={rip:#x} rsp={rsp:#x}"),
        None => format!(" task={id} kernel"),
    }
}

#[cfg(target_arch = "x86_64")]
pub fn x86_page_fault(cr2: u64, rip: u64, rsp: u64, code: u64, user: bool) -> ! {
    fatal_line(&format!(
        "page fault cr2={cr2:#x} rip={rip:#x} rsp={rsp:#x} code={code:#x} {mode}{ctx}",
        mode = if user { "user" } else { "kernel" },
        ctx = task_ctx(),
    ));
}

#[cfg(target_arch = "x86_64")]
pub fn x86_general_protection(rip: u64, rsp: u64, code: u64, rbp: u64) -> ! {
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
    fatal_line(&format!(
        "general protection rip={rip:#x} rsp={rsp:#x} rbp={rbp:#x} rsp16={} rbp16={} code={code:#x} insn=[{hex}]{ctx}",
        rsp & 15,
        rbp & 15,
        ctx = task_ctx(),
    ));
}

#[cfg(target_arch = "x86_64")]
pub fn x86_double_fault(rip: u64, rsp: u64) -> ! {
    fatal_line(&format!(
        "double fault rip={rip:#x} rsp={rsp:#x}{ctx}",
        ctx = task_ctx(),
    ));
}

#[cfg(target_arch = "aarch64")]
pub fn aarch64_sync_abort(kind: &str, esr: u64, elr: u64, far: u64, sp_el0: Option<u64>) -> ! {
    let ec = (esr >> 26) & 0x3f;
    let sp = sp_el0
        .map(|sp| format!(" sp_el0={sp:#x}"))
        .unwrap_or_default();
    fatal_line(&format!(
        "{kind} ec={ec:#x} esr={esr:#x} elr={elr:#x} far={far:#x}{sp}{ctx}",
        ctx = task_ctx(),
    ));
}

#[cfg(target_arch = "riscv64")]
pub fn riscv64_page_fault(kind: &str, stval: u64, sepc: u64, user_sp: u64) -> ! {
    fatal_line(&format!(
        "{kind} stval={stval:#x} sepc={sepc:#x} sp={user_sp:#x}{ctx}",
        ctx = task_ctx(),
    ));
}

#[cfg(target_arch = "riscv64")]
pub fn riscv64_trap(code: u64, sepc: u64, stval: u64) -> ! {
    fatal_line(&format!(
        "trap scause={code:#x} sepc={sepc:#x} stval={stval:#x}{ctx}",
        ctx = task_ctx(),
    ));
}
