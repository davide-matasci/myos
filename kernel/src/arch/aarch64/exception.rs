//! Fault reporting for this arch's trap handlers (on top of `crate::exception`).

use alloc::format;

use crate::exception::{fatal_line, task_ctx};

/// `sp_el0` for a fault in user mode, `lr_sp` (x30 and sp at the fault) for
/// one in the kernel.
pub fn aarch64_sync_abort(
    kind: &str,
    esr: u64,
    elr: u64,
    far: u64,
    sp_el0: Option<u64>,
    lr_sp: Option<(u64, u64)>,
) -> ! {
    let ec = (esr >> 26) & 0x3f;
    let sp = sp_el0
        .map(|sp| format!(" sp_el0={sp:#x}"))
        .unwrap_or_default();
    let lr = lr_sp
        .map(|(lr, sp)| format!(" lr={lr:#x} sp={sp:#x}"))
        .unwrap_or_default();
    fatal_line(&format!(
        "{kind} ec={ec:#x} esr={esr:#x} elr={elr:#x} far={far:#x}{sp}{lr}{ctx}",
        ctx = task_ctx(),
    ));
}
