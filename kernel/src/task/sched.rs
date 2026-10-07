//! Scheduler: yield/schedule, the blocking `wait` loop, IRQ-state helpers
//! and the per-AP idle task.

use super::*;

/// Per CPU: its idle task's slot (`become_idle`), `usize::MAX` before.
static IDLE_SLOT: [AtomicUsize; crate::smp::MAX_CPUS] =
    [const { AtomicUsize::new(usize::MAX) }; crate::smp::MAX_CPUS];

/// The calling task is this CPU's idle task from now on: `schedule` runs it
/// only when nothing else can run here.
pub fn become_idle() {
    let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
    IDLE_SLOT[cpu].store(current_slot(), Ordering::SeqCst);
}

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
        let idle = IDLE_SLOT[cpu.min(crate::smp::MAX_CPUS - 1)].load(Ordering::Relaxed);
        let mut next = current;
        let mut idle_ready = false;
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
            // A woken task that is still some CPU's `CURRENT` (halting in
            // `block_until`) runs there when it resumes; never pick it here.
            if slot_on_cpu(i) {
                continue;
            }
            if i == idle {
                idle_ready = true;
                continue;
            }
            next = i;
            break;
        }
        // The idle task only when nothing else can run here: a task that
        // keeps running keeps its address space, where a round through the
        // idle task loaded the kernel's and back (a whole TLB flush each
        // way, every tick).
        let current_runs = matches!(tasks[current].state, State::Running | State::Ready);
        if next == current && idle_ready && !current_runs {
            next = idle;
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
            let old_kstack = tasks[current].kernel_stack_top;
            let aspace = tasks[next].aspace;
            let old_user = tasks[current].aspace != 0;
            // Leave `current` Running so peers cannot pick/reclaim it until it
            // is off this CPU (`finish_switch`). Publish next + CURRENT now.
            // SWITCHED_FROM is published under TASKS too: `wake` consults it
            // so a Blocked task that is still leaving this CPU is not made
            // Ready (and picked elsewhere) before `task_switch` saved its sp.
            tasks[next].state = State::Running;
            // A task woken while it was still this CPU's CURRENT (halting in
            // `block_until`, or preempted there by the timer) is Ready
            // already. Left Ready, a peer could pick it before task_switch
            // saved its sp and resume it from the frame it left the last
            // time, long since overwritten by its own stack (CI: the USB
            // thread's "switch frame: its frame changed"). Running until
            // `finish_switch`, like any task leaving a CPU.
            if tasks[current].state == State::Ready {
                tasks[current].state = State::Running;
            }
            // The frame of the syscall a task is in follows the task, not the
            // CPU: aarch64 exec resumes through it, and a syscall that blocked
            // here found another task's frame in the CPU's cell when it ran
            // again (`arch::syscall_frame`).
            tasks[current].syscall_frame = crate::arch::syscall_frame() as usize;
            crate::arch::set_syscall_frame(tasks[next].syscall_frame as *mut u64);
            set_current_slot(next);
            SWITCHED_FROM[cpu.min(crate::smp::MAX_CPUS - 1)].store(current, Ordering::SeqCst);
            Some((old_sp, new_sp, kstack, old_kstack, aspace, current, next, old_user))
        }
    };

    let Some((old_sp, new_sp, kstack, old_kstack, aspace, old, next, old_user)) = switch else {
        irq_restore(flags);
        return;
    };

    // The task leaving this CPU must still be on its kernel stack: an
    // overflow is reported here, by the task that did it, not later by the
    // owner of whatever the heap placed below (`arm_stack`).
    if !super::stack_intact(old_kstack) {
        panic!("kernel stack overflow: task {old} ran below its kernel stack (top {old_kstack:#x})");
    }

    if kstack != 0 {
        let cpu = crate::smp::cpu_id();
        let _ = cpu;
        crate::arch::set_kernel_stack_top(kstack);
    }
    // User FP/SIMD registers and the thread pointer follow the task (the
    // kernel never uses them).
    if old_user {
        fpu::switch_out(old);
        tp::switch_out(old);
    }
    if aspace != 0 {
        fpu::switch_in(next);
        tp::switch_in(next);
    }

    let want = if aspace == 0 {
        KERNEL_ASPACE.load(Ordering::SeqCst)
    } else {
        aspace
    };
    if want != loaded_aspace() {
        user::switch_aspace(want);
        set_loaded_aspace(want);
    }

    // `old` becomes Ready only once task_switch has saved its stack pointer
    // and left its stack: the task that resumes on this CPU publishes it
    // (`finish_switch`). Publishing it here let a peer CPU pick `old` and
    // switch to its stale saved `sp` while this CPU still ran on that stack
    // (x86 boot-mini: kernel #PF at rip=0x7 right after the SMP smoke, whose
    // unpinned kernel threads yield across CPUs). Do NOT IPI-kick: AP timers
    // pick up foreign-affinity Ready; fork kicks when a parallel child is
    // RR-homed onto another AP.
    check_switch_frame(next, new_sp);
    unsafe {
        task_switch(old_sp, new_sp);
    }
    finish_switch();
    irq_restore(flags);
}

