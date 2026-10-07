//! Threads: several tasks running one process.
//!
//! The process is its leader's slot (pid == the leader's tid): it holds the
//! address-space layout, fds, cwd, signal dispositions, job control and the
//! exit status, and every other thread of it reaches them through `tgid`.
//! A thread's own slot has its registers, kernel stack, thread pointer,
//! blocked/pending signals and scheduling state.
//!
//! A new thread gets a home CPU of its own (`place_thread`, round-robin as
//! `user_affinity` places processes), so the threads of a process run in
//! parallel. Their address space is then loaded on several CPUs: a mapping
//! removed or narrowed is flushed on all of them before its frame is freed
//! (`user::flush_user_tlb`).
//!
//! The process ends with its last thread. The leader carries it, so it goes
//! last: a leader whose thread ends waits for the others, then exits the
//! process (`die`). `exit_group` (`exit`, a fatal signal) kills the other
//! threads first.

use super::*;

/// Start a thread in the current process that enters user mode with
/// `regs`, with thread pointer `tls` (`None`: the caller's). Returns its tid.
/// It waits on this CPU, which runs the syscall with interrupts off, until
/// [`place_thread`] gives it a CPU of its own.
pub fn spawn_thread(regs: UserRegs, tls: Option<u64>) -> Option<usize> {
    let regs = {
        let mut r = regs;
        if let Some(v) = tls {
            crate::arch::regs_set_tls(&mut r, v);
        }
        r
    };
    let flags = irq_save();
    irq_off();
    let me = current_slot();
    let tls = tls.unwrap_or_else(tp::get);
    let Some((slot, stack_base, sp, top)) = claim_slot() else {
        irq_restore(flags);
        return None;
    };
    let mut tasks = TASKS.lock();
    let pid = tasks[me].tgid;
    let affinity = tasks[me].affinity;
    tasks[slot] = Task {
        state: State::Ready,
        stack_base,
        sp,
        kernel_stack_top: top,
        aspace: tasks[me].aspace,
        user_rip: regs.rip,
        user_rsp: regs.rsp,
        start_regs: Some(regs),
        // Not anyone's child: nobody waits for a thread, its slot is freed
        // once it is dead (`free_dead_orphans`).
        ppid: NO_PARENT,
        sig_blocked: tasks[me].sig_blocked,
        affinity,
        tgid: pid,
        name: tasks[me].name,
        ..EMPTY
    };
    super::acct::start(slot);
    signal_thread_reset(slot);
    fpu::fork(slot);
    tp::init(slot, tls);
    crate::personality::on_thread(me, slot);
    drop(tasks);
    irq_restore(flags);
    note_ready(affinity);
    Some(slot)
}

/// Give thread `tid`, new from [`spawn_thread`] and not run yet, a home CPU
/// of its own. Its creator calls this once it has set up what the thread
/// must find when it starts (`clone`'s thread ids).
pub fn place_thread(tid: usize) {
    let flags = irq_save();
    irq_off();
    let mut tasks = TASKS.lock();
    let fresh = tid < MAX_TASKS && tasks[tid].state == State::Ready && tasks[tid].start_regs.is_some();
    let affinity = if fresh { user_affinity() } else { None };
    if affinity.is_some() {
        tasks[tid].affinity = affinity;
    }
    drop(tasks);
    irq_restore(flags);
    if affinity.is_some() {
        note_ready(affinity);
    }
}

/// End the calling thread with `code`. The leader carries the process, so
/// it waits for the other threads and then ends the process with `code`
/// (unless it is already ending with another status).
pub fn thread_exit(code: u8) -> ! {
    let (me, pid) = with_thread_mut(|t| (current_slot(), t.tgid));
    if me == pid {
        with_leader_mut(|t| {
            if !t.group_exit {
                t.exit_code = code;
            }
        });
        wait_for_threads(pid);
        die();
    }
    // Nothing of the process is this thread's to free.
    hand_over_cpu_time();
    retire(Some(key_threads(pid)));
}

/// An ending thread (not the leader) gives its CPU time to its process,
/// which counts it from now on (`/proc/<pid>/status`).
fn hand_over_cpu_time() {
    let flags = irq_save();
    irq_off();
    let mut tasks = TASKS.lock();
    let me = current_slot();
    super::acct::charge(me);
    let ns = super::acct::take_cpu_ns(me);
    let pid = tasks[me].tgid;
    if let Some(p) = tasks.proc_opt_mut(pid) {
        p.ended_cpu_ns += ns;
    }
    drop(tasks);
    irq_restore(flags);
}

