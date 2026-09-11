//! Log CPU exceptions to serial before halting (used by arch interrupt handlers).

extern crate alloc;

use alloc::format;
use alloc::string::String;

use crate::arch;
use crate::console;
use crate::task;
#[cfg(target_arch = "riscv64")]
use crate::user;

pub fn fatal_line(line: &str) -> ! {
    console::status_fail(&format!("exception: {line}"));
    console::flush();
    arch::exit_qemu(arch::QEMU_FAILURE);
    arch::halt();
}

fn task_ctx() -> String {
    let id = task::current_id();
    match task::current_user_pc_sp() {
        Some((rip, rsp)) => {
            // CI diagnostic: user-stack overflow probe. stack bottom = user_base
            // + stack_off (stack grows down toward the image); report headroom.
            let mut s = format!(" task={id} user rip={rip:#x} rsp={rsp:#x}");
            let min_sp = task::current_min_user_sp();
            if min_sp != 0 {
                let (base, _span, stack_off) = task::current_user_map();
                let bottom = base + stack_off;
                s.push_str(&alloc::format!(
                    " min_sp={min_sp:#x} stack_bottom={bottom:#x} headroom={:#x}",
                    (min_sp as u64).saturating_sub(bottom)
                ));
            }
            s
        }
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
pub fn riscv64_page_fault(kind: &str, stval: u64, sepc: u64, user_sp: u64, frame: *mut u64) -> ! {
    // Syscall ring FIRST: dump_kernel_frame's fatal_line on SYSCALL_FRAME null
    // never returns, so the history must be printed before any fatal path.
    task::dump_syscall_history();
    // Canary-probe dump (strip before PR): if a pre-return sret left the pool
    // canary in sscratch, show where it was left. Print on EVERY fault branch.
    console::status_fail(&alloc::format!(
        "canary-sscratch: hits={} last_sepc={:#x} last_scause={:#x}\n",
        crate::arch::sscratch_canary().0,
        crate::arch::sscratch_canary().1,
        crate::arch::sscratch_canary().2
    ));
    // sepc=0 with a valid saved user pc means the trap frame itself was
    // zeroed — dump the kernel-stack bytes around the frame so the
    // corrupting write's footprint is identifiable in the CI serial.
    if sepc == 0 {
        if let Some((rip, rsp)) = task::current_user_pc_sp() {
            dump_kernel_frame("page-fault sepc=0", rip as u64);
            // Overflow probe: same headroom info as task_ctx, but emitted here
            // too because the SYSCALL_FRAME-null dump path never reaches it.
            let min_sp = task::current_min_user_sp();
            let (base, _span, stack_off) = task::current_user_map();
            let bottom = base + stack_off;
            console::status_fail(&alloc::format!(
                "overflow-probe: min_sp={min_sp:#x} stack_bottom={bottom:#x} headroom={:#x} rsp={rsp:#x}\n",
                (min_sp as u64).saturating_sub(bottom)
            ));
            task::diagnose_stack_pool();
            // CI diagnostic: dump the live trap frame (all saved user regs at
            // fault). a7 slot = last syscall nr, ra = jump target's source.
            if !frame.is_null() {
                let mut fline = String::from("trap-frame:\n");
                let f = unsafe { core::slice::from_raw_parts(frame, 35) };
                const R: [&str; 32] = ["x0","ra","sp","gp","tp","t0","t1","t2","s0","s1","a0","a1","a2","a3","a4","a5","a6","a7","s2","s3","s4","s5","s6","s7","s8","s9","s10","s11","t3","t4","t5","t6"];
                for (i, v) in f.iter().enumerate() {
                    let name = if i < 32 { R[i] } else { match i { 32 => "sepc", 33 => "sstatus", 34 => "sscratch", _ => "?" } };
                    fline.push_str(&alloc::format!("  {name}={v:#x}{}\n", if *v == 0 { "  <-- ZERO" } else { "" }));
                }
                console::status_fail(&fline);
            }
            // CI diagnostic: dump the user stack around rsp. Zeroed return
            // slots (ra=0) prove user-STACK corruption vs a corrupted
            // function pointer in data/heap.
            let aspace = task::current_aspace();
            let mut bytes = [0u8; 128];
            if user::copy_from_user(aspace, rsp, &mut bytes) {
                let mut line = String::from("user-stack@rsp:\n");
                for (i, w) in bytes.chunks_exact(8).enumerate() {
                    let v = u64::from_le_bytes(w.try_into().unwrap());
                    line.push_str(&alloc::format!("  [rsp+{:>#5x}]={v:#x}{}\n", i * 8, if v == 0 { "  <-- ZERO" } else { "" }));
                }
                console::status_fail(&line);
            } else {
                console::status_fail("user-stack@rsp: unreadable\n");
            }
        }
    }
            // Failure-path-only probe: dump the raw instruction words around
            // sepc read straight from user memory. Fires only on already-
            // failing runs, so it cannot perturb passing runs. Verifies the
            // reported sepc against the actual bytes (prelink/link layout
            // made objdump-based attribution unreliable).
            if !frame.is_null() {
                let cur_aspace = task::current_aspace();
                let mut ib = [0u8; 32];
                if user::copy_from_user(cur_aspace, (sepc.saturating_sub(12)) as usize, &mut ib) {
                    let mut l = alloc::format!("code@sepc={sepc:#x} (words at sepc-12..sepc+4):\n");
                    for (k, w) in ib.chunks_exact(4).enumerate() {
                        let v = u32::from_le_bytes(w.try_into().unwrap());
                        let off = (k as isize * 4 - 12) as i64;
                        l.push_str(&alloc::format!("  [sepc{off:+#x}]={v:#010x}\n"));
                    }
                    console::status_fail(&l);
                } else {
                    console::status_fail("code@sepc: unreadable\n");
                }
            }
    // Failure-path-only: brk-boundary analysis (wild ptr / heap exhaustion).
    if !frame.is_null() && stval >= (task::current_brk() as u64).saturating_sub(1) {
        user::report_brk_boundary_fault(stval as usize, task::current_aspace());
    }

    // kernel refused a sys_brk growth (heap window exhausted). Surface the
    // heap/brk state and refusal counter on the fatal line.
    if stval < 0x1000 {
        user::report_heap_diag();
    }
    // CI diagnostic: classify user faults inside the brk heap window — a fault
    // below brk_cur means the kernel lost a PTE for a mapped heap page; above
    // brk_cur means a userspace wild pointer / heap exhaustion.
    // CI diagnostic: dump the live trap frame for EVERY user page fault.
    if !frame.is_null() {
        let mut fline = String::from("trap-frame:\n");
        let f = unsafe { core::slice::from_raw_parts(frame, 35) };
        const R: [&str; 32] = ["x0","ra","sp","gp","tp","t0","t1","t2","s0","s1","a0","a1","a2","a3","a4","a5","a6","a7","s2","s3","s4","s5","s6","s7","s8","s9","s10","s11","t3","t4","t5","t6"];
        for (i, v) in f.iter().enumerate() {
            let name = if i < 32 { R[i] } else { match i { 32 => "sepc", 33 => "sstatus", 34 => "sscratch", _ => "?" } };
            fline.push_str(&alloc::format!("  {name}={v:#x}{}\n", if *v == 0 { "  <-- ZERO" } else { "" }));
        }
        console::status_fail(&fline);
    }
    let (heap_base, brk_cur) = task::current_brk_info();
    if stval >= heap_base && stval < heap_base + (user::HEAP_WINDOW_PAGES as u64) * user::PAGE as u64 {
        let verdict = if stval < brk_cur { "PTE-LOST (brk covers it!)" } else { "beyond brk (wild ptr/exhaustion)" };
        console::status_fail(&alloc::format!(
            "heap-fault-classify: stval={stval:#x} heap_base={heap_base:#x} brk_cur={brk_cur:#x} -> {verdict}\n"
        ));
    }
    fatal_line(&format!(
        "{kind} stval={stval:#x} sepc={sepc:#x} sp={user_sp:#x}{ctx}",
        ctx = task_ctx(),
    ));
}

/// Hex-dump bytes around the live trap frame (riscv64 syscall frame: sepc
/// at word 32, user sp at word 34) plus a marker so corruption patterns
/// (zeros vs sentinel vs garbage) are visible in CI serial output.
#[cfg(target_arch = "riscv64")]
fn dump_kernel_frame(why: &str, saved_rip: u64) {
    let frame = user::syscall_frame_ptr() as *mut u64;
    if frame.is_null() {
        // Non-fatal: report and return so the caller's overflow-probe and the
        // final fatal line (with task ctx) still print.
        console::status_fail(&format!("frame-dump: {why} SYSCALL_FRAME null (saved rip={saved_rip:#x})"));
        console::flush();
        return;
    }
    let mut out = alloc::format!("frame-dump: {why} frame={:p} saved_rip={saved_rip:#x}\n", frame);
    unsafe {
        for w in -8isize..16 {
            let v = core::ptr::read_unaligned(frame.offset(w));
            out.push_str(&alloc::format!("  [+{}w]={:#x}\n", w, v));
        }
    }
    console::status_fail(&out);
    console::flush();
}

#[cfg(target_arch = "riscv64")]
pub fn riscv64_trap(code: u64, sepc: u64, stval: u64) -> ! {
    fatal_line(&format!(
        "trap scause={code:#x} sepc={sepc:#x} stval={stval:#x}{ctx}",
        ctx = task_ctx(),
    ));
}
