//! Scheduler: yield/schedule, the blocking `wait` loop, IRQ-state helpers
//! and the per-AP idle task.

use super::*;

pub fn enable_preempt() {
    PREEMPT_ON.store(true, Ordering::SeqCst);
}

pub fn yield_now() {
    // Must restore the caller's IF. Unconditional sti broke syscalls
    // (syscall_entry runs with cli): wait_child then locked TASKS with
    // IF=1 and the timer nested into schedule → same-CPU spin deadlock.
    let flags = irq_save();
    irq_off();
    schedule();
    irq_restore(flags);
}

/// Same switch as `yield_now`. No-op until `enable_preempt` so the first-tick
/// IRQ proof does not leave the Limine stack.
pub fn schedule() {
    if !PREEMPT_ON.load(Ordering::SeqCst) {
        return;
    }
    // Hold IF off across TASKS + switch. Caller may already have IF clear
    // (timer, yield); save/restore so we never leave IF on while locked.
    let flags = irq_save();
    irq_off();
    // Soft-ACK pending TLB shootdowns while IF is off so a peer in die()
    // reclaim cannot spin forever waiting for an IPI we cannot take yet.
    crate::smp::tlb_service();

    // Pick `next` under TASKS, but do NOT mark the previous task Ready and do
    // NOT switch CR3 while holding the lock:
    // - Ready-before-aspace let a peer reclaim while this CPU still had CR3
    //   (NX #PF / mmap tear) — keep old as Running until after the switch.
    // - aspace/rsp0 under TASKS held the global scheduler lock across CR3 and
    //   IPI-heavy unload drains; with RR home CPUs that livelocked CI bios at
    //   the histrecall/arrow stage (peer IF-off spinning on TASKS).
    let switch = {
        let mut tasks = TASKS.lock();
        let current = current_slot();

        crate::smp::note_schedule();
        let cpu = crate::smp::cpu_id();
        let mut next = current;
        for off in 1..MAX_TASKS {
            let i = (current + off) % MAX_TASKS;
            if tasks[i].state != State::Ready {
                continue;
            }
            if let Some(aff) = tasks[i].affinity {
                if aff != cpu {
                    continue;
                }
            }
            next = i;
            break;
        }

        if next == current {
            if tasks[current].state == State::Ready {
                tasks[current].state = State::Running;
            }
            None
        } else {
            let old_sp = core::ptr::addr_of_mut!(tasks[current].sp);
            let new_sp = tasks[next].sp;
            let kstack = tasks[next].kernel_stack_top;
            let aspace = tasks[next].aspace;
            // Leave `current` Running so peers cannot pick/reclaim it until we
            // have switched CR3 below. Publish next + CURRENT now.
            tasks[next].state = State::Running;
            set_current_slot(next);
            Some((old_sp, new_sp, kstack, aspace, current, next))
        }
    };

    let Some((old_sp, new_sp, kstack, aspace, old, _next)) = switch else {
        irq_restore(flags);
        return;
    };

    if kstack != 0 {
        let cpu = crate::smp::cpu_id();
        stamp_stack_cpu(kstack, cpu);
        user::set_kernel_rsp0(kstack);
        #[cfg(target_arch = "x86_64")]
        crate::arch::gdt::set_rsp0(kstack as u64);
        // riscv64: no sscratch write here — it stays 0 in S-mode and is armed
        // with the kernel stack top only on the way out to U-mode.
    }
    #[cfg(feature = "linux-compat")]
    crate::linux::on_switch(_next);

    let want = if aspace == 0 {
        KERNEL_ASPACE.load(Ordering::SeqCst)
    } else {
        aspace
    };
    if want != loaded_aspace() {
        user::switch_aspace(want);
        set_loaded_aspace(want);
    }

    // CR3 is `next`'s — safe to publish Ready on `old`.
    // Do NOT IPI-kick here: Ready is visible while we still run on `old`'s
    // stack until task_switch; a peer running `old` early NX-faulted under
    // -smp 4. AP timers pick up foreign-affinity Ready; fork kicks when a
    // parallel child is RR-homed onto another AP.
    {
        let mut tasks = TASKS.lock();
        if tasks[old].state == State::Running {
            tasks[old].state = State::Ready;
        }
    }

    unsafe {
        task_switch(old_sp, new_sp);
    }
    irq_restore(flags);
}

pub(super) fn irq_save() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let r: u64;
        core::arch::asm!("pushfq; pop {r}", r = out(reg) r);
        r
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let r: u64;
        core::arch::asm!(
            "mrs {r}, daif",
            r = out(reg) r,
            options(nomem, nostack, preserves_flags)
        );
        r
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        let r: u64;
        core::arch::asm!(
            "csrr {r}, sstatus",
            r = out(reg) r,
            options(nomem, nostack, preserves_flags)
        );
        r
    }
}