/// Per task: where its switch frame was saved when it last left a CPU, a
/// checksum of the frame, and that CPU (`check_switch_frame`). Zero sp:
/// not recorded (a new stack).
static LEFT_SP: [AtomicUsize; MAX_TASKS] = [const { AtomicUsize::new(0) }; MAX_TASKS];
static LEFT_SUM: [AtomicU64; MAX_TASKS] = [const { AtomicU64::new(0) }; MAX_TASKS];
static LEFT_CPU: [AtomicUsize; MAX_TASKS] = [const { AtomicUsize::new(0) }; MAX_TASKS];

fn frame_sum(sp: usize) -> u64 {
    let mut sum = 0xcbf2_9ce4_8422_2325u64;
    for off in (0..crate::arch::switch::FRAME_BYTES).step_by(8) {
        let w = unsafe { core::ptr::read_volatile((sp + off) as *const u64) };
        sum = (sum ^ w).wrapping_mul(0x100_0000_01b3);
    }
    sum
}

/// `slot` left `cpu` with its frame saved at `sp` (`finish_switch`).
fn remember_frame(slot: usize, sp: usize, cpu: usize) {
    if sp == 0 {
        return;
    }
    LEFT_SUM[slot].store(frame_sum(sp), Ordering::Relaxed);
    LEFT_CPU[slot].store(cpu, Ordering::Relaxed);
    LEFT_SP[slot].store(sp, Ordering::Relaxed);
}

/// `slot` starts on a new stack: nothing to compare its first frame with.
pub(super) fn forget_frame(slot: usize) {
    LEFT_SP[slot].store(0, Ordering::Relaxed);
}