/// End the whole process with `code`, or because of `sig` (non-zero): the
/// first caller sets the exit status, the other threads are killed, and the
/// leader ends the process once it is the last thread.
pub fn exit_group(code: u8, sig: u32) -> ! {
    let pid = kill_other_threads(|leader| {
        if !leader.group_exit {
            leader.group_exit = true;
            leader.exit_code = code;
            leader.term_sig = sig as u8;
        }
    });
    if current_slot() == pid {
        wait_for_threads(pid);
        die();
    }
    hand_over_cpu_time();
    retire(Some(key_threads(pid)));
}

/// Before exec replaces the process image: end its other threads. Only the
/// leader may exec while there are others (Linux lets any thread, which then
/// takes over the pid; not supported). False if the caller may not.
pub fn exec_alone() -> bool {
    let (me, pid) = with_thread_mut(|t| (current_slot(), t.tgid));
    if !has_other_threads(pid) {
        return true;
    }
    if me != pid {
        return false;
    }
    // `group_exit` keeps the killed threads from setting an exit status.
    kill_other_threads(|leader| leader.group_exit = true);
    wait_for_threads(pid);
    with_leader_mut(|t| t.group_exit = false);
    true
}

/// Under TASKS: apply `mark` to the current process's leader, then send
/// `SIGKILL` to every other thread of it (they end at their next syscall
/// exit, or at once if preempted in user mode). Returns the pid.
fn kill_other_threads(mark: impl FnOnce(&mut Task)) -> usize {
    let flags = irq_save();
    irq_off();
    let mut kicks = 0;
    let pid = {
        let mut tasks = TASKS.lock();
        let me = current_slot();
        let pid = tasks[me].tgid;
        mark(&mut tasks[pid]);
        for j in 0..MAX_TASKS {
            if j != me && is_thread_of(&tasks, j, pid) {
                tasks[j].sig_pending |= 1 << crate::signal::SIGKILL;
                kicks |= wake_task_locked(&mut tasks, j);
            }
        }
        pid
    };
    irq_restore(flags);
    kick_cpus_mask(kicks);
    pid
}

/// `j` is a live (not yet dead) user thread of process `pid`.
fn is_thread_of(tasks: &TaskTable, j: usize, pid: usize) -> bool {
    let t = &tasks[j];
    t.tgid == pid
        && t.user_rip != 0
        && matches!(t.state, State::Ready | State::Running | State::Blocked)
}

/// Whether process `pid` has live threads besides the caller.
fn has_other_threads(pid: usize) -> bool {
    let flags = irq_save();
    irq_off();
    let tasks = TASKS.lock();
    let me = current_slot();
    let any = (0..MAX_TASKS).any(|j| j != me && is_thread_of(&tasks, j, pid));
    drop(tasks);
    irq_restore(flags);
    any
}

/// The leader waits until it is the last thread of process `pid`.
fn wait_for_threads(pid: usize) {
    loop {
        let seq = wait_seq();
        if !has_other_threads(pid) {
            return;
        }
        block_until(key_threads(pid), seq, 0);
    }
}

/// The calling thread's id.
pub fn current_tid() -> usize {
    current_slot()
}

/// Outcome of [`wait_addr`].
pub enum AddrWait {
    /// Woken by [`wake_addr`] (or spuriously: callers re-check their condition).
    Woken,
    /// The word at the address did not hold the expected value.
    Changed,
    TimedOut,
    /// A signal that acts ended the wait (the syscall is marked interrupted).
    Interrupted,
    Fault,
}

/// Block the calling thread while the 32-bit word at user address `addr`
/// holds `expected`, until [`wake_addr`] on the same address, the monotonic
/// `deadline` (0 = none) or a signal. The building block of user-space
/// locks (Linux `futex`).
pub fn wait_addr(addr: usize, expected: u32, deadline: u64) -> AddrWait {
    if addr % 4 != 0 || !user::buffer_ok(addr, 4) {
        return AddrWait::Fault;
    }
    let key = key_addr(current_pid(), addr);
    // Read the sequence before the word: a wake after this makes
    // `block_until` return at once, so none is lost between the two.
    let seq = wait_seq();
    let mut word = [0u8; 4];
    if !user::copy_from_user(current_aspace(), addr, &mut word) {
        return AddrWait::Fault;
    }
    if u32::from_le_bytes(word) != expected {
        return AddrWait::Changed;
    }
    if crate::signal::interrupt_wait() {
        return AddrWait::Interrupted;
    }
    block_until(key, seq, deadline);
    if crate::signal::interrupt_wait() {
        return AddrWait::Interrupted;
    }
    if deadline != 0 && crate::time::monotonic_ns() >= deadline {
        return AddrWait::TimedOut;
    }
    AddrWait::Woken
}

/// Wake at most `max` threads of the current process waiting on `addr`
/// ([`wait_addr`]). Returns how many.
pub fn wake_addr(addr: usize, max: usize) -> usize {
    wake_n(key_addr(current_pid(), addr), max)
}