pub(super) fn irq_restore(flags: u64) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        if flags & (1 << 9) != 0 {
            core::arch::asm!("sti", options(nostack, preserves_flags));
        } else {
            core::arch::asm!("cli", options(nostack, preserves_flags));
        }
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr daif, {r}", r = in(reg) flags, options(nostack));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        if flags & (1 << 1) != 0 {
            core::arch::asm!("csrs sstatus, {}", in(reg) 1 << 1, options(nostack));
        } else {
            core::arch::asm!("csrc sstatus, {}", in(reg) 1 << 1, options(nostack));
        }
    }
}

pub(super) fn irq_off() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("cli", options(nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr daifset, #3", options(nostack));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("csrc sstatus, {}", in(reg) 1 << 1, options(nostack));
    }
}

pub(super) fn irq_on() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("sti", options(nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr daifclr, #3", options(nostack));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("csrs sstatus, {}", in(reg) 1 << 1, options(nostack));
    }
}

pub(super) fn wait() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("hlt", options(nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("wfi", options(nostack, preserves_flags));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("wfi", options(nostack, preserves_flags));
    }
}

/// Per-AP idle stack base stashed before the Limine→idle migrate so bring-up
/// can finish on the big stack (see [`ap_idle_loop`]).
static AP_IDLE_STACK_BASE: [AtomicUsize; crate::smp::MAX_CPUS] =
    [const { AtomicUsize::new(0) }; crate::smp::MAX_CPUS];

/// Idle loop for a secondary CPU brought up by [`crate::smp`].
///
/// Allocates a pinned idle task so `schedule` can leave and return to this CPU.
///
/// Critical: Limine's AP stack is tiny (UEFI path especially). Do **not**
/// construct a multi-KiB [`Task`] or enable IRQs while still on it — both
/// overflowed into NX execute (`code=0x11`, `cr2==rip`) under `-smp 4`
/// (PR #164 boot-mini uefi). Migrate first with IF clear / !ONLINE, then
/// finish bring-up on the 64KiB idle stack.
pub fn ap_idle_loop(logical: usize) -> ! {
    // Enter with IRQs masked (ap_entry cli / DAIF). Keep them off until
    // bring-up completes on the idle stack.
    irq_off();
    let layout = Layout::from_size_align(STACK_SIZE, 16).expect("ap idle stack");
    let stack = unsafe { alloc(layout) };
    assert!(!stack.is_null(), "ap idle stack alloc");
    let base = stack as usize;
    let top = base + STACK_SIZE;
    stamp_stack_cpu(top, logical);
    if logical < crate::smp::MAX_CPUS {
        AP_IDLE_STACK_BASE[logical].store(base, Ordering::SeqCst);
    }
    // Minimal work on Limine's stack: seed + switch. Task install happens in
    // [`ap_idle_bringup`] once RSP is on the 64KiB allocation.
    let sp = unsafe { seed_stack(stack, STACK_SIZE, ap_idle_trampoline as *const () as usize) };
    let mut discard_sp: usize = 0;
    unsafe {
        task_switch(core::ptr::addr_of_mut!(discard_sp), sp);
    }
    unreachable!()
}

/// Finish AP idle bring-up on the 64KiB idle stack (see [`ap_idle_loop`]).
fn ap_idle_bringup() {
    let logical = crate::smp::cpu_id();
    let base = if logical < crate::smp::MAX_CPUS {
        AP_IDLE_STACK_BASE[logical].load(Ordering::SeqCst)
    } else {
        0
    };
    assert!(base != 0, "ap idle stack base");
    let top = base + STACK_SIZE;
    // Current RSP is already on this stack (we got here via task_switch ret).
    // Record that SP so the first schedule away/back saves/restores correctly.
    let sp_now: usize;
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) sp_now, options(nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("mov {}, sp", out(reg) sp_now, options(nostack, preserves_flags));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("mv {}, sp", out(reg) sp_now, options(nostack, preserves_flags));
    }

    // Install idle Task **in place** (no stack temporary — same discipline as
    // fork_current; a full `Task { .. }` literal is multi-KiB).
    let mut tasks = TASKS.lock();
    let slot = tasks
        .iter()
        .position(|t| t.state == State::Unused)
        .expect("no AP idle slot");
    {
        let t = &mut tasks[slot];
        unsafe {
            core::ptr::write(t, EMPTY);
        }
        t.state = State::Running;
        t.stack_base = base;
        t.sp = sp_now;
        t.entry = Some(ap_idle_body);
        t.kernel_stack_top = top;
        t.sid = slot;
        t.pgid = slot;
        t.affinity = Some(logical);
    }
    drop(tasks);
    set_current_slot(slot);
    // APs may still hold Limine's early TTBR0; install the BSP kernel/device
    // root and record it so the first schedule does not race TLB shootdowns.
    let k = KERNEL_ASPACE.load(Ordering::SeqCst);
    if k != 0 {
        user::switch_aspace(k);
    }
    set_loaded_aspace(k);
    crate::smp::mark_running(logical);
    enable_preempt();
    // IRQs only after CURRENT/idle exist *and* we left the Limine stack.
    irq_on();
    ap_idle_body();
}

fn ap_idle_body() {
    loop {
        crate::smp::tlb_service();
        yield_now();
        crate::arch::wait_interrupt();
    }
}

fn ap_idle_trampoline() {
    ap_idle_bringup();
}