/// The frame `task_switch` resumes `next` from must return into the
/// kernel. One overwritten while its task was off its CPU (zeroed, during
/// USB enumeration in CI: a woken task picked by a peer while still leaving
/// its CPU, resumed from its previous frame) jumps to 0; report it here,
/// with the stack around it, instead of as a fault at address 0 afterwards.
fn check_switch_frame(next: usize, sp: usize) {
    let ra = unsafe { crate::arch::switch::frame_return(sp) };
    let anchor = check_switch_frame as *const () as usize;
    let (left_sp, left_sum, left_cpu) = (
        LEFT_SP[next].load(Ordering::Relaxed),
        LEFT_SUM[next].load(Ordering::Relaxed),
        LEFT_CPU[next].load(Ordering::Relaxed),
    );
    // What changed since the task left its CPU: the saved stack pointer
    // (another context stored its own there) or the frame it points at
    // (memory written over).
    let changed = if left_sp == 0 {
        None
    } else if left_sp != sp {
        Some(alloc::format!("its saved sp changed from {left_sp:#x} since it left cpu {left_cpu}"))
    } else if frame_sum(sp) != left_sum {
        Some(alloc::format!("its frame changed since it left cpu {left_cpu}"))
    } else {
        None
    };
    if changed.is_none() && ra.abs_diff(anchor) < 64 << 20 {
        return;
    }
    if let Some(what) = &changed {
        crate::console::status_fail(&alloc::format!("switch frame: task {next}: {what}"));
    }
    let (base, top, state, affinity) = {
        let tasks = TASKS.lock();
        let t = &tasks[next];
        (t.stack_base, t.kernel_stack_top, t.state as u8, t.affinity)
    };
    let hhdm = crate::limine_boot::hhdm_offset() as usize;
    crate::console::status_fail(&alloc::format!(
        "switch frame: task {next} (state {state} affinity {affinity:?}) resumes at {ra:#x}: sp {sp:#x} \
         (phys {:#x}) stack {base:#x}..{top:#x}, cpu {}",
        sp.wrapping_sub(hhdm),
        crate::smp::cpu_id(),
    ));
    // The words around the frame, runs of zeros collapsed: how much was
    // overwritten, and with what alignment, says what wrote it.
    let from = (sp.saturating_sub(512) & !7).max(base);
    let to = (sp + 512).min(top);
    let mut addr = from;
    while addr < to {
        let w = unsafe { core::ptr::read_volatile(addr as *const usize) };
        if w == 0 {
            let start = addr;
            while addr < to && unsafe { core::ptr::read_volatile(addr as *const usize) } == 0 {
                addr += 8;
            }
            crate::console::write_str(&alloc::format!("  {start:#x}..{addr:#x} zero ({} bytes)\n", addr - start));
            continue;
        }
        crate::console::write_str(&alloc::format!("  {addr:#x}: {w:#x}\n"));
        addr += 8;
    }
    panic!("switch frame of task {next} overwritten");
}

/// Per CPU: the task this CPU just switched away from, still marked Running
/// until [`finish_switch`] runs on the other side of the switch.
static SWITCHED_FROM: [AtomicUsize; crate::smp::MAX_CPUS] =
    [const { AtomicUsize::new(usize::MAX) }; crate::smp::MAX_CPUS];

/// First thing after a task switch, on the incoming task's stack (irqs off):
/// the previous task is off this CPU's stack now, so peers may run it.
pub(super) fn finish_switch() {
    let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
    // Clear SWITCHED_FROM under TASKS, where `wake` reads it: cleared before
    // the lock, a wake in between made a Blocked `prev` Ready at once, a peer
    // resumed it before its frame was recorded, and this late update then
    // turned it Ready while it ran there.
    let mut tasks = TASKS.lock();
    let prev = SWITCHED_FROM[cpu].swap(usize::MAX, Ordering::SeqCst);
    if prev == usize::MAX {
        return;
    }
    remember_frame(prev, tasks[prev].sp, cpu);
    if tasks[prev].state == State::Running {
        tasks[prev].state = State::Ready;
    } else if tasks[prev].state == State::Blocked && tasks[prev].wake_pending {
        // Woken while it was still on the old CPU's stack: runnable now.
        tasks[prev].wake_pending = false;
        tasks[prev].state = State::Ready;
    }
}

/// True while `slot` is still being switched away from on some CPU: its
/// saved stack pointer is not valid until `finish_switch` runs there.
fn mid_switch(slot: usize) -> bool {
    SWITCHED_FROM.iter().any(|s| s.load(Ordering::SeqCst) == slot)
}

/// True while `slot` is some CPU's current task (also used by lifecycle).
pub(super) fn slot_on_cpu(slot: usize) -> bool {
    CURRENT.iter().any(|c| c.load(Ordering::SeqCst) == slot)
}

// --- Blocking waits ----------------------------------------------------------
//
// A waiter reads `wait_seq()`, checks its condition, and if it must wait calls
// `block_until(key, seq, deadline)`. Producers change state and then call
// `wake(key)`. `wake` bumps the sequence under TASKS and `block_until` refuses
// to block when the sequence moved, so a wake between the check and the block
// is never lost. `wake_any` skips the scan when no WAIT_ANY waiter is
// registered, but bumps the sequence first; `block_until` registers before it
// checks the sequence, so one of the two always sees the other. Waits are
// always re-checked by the caller (spurious wakes are fine), which keeps every
// existing `loop { check; wait }` shape intact.

