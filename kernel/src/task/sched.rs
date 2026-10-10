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
    let flags = irq_save();
    irq_off();
    set_name(&mut TASKS.lock()[current_slot()], b"idle");
    irq_restore(flags);
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
    let _ = pick_and_switch();
}

/// What [`pick_and_switch`] did.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pick {
    /// Ran another task; the caller is back on a CPU, Running.
    Switched,
    /// Nothing else to run here: the caller kept the CPU. `blocked`: it is
    /// `Blocked` (a wait with nothing else runnable halts next).
    Stayed { blocked: bool },
}

/// Run the next task for this CPU, if there is one: the first of the CPU's
/// run queue and the floating tasks after the current one, else, when the
/// current task cannot go on, a user task pulled from a busy CPU, else the
/// CPU's idle task.
fn pick_and_switch() -> Pick {
    if !PREEMPT_ON.load(Ordering::SeqCst) {
        return Pick::Stayed { blocked: false };
    }
    // Hold IF off across TASKS + switch. Caller may already have IF clear
    // (timer, yield); save/restore so we never leave IF on while locked.
    let flags = irq_save();
    irq_off();
    // An interrupt that ended a tickless halt and switches tasks from inside
    // it: the tick back first (`halt`).
    crate::arch::timer_resume();
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
    let mut stayed_blocked = false;
    let switch = {
        let mut tasks = TASKS.lock();
        let current = current_slot();
        // Every pass, not only a switch: a task alone on its CPU is charged
        // at each tick.
        super::acct::charge(current);

        crate::smp::note_schedule();
        let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
        let idle = IDLE_SLOT[cpu].load(Ordering::Relaxed);
        // The run queues hold only Ready tasks that are on no CPU: a woken
        // task still halting in `block_until` somewhere resumes there.
        let queued = RUNQ[cpu].load(Ordering::SeqCst) | FLOAT.load(Ordering::SeqCst);
        // A task that can go on keeps its CPU when nothing is queued: it
        // keeps its address space too, where a round through the idle task
        // loaded the kernel's and back (a whole TLB flush each way, every
        // tick). The idle task runs only when nothing else can.
        let current_runs = current != idle && matches!(tasks[current].state, State::Running | State::Ready);
        let mut next = pick_from(queued, current).inspect(|&n| dequeue(n, tasks[n].affinity));
        if next.is_none() && !current_runs {
            next = pull(&mut tasks, cpu);
        }
        let next = match next {
            Some(n) => n,
            None if !current_runs && idle != usize::MAX && idle != current => idle,
            None => current,
        };

        if next == current {
            if tasks[current].state == State::Ready {
                tasks[current].state = State::Running;
            }
            stayed_blocked = tasks[current].state == State::Blocked;
            None
        } else {
            // A CPU that runs a task is not idle; the idle task is picked
            // only by the idle loop, which says so itself.
            if next != idle {
                CPU_IDLE[cpu].store(false, Ordering::SeqCst);
            }
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
            SWITCHED_FROM[cpu].store(current, Ordering::SeqCst);
            Some((old_sp, new_sp, kstack, old_kstack, aspace, current, next, old_user))
        }
    };

    let Some((old_sp, new_sp, kstack, old_kstack, aspace, old, next, old_user)) = switch else {
        irq_restore(flags);
        return Pick::Stayed { blocked: stayed_blocked };
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
    // A shootdown passed this CPU over while it was idle: the switch
    // flushes, or the TLB is flushed here (`smp::tlb_shootdown`).
    let stale = crate::smp::tlb_take_stale(crate::smp::cpu_id());
    if want != loaded_aspace() {
        user::switch_aspace(want);
        set_loaded_aspace(want);
    } else if stale {
        crate::arch::flush_tlb_local();
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
    Pick::Switched
}

// --- Run queues --------------------------------------------------------------
//
// The Ready tasks that are on no CPU, a bit per slot: one queue per CPU for
// the tasks homed on it, one for the tasks that may run anywhere (the
// kernel's floating threads). The idle tasks are never queued: `schedule`
// falls back to its CPU's. The bits change with the task states, under
// TASKS; the idle loop reads them without it to ask "is anything ready
// here?", and `enqueue` to pick a CPU to kick.

const _: () = assert!(MAX_TASKS <= 64);

/// Per CPU: its run queue.
static RUNQ: [AtomicU64; crate::smp::MAX_CPUS] = [const { AtomicU64::new(0) }; crate::smp::MAX_CPUS];
/// The queue of the tasks with no home.
static FLOAT: AtomicU64 = AtomicU64::new(0);
/// Per CPU: how many tasks it pulled from another CPU's queue (`/proc/cpuinfo`).
static PULLS: [AtomicU64; crate::smp::MAX_CPUS] = [const { AtomicU64::new(0) }; crate::smp::MAX_CPUS];

pub fn pulls(cpu: usize) -> u64 {
    PULLS.get(cpu).map_or(0, |n| n.load(Ordering::Relaxed))
}

fn queue(affinity: Option<usize>) -> &'static AtomicU64 {
    match affinity {
        Some(c) => &RUNQ[c.min(crate::smp::MAX_CPUS - 1)],
        None => &FLOAT,
    }
}

/// Task `slot`, Ready and on no CPU, joins the queue its home says (under
/// TASKS). It is picked by its home CPU, or by an idle one: see
/// [`kick_for`] for the kicks that get it run now.
fn enqueue(slot: usize, affinity: Option<usize>) {
    queue(affinity).fetch_or(1 << slot, Ordering::SeqCst);
}

fn dequeue(slot: usize, affinity: Option<usize>) {
    queue(affinity).fetch_and(!(1 << slot), Ordering::SeqCst);
}

/// Slot `slot` leaves every queue (a recycled slot).
pub(super) fn unqueue(slot: usize) {
    for q in RUNQ.iter().chain(core::iter::once(&FLOAT)) {
        q.fetch_and(!(1 << slot), Ordering::SeqCst);
    }
}

/// Is anything queued that this CPU may run?
fn queued_for(cpu: usize) -> bool {
    RUNQ[cpu].load(Ordering::SeqCst) | FLOAT.load(Ordering::SeqCst) != 0
}

/// The bit of `mask` after `current`, round-robin: the first above it, or
/// else its lowest.
fn pick_from(mask: u64, current: usize) -> Option<usize> {
    if mask == 0 {
        return None;
    }
    let above = match current.checked_add(1) {
        Some(n) if n < 64 => mask & (u64::MAX << n),
        _ => 0,
    };
    let from = if above != 0 { above } else { mask };
    Some(from.trailing_zeros() as usize)
}

/// Whether user tasks run on `cpu` (`user_affinity` keeps them off the BSP
/// on x86_64 and aarch64).
fn hosts_user(cpu: usize) -> bool {
    cpu != 0 || crate::arch::USER_TASKS_ON_BSP
}

/// Whether another CPU may take task `t` from its home's queue: a user
/// task with its address space (one in `die` has let it go: it finishes
/// where it is), and not a thread that has not run yet (it waits on its
/// creator's CPU until `place_thread`, which the creator calls once the
/// thread's ids are stored where it will read them).
fn pullable(slot: usize, t: &Task) -> bool {
    t.aspace != 0 && (t.tgid == slot || t.start_regs.is_none())
}

/// A CPU with nothing to run takes a user task waiting in a busy CPU's
/// queue and makes itself its home (issue #282): the task is on no CPU and
/// its last CPU has switched to another root, so its address space is
/// loaded where it runs, as a task's is after any switch. Called under
/// TASKS for the calling CPU `cpu`; the slot, dequeued and re-homed.
fn pull(tasks: &mut TaskTable, cpu: usize) -> Option<usize> {
    if !hosts_user(cpu) {
        return None;
    }
    for c in 0..crate::smp::MAX_CPUS {
        // An idle CPU runs its own queue as soon as it is kicked.
        if c == cpu || CPU_IDLE[c].load(Ordering::SeqCst) {
            continue;
        }
        let mut mask = RUNQ[c].load(Ordering::SeqCst);
        while mask != 0 {
            let slot = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            if pullable(slot, &tasks[slot]) {
                dequeue(slot, Some(c));
                tasks[slot].affinity = Some(cpu);
                PULLS[cpu].fetch_add(1, Ordering::Relaxed);
                return Some(slot);
            }
        }
    }
    None
}

/// The CPUs to kick for task `slot`, just queued with `affinity`: its home,
/// when it has one (`kick` IPIs it only when it is halted; the flag makes
/// an idle loop between its pick and its halt look again); and when the
/// home is busy, or there is none, an idle CPU that may run it, so it is
/// pulled now rather than at the home's next tick. `from_switch`: queued by
/// the CPU it just left, which needs no flag for it.
fn kick_for(tasks: &TaskTable, slot: usize, affinity: Option<usize>, from_switch: bool, kicks: &mut u64) {
    let home = affinity.map(|c| c.min(crate::smp::MAX_CPUS - 1));
    if let Some(h) = home {
        if !from_switch {
            NEED_RESCHED[h].store(true, Ordering::SeqCst);
            *kicks |= 1 << h;
        }
        if !pullable(slot, &tasks[slot]) || CPU_IDLE[h].load(Ordering::SeqCst) {
            return;
        }
    }
    let puller = (0..crate::smp::MAX_CPUS).find(|&c| {
        Some(c) != home && CPU_IDLE[c].load(Ordering::SeqCst) && (home.is_none() || hosts_user(c))
    });
    if let Some(c) = puller {
        NEED_RESCHED[c].store(true, Ordering::SeqCst);
        *kicks |= 1 << c;
    }
}

/// A task made Ready while on no CPU (spawn, fork, a new thread; under
/// TASKS): queued and its kicks noted, to be sent once TASKS is dropped
/// (`kick_cpus_mask`).
pub(super) fn ready_locked(tasks: &TaskTable, slot: usize, kicks: &mut u64) {
    let affinity = tasks[slot].affinity;
    enqueue(slot, affinity);
    kick_for(tasks, slot, affinity, false, kicks);
}

/// Thread `slot`, queued and not run yet, gets the home `affinity` (under
/// TASKS): it moves queue, and the kicks follow.
pub(super) fn rehome_locked(tasks: &mut TaskTable, slot: usize, affinity: Option<usize>, kicks: &mut u64) {
    dequeue(slot, tasks[slot].affinity);
    tasks[slot].affinity = affinity;
    ready_locked(tasks, slot, kicks);
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
    let mut kicks = 0u64;
    {
        // Clear SWITCHED_FROM under TASKS, where `wake` reads it: cleared
        // before the lock, a wake in between made a Blocked `prev` Ready at
        // once, a peer resumed it before its frame was recorded, and this
        // late update then turned it Ready while it ran there.
        let mut tasks = TASKS.lock();
        let prev = SWITCHED_FROM[cpu].swap(usize::MAX, Ordering::SeqCst);
        if prev == usize::MAX {
            return;
        }
        remember_frame(prev, tasks[prev].sp, cpu);
        let ready = if tasks[prev].state == State::Running {
            true
        } else if tasks[prev].state == State::Blocked && tasks[prev].wake_pending {
            // Woken while it was still on the old CPU's stack: runnable now.
            tasks[prev].wake_pending = false;
            true
        } else {
            false
        };
        // Queued now that it is off the CPU; the idle task waits for its
        // CPU to have nothing else.
        if ready {
            tasks[prev].state = State::Ready;
            if prev != IDLE_SLOT[cpu].load(Ordering::Relaxed) {
                let affinity = tasks[prev].affinity;
                enqueue(prev, affinity);
                kick_for(&tasks, prev, affinity, true, &mut kicks);
            }
        }
    }
    kick(kicks);
}

/// True while `slot` is still being switched away from on some CPU: its
/// saved stack pointer is not valid until `finish_switch` runs there, and
/// `finish_switch` still expects the task that left in the slot.
pub(super) fn mid_switch(slot: usize) -> bool {
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

/// Whether `cpu` is in the idle loop: between picks, or halted. Said before
/// a task is picked (`CPU_IDLE` is cleared under `TASKS` before the switch),
/// so a shootdown may pass an idle CPU over (`smp::tlb_shootdown`).
pub fn cpu_idle(cpu: usize) -> bool {
    CPU_IDLE[cpu.min(crate::smp::MAX_CPUS - 1)].load(Ordering::SeqCst)
}
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

/// The longest a halted CPU without its tick sleeps with no deadline due: a
/// wakeup that was never kicked costs this much latency, not a hang.
const IDLE_BACKSTOP_NS: u64 = 1_000_000_000;

/// Halt until an interrupt: called with interrupts off, returns with them on.
///
/// Tickless idle: a CPU stops its tick while it halts, its timer armed only
/// for the next sleep deadline (`NEXT_DEADLINE`, or the backstop), so an
/// idle CPU is not woken 100 times a second. Whatever ends the halt and
/// runs something puts the tick back for preemption: this function, or
/// `schedule` when an interrupt switches to another task from inside the
/// halt. CPU 0 also wakes for the cursor blink while the screen shows it,
/// and keeps ticking when the UART has no interrupt: its tick drains the
/// UART then (`input::tick`).
fn halt(cpu: usize) {
    IDLE_HALTS[cpu].fetch_add(1, Ordering::Relaxed);
    let tickless = cpu != 0 || crate::input::uart_irq();
    if tickless {
        let backstop = crate::time::monotonic_ns().saturating_add(IDLE_BACKSTOP_NS);
        let mut wake = NEXT_DEADLINE.load(Ordering::SeqCst).min(backstop);
        if cpu == 0 && crate::console::cursor_blinks() {
            wake = wake.min(crate::time::next_blink_ns());
        }
        crate::arch::timer_idle(wake);
    }
    super::acct::idle(current_slot(), crate::arch::idle_wait);
    if tickless {
        irq_off();
        crate::arch::timer_resume();
        irq_on();
    }
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
        // Idle from here until something runs: a wake for this CPU
        // meanwhile sets its flag and IPIs it. Then one pass: run something
        // else if there is anything (it comes back here once this task was
        // picked again, Running), or learn that there is nothing.
        let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
        CPU_IDLE[cpu].store(true, Ordering::SeqCst);
        match pick_and_switch() {
            // Picked from a queue: Running, on this or another CPU; or
            // woken while halting here: Running too.
            Pick::Switched | Pick::Stayed { blocked: false } => break,
            Pick::Stayed { blocked: true } => {}
        }
        // Nothing else runnable here: halt on our own stack until an
        // interrupt (timer / IPI from a waker) arrives, unless a wake came
        // since the pick. `idle_wait` enters with IRQs off, so a wake that
        // already raised an IPI is pending and ends the halt immediately.
        if !NEED_RESCHED[cpu].swap(false, Ordering::SeqCst) && !queued_for(cpu) {
            halt(cpu);
        } else {
            irq_on();
        }
        irq_off();
    }
    let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
    CPU_IDLE[cpu].store(false, Ordering::SeqCst);
    // Back to the task that blocked, on the same CPU: the shootdowns that
    // passed this CPU over while it was idle are honored here.
    if crate::smp::tlb_take_stale(cpu) {
        crate::arch::flush_tlb_local();
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
    // Still leaving a CPU: `finish_switch` queues it there.
    if mid_switch(slot) {
        t.wake_pending = true;
        return;
    }
    t.state = State::Ready;
    // Halting in `block_until`: it resumes on that CPU; else it is queued
    // for its home, or for whoever takes it.
    let halting = CURRENT.iter().position(|cur| cur.load(Ordering::SeqCst) == slot);
    if let Some(c) = halting {
        NEED_RESCHED[c].store(true, Ordering::SeqCst);
        *kicks |= 1 << c;
        return;
    }
    let affinity = t.affinity;
    enqueue(slot, affinity);
    kick_for(tasks, slot, affinity, false, kicks);
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

/// Process `i`'s `ITIMER_REAL` at `now` (under `TASKS`): `SIGALRM` once it
/// is due, re-armed by its interval; returns when it is next due
/// (`u64::MAX`: disarmed). A due alarm whose signal could not be sent yet
/// stays due, so the next tick tries again.
fn alarm_tick(tasks: &mut TaskTable, i: usize, now: u64, kicks: &mut u64) -> u64 {
    let (at, every) = (tasks[i].alarm_at, tasks[i].alarm_every);
    if !super::signals::signal_live(&tasks[i]) {
        tasks[i].alarm_at = 0;
        return u64::MAX;
    }
    if at > now {
        return at;
    }
    let Some(k) = super::signals::signal_send_locked(tasks, i, crate::signal::SIGALRM) else {
        return at;
    };
    *kicks |= k;
    // Late by more than one interval: the missed expiries are one signal.
    let next = if every == 0 { 0 } else { at + every * ((now - at) / every + 1) };
    tasks[i].alarm_at = next;
    if next == 0 { u64::MAX } else { next }
}

/// Set process `pid`'s `ITIMER_REAL` to fire in `value_ns` (0 = disarm),
/// then every `interval_ns`; returns what was left of the old one and its
/// interval.
pub fn itimer_swap(pid: usize, new: Option<(u64, u64)>) -> (u64, u64) {
    if pid >= MAX_TASKS {
        return (0, 0);
    }
    let flags = irq_save();
    irq_off();
    let now = crate::time::monotonic_ns();
    let old = {
        let mut tasks = TASKS.lock();
        let t = &mut tasks[pid];
        let left = if t.alarm_at == 0 { 0 } else { t.alarm_at.saturating_sub(now).max(1) };
        let old = (left, t.alarm_every);
        if let Some((value, interval)) = new {
            t.alarm_at = if value == 0 { 0 } else { now.saturating_add(value) };
            t.alarm_every = if value == 0 { 0 } else { interval };
            if t.alarm_at != 0 {
                NEXT_DEADLINE.fetch_min(t.alarm_at, Ordering::SeqCst);
            }
        }
        old
    };
    irq_restore(flags);
    old
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
            if tasks[i].alarm_at != 0 {
                next = next.min(alarm_tick(&mut tasks, i, now, &mut kicks));
            }
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

/// One idle-loop step: run whatever is queued for this CPU (or pull a
/// task from a busy one), then halt until the next interrupt unless work
/// arrived meanwhile. One pass of the scheduler decides (issue #369): the
/// CPU says it is idle before the pick, so a task queued for it after the
/// pick kicks it (the flag, and an IPI that is pending when it halts), and
/// one more look at its queue catches the rest.
pub fn idle_step() {
    let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
    irq_off();
    CPU_IDLE[cpu].store(true, Ordering::SeqCst);
    if let Pick::Stayed { .. } = pick_and_switch() {
        if !NEED_RESCHED[cpu].swap(false, Ordering::SeqCst) && !queued_for(cpu) {
            halt(cpu);
        } else {
            irq_on();
        }
    } else {
        irq_on();
    }
    CPU_IDLE[cpu].store(false, Ordering::SeqCst);
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
    super::acct::start(slot);
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