/// `wait_key` that every `wake` call matches (pollers, idle sleeps).
pub const WAIT_ANY: usize = usize::MAX;
/// Key spaces so unrelated objects never share a key.
pub const fn key_pipe(id: usize) -> usize {
    0x1_0000 + id
}
pub const fn key_pty(id: usize) -> usize {
    0x2_0000 + id
}
pub const fn key_child(parent: usize) -> usize {
    0x3_0000 + parent
}
pub const KEY_CONSOLE: usize = 0x4_0000;
/// Waits that only a signal (`wake_task`) ends; no `wake(KEY_SIGNAL)` exists.
pub const KEY_SIGNAL: usize = 0x5_0000;
/// A process leader waiting for its other threads to end.
pub const fn key_threads(pid: usize) -> usize {
    0x6_0000 + pid
}
/// Waiters for a file lock on node `node` (`fs::lock`).
pub const fn key_lock(node: usize) -> usize {
    0x10_0000 + node
}
/// Threads of process `pid` waiting on user address `addr` (`wait_addr`):
/// bit 63 set, the pid above the 48-bit user address.
pub const fn key_addr(pid: usize, addr: usize) -> usize {
    1 << 63 | pid << 48 | (addr & 0xffff_ffff_ffff)
}

/// Bumped on every wake (under TASKS, except `wake_any`'s fast path).
static WAIT_SEQ: AtomicU64 = AtomicU64::new(0);
/// Earliest `wake_at` of any Blocked task (u64::MAX = none).
static NEXT_DEADLINE: AtomicU64 = AtomicU64::new(u64::MAX);
/// Number of tasks blocked with `WAIT_ANY` (lets producers skip the scan).
static ANY_WAITERS: AtomicUsize = AtomicUsize::new(0);
/// Per CPU: a task for this CPU became runnable while it may be about to
/// halt; its idle loop re-checks instead of sleeping.
static NEED_RESCHED: [AtomicBool; crate::smp::MAX_CPUS] =
    [const { AtomicBool::new(false) }; crate::smp::MAX_CPUS];
/// Per CPU: halted (or about to halt) in an idle loop / `block_until`.
static CPU_IDLE: [AtomicBool; crate::smp::MAX_CPUS] =
    [const { AtomicBool::new(false) }; crate::smp::MAX_CPUS];
/// Per CPU: how often it halted waiting for an interrupt (`/proc/cpuinfo`).
static IDLE_HALTS: [AtomicU64; crate::smp::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::smp::MAX_CPUS];

pub fn idle_halts(cpu: usize) -> u64 {
    if cpu < crate::smp::MAX_CPUS {
        IDLE_HALTS[cpu].load(Ordering::Relaxed)
    } else {
        0
    }
}

/// Number of tasks currently Blocked (diagnostics).
pub fn blocked_count() -> usize {
    let flags = irq_save();
    irq_off();
    let n = TASKS.lock().iter().filter(|t| t.state == State::Blocked).count();
    irq_restore(flags);
    n
}

fn halt(cpu: usize) {
    IDLE_HALTS[cpu].fetch_add(1, Ordering::Relaxed);
    crate::arch::idle_wait();
}

pub fn wait_seq() -> u64 {
    WAIT_SEQ.load(Ordering::SeqCst)
}

/// Monotonic-ns deadline `ms` from now (for `block_until`).
pub fn deadline_ms(ms: u64) -> u64 {
    crate::time::monotonic_ns().saturating_add(ms.saturating_mul(1_000_000)).max(1)
}

/// Block the current task until `wake(key)` (or any wake for `WAIT_ANY`), a
/// signal (`wake_task`), or the monotonic deadline (`0` = none). Returns at
/// once if a wake happened since `seq` was read. Callers re-check their
/// condition afterwards.
pub fn block_until(key: usize, seq: u64, deadline: u64) {
    if !PREEMPT_ON.load(Ordering::SeqCst) {
        return;
    }
    let flags = irq_save();
    irq_off();
    let me = current_slot();
    {
        let mut tasks = TASKS.lock();
        // Count ourselves as a WAIT_ANY waiter before the WAIT_SEQ check:
        // `wake_any` bumps WAIT_SEQ before it reads ANY_WAITERS, so either
        // it sees us (and wakes us once we are Blocked: it takes TASKS) or
        // we see its bump and return.
        if key == WAIT_ANY {
            ANY_WAITERS.fetch_add(1, Ordering::SeqCst);
        }
        let early = WAIT_SEQ.load(Ordering::SeqCst) != seq
            || tasks[me].state != State::Running
            || (deadline != 0 && crate::time::monotonic_ns() >= deadline);
        if early {
            if key == WAIT_ANY {
                ANY_WAITERS.fetch_sub(1, Ordering::SeqCst);
            }
            drop(tasks);
            irq_restore(flags);
            return;
        }
        let t = &mut tasks[me];
        t.state = State::Blocked;
        t.wait_key = key;
        t.wake_at = deadline;
        t.wake_pending = false;
        if deadline != 0 {
            NEXT_DEADLINE.fetch_min(deadline, Ordering::SeqCst);
            // This CPU's timer fires at the deadline, not at the next tick.
            crate::arch::timer_deadline(deadline);
        }
    }
    loop {
        // Run something else on this CPU if there is anything; comes back
        // here once we are Ready again (or at once when nothing is runnable).
        schedule();
        let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
        CPU_IDLE[cpu].store(true, Ordering::SeqCst);
        let still_blocked = {
            let mut tasks = TASKS.lock();
            match tasks[me].state {
                State::Blocked => true,
                State::Ready => {
                    tasks[me].state = State::Running;
                    false
                }
                _ => false,
            }
        };
        if !still_blocked {
            CPU_IDLE[cpu].store(false, Ordering::SeqCst);
            break;
        }
        // Nothing else runnable here: halt on our own stack until an
        // interrupt (timer / IPI from a waker) arrives. `idle_wait` enters
        // with IRQs off, so a wake that already raised an IPI is pending and
        // ends the halt immediately.
        if !NEED_RESCHED[cpu].swap(false, Ordering::SeqCst) {
            halt(cpu);
        } else {
            irq_on();
        }
        irq_off();
        CPU_IDLE[cpu].store(false, Ordering::SeqCst);
    }
    irq_restore(flags);
}

/// Make task `slot` runnable if it is Blocked. Returns the CPU to kick, if
/// any. Caller holds TASKS.
fn wake_locked(tasks: &mut TaskTable, slot: usize, kicks: &mut u64) {
    let t = &mut tasks[slot];
    if t.state != State::Blocked {
        return;
    }
    if t.wait_key == WAIT_ANY {
        ANY_WAITERS.fetch_sub(1, Ordering::SeqCst);
    }
    // Keep a stale key from matching a later wake once the task runs again.
    t.wait_key = 0;
    t.wake_at = 0;
    if mid_switch(slot) {
        t.wake_pending = true;
    } else {
        t.state = State::Ready;
    }
    // Where will it run? Its home CPU, the CPU it is halting on, or any
    // idle CPU for a floating task.
    let mut target = None;
    for (c, cur) in CURRENT.iter().enumerate() {
        if cur.load(Ordering::SeqCst) == slot {
            target = Some(c);
        }
    }
    if target.is_none() {
        target = t.affinity;
    }
    if target.is_none() {
        target = (0..crate::smp::MAX_CPUS).find(|&c| CPU_IDLE[c].load(Ordering::SeqCst));
    }
    if let Some(c) = target {
        let c = c.min(crate::smp::MAX_CPUS - 1);
        NEED_RESCHED[c].store(true, Ordering::SeqCst);
        *kicks |= 1 << c;
    }
}

/// IPI the CPUs in `kicks` that are halted (never ourselves: the caller
/// returns to its own scheduler soon enough).
fn kick(kicks: u64) {
    if kicks == 0 || crate::smp::online_count() <= 1 {
        return;
    }
    let me = crate::smp::cpu_id();
    for c in 0..crate::smp::MAX_CPUS {
        if kicks & (1 << c) != 0 && c != me && CPU_IDLE[c].load(Ordering::SeqCst) {
            crate::smp::kick_cpu(c);
        }
    }
}

/// Wake every task blocked on `key` (and every `WAIT_ANY` waiter).
pub fn wake(key: usize) {
    let flags = irq_save();
    irq_off();
    let mut kicks = 0u64;
    {
        let mut tasks = TASKS.lock();
        WAIT_SEQ.fetch_add(1, Ordering::SeqCst);
        let any = ANY_WAITERS.load(Ordering::SeqCst) != 0;
        for i in 0..MAX_TASKS {
            if tasks[i].state == State::Blocked
                && (tasks[i].wait_key == key || (any && tasks[i].wait_key == WAIT_ANY))
            {
                wake_locked(&mut tasks, i, &mut kicks);
            }
        }
    }
    irq_restore(flags);
    kick(kicks);
}

/// Wake at most `max` tasks blocked on `key` exactly (not the `WAIT_ANY`
/// pollers). Returns how many it woke.
pub fn wake_n(key: usize, max: usize) -> usize {
    let flags = irq_save();
    irq_off();
    let mut kicks = 0u64;
    let mut n = 0;
    {
        let mut tasks = TASKS.lock();
        WAIT_SEQ.fetch_add(1, Ordering::SeqCst);
        for i in 0..MAX_TASKS {
            if n == max {
                break;
            }
            if tasks[i].state == State::Blocked && tasks[i].wait_key == key {
                wake_locked(&mut tasks, i, &mut kicks);
                n += 1;
            }
        }
    }
    irq_restore(flags);
    kick(kicks);
    n
}

/// Wake `WAIT_ANY` waiters only (something happened that a poller may care
/// about: console input, pipe/pty/device traffic, an exit). Cheap when nobody
/// waits that way.
pub fn wake_any() {
    // Bump WAIT_SEQ even when nobody waits yet: a poller between its scan
    // and `block_until` then returns instead of sleeping through this event
    // (it registers in ANY_WAITERS before it checks WAIT_SEQ).
    WAIT_SEQ.fetch_add(1, Ordering::SeqCst);
    if ANY_WAITERS.load(Ordering::SeqCst) == 0 {
        return;
    }
    wake(WAIT_ANY);
}

/// Wake one task whatever it waits for (a signal arrived). Caller holds TASKS.
pub(super) fn wake_task_locked(tasks: &mut TaskTable, slot: usize) -> u64 {
    let mut kicks = 0u64;
    if slot < MAX_TASKS {
        WAIT_SEQ.fetch_add(1, Ordering::SeqCst);
        wake_locked(tasks, slot, &mut kicks);
    }
    kicks
}

/// Deliver pending kicks computed under the lock, after it was dropped.
pub(super) fn kick_cpus_mask(kicks: u64) {
    kick(kicks);
}

/// Wake one task whatever it waits for (a signal arrived).
#[allow(dead_code)]
pub fn wake_task(slot: usize) {
    let flags = irq_save();
    irq_off();
    let kicks = {
        let mut tasks = TASKS.lock();
        wake_task_locked(&mut tasks, slot)
    };
    irq_restore(flags);
    kick(kicks);
}

/// The earliest pending sleep deadline (monotonic ns; `u64::MAX` for none):
/// each arch programs its timer for it when it comes before the next tick.
pub fn next_deadline_ns() -> u64 {
    NEXT_DEADLINE.load(Ordering::SeqCst)
}

/// Called from every timer IRQ (before `schedule`): wake tasks whose
/// deadline passed. One atomic load on the common path.
pub fn timer_tick() {
    let now = crate::time::monotonic_ns();
    if now < NEXT_DEADLINE.load(Ordering::SeqCst) {
        return;
    }
    let mut kicks = 0u64;
    {
        let Some(mut tasks) = TASKS.try_lock() else {
            return; // the holder's own tick, or the next one, will do it
        };
        let mut next = u64::MAX;
        let mut woke = false;
        for i in 0..MAX_TASKS {
            if tasks[i].state != State::Blocked || tasks[i].wake_at == 0 {
                continue;
            }
            if tasks[i].wake_at <= now {
                woke = true;
                wake_locked(&mut tasks, i, &mut kicks);
            } else {
                next = next.min(tasks[i].wake_at);
            }
        }
        NEXT_DEADLINE.store(next, Ordering::SeqCst);
        if woke {
            WAIT_SEQ.fetch_add(1, Ordering::SeqCst);
        }
    }
    kick(kicks);
}

/// Sleep the current task until the monotonic deadline (or a signal via
/// `wake_task`). Returns true if the deadline passed, false if interrupted.
pub fn sleep_until(deadline: u64, wake_on_any_event: bool) -> bool {
    loop {
        let seq = wait_seq();
        if crate::time::monotonic_ns() >= deadline {
            return true;
        }
        if crate::signal::current_should_wake() {
            return false;
        }
        block_until(if wake_on_any_event { WAIT_ANY } else { 0 }, seq, deadline);
        if wake_on_any_event {
            // Any event ends the wait: the caller re-polls its sources.
            return crate::time::monotonic_ns() >= deadline;
        }
    }
}

/// One idle-loop step: run whatever is Ready for this CPU, then halt until
/// the next interrupt unless work arrived meanwhile.
pub fn idle_step() {
    yield_now();
    let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
    CPU_IDLE[cpu].store(true, Ordering::SeqCst);
    irq_off();
    if !NEED_RESCHED[cpu].swap(false, Ordering::SeqCst) && !ready_for_cpu(cpu) {
        halt(cpu);
    } else {
        irq_on();
    }
    CPU_IDLE[cpu].store(false, Ordering::SeqCst);
}

/// Is any off-CPU task Ready for `cpu`? (IRQs off; TASKS not held.)
fn ready_for_cpu(cpu: usize) -> bool {
    let Some(tasks) = TASKS.try_lock() else {
        return true;
    };
    for i in 0..MAX_TASKS {
        if tasks[i].state == State::Ready
            && tasks[i].affinity.is_none_or(|a| a == cpu)
            && !slot_on_cpu(i)
        {
            return true;
        }
    }
    false
}

/// Spawn/fork made a task Ready: note it for its home CPU (or any idle CPU)
/// and kick that CPU if it is halted.
pub(super) fn note_ready(affinity: Option<usize>) {
    let target = affinity
        .or_else(|| (0..crate::smp::MAX_CPUS).find(|&c| CPU_IDLE[c].load(Ordering::SeqCst)));
    if let Some(c) = target {
        let c = c.min(crate::smp::MAX_CPUS - 1);
        NEED_RESCHED[c].store(true, Ordering::SeqCst);
        kick(1 << c);
    }
}

pub(super) use crate::arch::{hlt as wait, irq_off, irq_on, irq_restore, irq_save};

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
    crate::arch::stamp_stack_cpu(top, logical);
    super::arm_stack(top);
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
    let sp_now: usize = crate::arch::read_sp();

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
        t.affinity = Some(logical);
    }
    forget_frame(slot);
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
    become_idle();
    enable_preempt();
    // IRQs only after CURRENT/idle exist *and* we left the Limine stack.
    irq_on();
    ap_idle_body();
}

fn ap_idle_body() {
    loop {
        crate::smp::tlb_service();
        idle_step();
    }
}

fn ap_idle_trampoline() {
    ap_idle_bringup();
}
