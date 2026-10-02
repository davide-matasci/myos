//! Task lifecycle: spawn, fork, exec image replacement, exit, wait/reap,
//! zombies and orphans, and fork CPU affinity.

use super::*;

/// Per-parent count of exited-but-unreaped (zombie) user children, for the
/// legacy `SIGCHLD_PENDING` syscall. A counter is robust against task-state
/// scan timing (the child may be reaped by a concurrent waiter).
static ZOMBIES: [core::sync::atomic::AtomicU32; MAX_TASKS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; MAX_TASKS];

/// Record that `parent` gained a zombie child (called in `die()`).
pub fn note_zombie(parent: usize) {
    if parent < MAX_TASKS {
        ZOMBIES[parent].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}

/// A child of `parent` was reaped; drop the zombie marker (saturating).
pub fn note_reap(parent: usize) {
    if parent < MAX_TASKS {
        ZOMBIES[parent].try_update(
            core::sync::atomic::Ordering::Relaxed,
            core::sync::atomic::Ordering::Relaxed,
            |n| Some(n.saturating_sub(1)),
        ).ok();
    }
}

/// `ppid` of a task whose parent exited first. `ppid` is a slot index and
/// slots are recycled, so an orphan must not keep pointing at its dead
/// parent's slot (see `orphan_children`).
const NO_PARENT: usize = usize::MAX;

/// True once dead task `slot` has left its kernel stack for good, so the slot
/// (and the kernel stack it keeps for reuse) may be recycled.
///
/// `die()` marks the task Dead and zeroes its saved `sp`, then keeps running
/// on its own kernel stack until `schedule` switches away; `CURRENT` already
/// names the next task before that switch. `task_switch` stores the outgoing
/// stack pointer into `sp` as its last use of the old stack, so a non-zero
/// `sp` on a Dead, off-CPU task means nothing runs on that stack any more.
/// Reaping earlier let the next fork seed the same kernel stack while the
/// dying task still ran on it: the new child resumed from a clobbered frame
/// (dropbear session children crashing with garbage pointers / return
/// addresses under -smp 4).
pub(super) fn reapable(tasks: &[Task; MAX_TASKS], slot: usize) -> bool {
    tasks[slot].state == State::Dead
        && !slot_on_cpu(slot)
        && unsafe { core::ptr::read_volatile(core::ptr::addr_of!(tasks[slot].sp)) } != 0
}

/// Free the slots of dead, already-reported (`NO_PARENT`) tasks that have left
/// their kernel stacks, keeping each stack for reuse.
fn free_dead_orphans(tasks: &mut [Task; MAX_TASKS]) {
    for j in 0..MAX_TASKS {
        if tasks[j].ppid == NO_PARENT && tasks[j].user_rip != 0 && reapable(tasks, j) {
            let stack_base = tasks[j].stack_base;
            tasks[j] = EMPTY;
            if stack_base != 0 {
                tasks[j].stack_base = stack_base;
            }
        }
    }
}

/// Detach the children of dying task `id`, and free orphan zombies.
///
/// Children are found by `ppid == slot`, and a freed slot is soon reused by
/// the next fork. Children left pointing at `id` were inherited by that new
/// occupant: its `waitpid(-1)` reaped a stranger's zombie, and because pids
/// are slot indices the stranger's pid could equal the pid of the command it
/// forks next. dropbear's "exited before we recorded it" fallback then
/// matched that pid, marked its own command as already exited, and closed the
/// command's stdout early: SSH sessions lost their output ("remote echo
/// missing") or the command died with EPIPE.
///
/// Dead children can never be reaped now, so free them (like `wait_child`);
/// live ones become `NO_PARENT` and are freed by a later `die()` sweep once
/// they are dead and off-CPU.
fn orphan_children(tasks: &mut [Task; MAX_TASKS], id: usize) {
    for j in 0..MAX_TASKS {
        if j == id || tasks[j].state == State::Unused || tasks[j].user_rip == 0 {
            continue;
        }
        let dead_orphan = tasks[j].ppid == NO_PARENT && tasks[j].state == State::Dead;
        if tasks[j].ppid != id && !dead_orphan {
            continue;
        }
        if reapable(tasks, j) {
            let stack_base = tasks[j].stack_base;
            tasks[j] = EMPTY;
            if stack_base != 0 {
                tasks[j].stack_base = stack_base;
            }
        } else {
            tasks[j].ppid = NO_PARENT;
        }
    }
    if id < MAX_TASKS {
        ZOMBIES[id].store(0, core::sync::atomic::Ordering::Relaxed);
    }
}

pub fn zombie_count(parent: usize) -> usize {
    if parent < MAX_TASKS {
        ZOMBIES[parent].load(core::sync::atomic::Ordering::Relaxed) as usize
    } else {
        0
    }
}
/// Home CPU for a new top-level user task: round-robin over the online
/// CPUs. Every user task is pinned to a home (no live migration), which is
/// what lets `flush_user_tlb` stay local: an address space is only ever
/// loaded on its home CPU.
///
/// x86_64 / aarch64 skip the BSP (it owns the console, UART drain and the
/// kernel_main idle loop; with 3 APs under `-smp 4` user work has plenty of
/// room). riscv64 runs `-smp 2`, so the RR covers both harts.
/// aarch64 history: user was BSP-pinned while block I/O waited on BSP-
/// targeted SPIs; virtio-blk/NVMe are polled now, so APs can run user code
/// (verified: boot-mini needles under `-smp 4`).
pub(super) fn user_affinity() -> Option<usize> {
    let n = crate::smp::online_count();
    if n <= 1 {
        return Some(0);
    }
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    #[cfg(target_arch = "riscv64")]
    {
        Some(NEXT.fetch_add(1, Ordering::SeqCst) % n)
    }
    #[cfg(not(target_arch = "riscv64"))]
    {
        // RR over [1, n): never assign user to BSP.
        Some(1 + NEXT.fetch_add(1, Ordering::SeqCst) % (n - 1))
    }
}

/// True if `ppid` already has a Ready/Running user child. Combined with a
/// ctty check at fork: interactive shells parallel-fork (`make -j`,
/// pipelines) RR-spread; init/netd (no ctty) always inherit so getty/login
/// stay with the session home — no basename sticky allowlists.
pub(super) fn parent_has_active_child(tasks: &[Task; MAX_TASKS], ppid: usize) -> bool {
    for i in 0..MAX_TASKS {
        if i == ppid || tasks[i].ppid != ppid || tasks[i].user_rip == 0 {
            continue;
        }
        match tasks[i].state {
            State::Ready | State::Running | State::Blocked => return true,
            State::Unused | State::Dead => {}
        }
    }
    false
}

/// Fork affinity: inherit, or RR-spread when a ctty-bearing parent already
/// has an active child (parallel jobs). Returns `(affinity, kick)` — kick
/// only when the child was placed on a different home than the parent.
fn fork_child_affinity(tasks: &[Task; MAX_TASKS], ppid: usize) -> (Option<usize>, bool) {
    let parent_aff = tasks[ppid].affinity;
    if tasks[ppid].has_ctty && parent_has_active_child(tasks, ppid) {
        let next = user_affinity();
        (next, next != parent_aff)
    } else {
        (parent_aff, false)
    }
}

/// In-place exec: replace the current task's user image. Does not spawn,
/// does not bump USERS_ALIVE, does not note_exit. Keeps the fd table so
/// shell redirects and pipes survive exec.
pub fn replace_user(
    aspace: u64,
    user_rip: usize,
    user_rsp: usize,
    user_base: u64,
    image_span: usize,
    stack_off: u64,
    user_argc: usize,
    user_argv: usize,
) {
    with_current_mut(|t| {
        t.aspace = aspace;
        t.user_rip = user_rip;
        t.user_rsp = user_rsp;
        t.user_base = user_base;
        t.image_span = image_span;
        t.stack_off = stack_off;
        t.user_argc = user_argc;
        t.user_argv = user_argv;
        t.fork_regs = None;
        t.brk_cur = heap_base_for(user_base, stack_off);
        t.mmap = EMPTY_MMAP;
        // POSIX exec: ignored signals, the blocked mask and pending signals
        // survive; caught ones revert to SIG_DFL (signal_table_exec below).
        // Keep fork-assigned affinity across exec. Spreading for make -j /
        // pipelines happens at fork when a sibling is already active — not
        // via basename allowlists or blanket post-exec RR.
    });
    signal_table_exec(current_slot());
    fpu::reset(current_slot());
    #[cfg(feature = "linux-compat")]
    crate::linux::on_exec(current_slot());
    user::switch_aspace(aspace);
    set_loaded_aspace(aspace);
}

pub fn spawn(entry: fn()) {
    spawn_inner(
        0,
        Some(entry),
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        [FdEntry::Empty; MAX_FDS],
        None,
    );
}

pub fn spawn_user(
    aspace: u64,
    user_rip: usize,
    user_rsp: usize,
    user_base: u64,
    image_span: usize,
    stack_off: u64,
    user_argc: usize,
    user_argv: usize,
) {
    spawn_inner(
        aspace,
        None,
        user_rip,
        user_rsp,
        user_base,
        image_span,
        stack_off,
        user_argc,
        user_argv,
        0,
        default_user_fds(),
        None,
    );
}

/// Copy the current user task: new aspace, copied fds, fork resume regs.
/// Child is Ready and will resume userspace with rax/x0 = 0. Returns child slot.
pub fn fork_current(child_regs: ForkRegs) -> Option<usize> {
    let flags = irq_save();
    irq_off();

    // Do NOT `let t = tasks[id]` (full Task Copy). Each FdEntry can carry a
    // 96-byte Vnode path — with MAX_FDS fds + scalars a full Task snapshot on
    // the 64 KiB kstack has silently corrupted forked children before. Clone
    // fds once by reference under the lock; copy only small scalars out.
    let mut child_fds = [FdEntry::Empty; MAX_FDS];
    let (
        base,
        span,
        off,
        ppid,
        uargc,
        uargv,
        brk,
        cwd,
        cwd_len,
        mmap,
        sid,
        pgid,
        has_ctty,
        sig_ignored,
        sig_blocked,
    ) = {
        let tasks = TASKS.lock();
        let id = current_slot();
        let t = &tasks[id];
        if t.user_rip == 0 {
            drop(tasks);
            irq_restore(flags);
            return None;
        }
        for i in 0..MAX_FDS {
            child_fds[i] = fd_clone(t.fds[i]);
        }
        (
            t.user_base,
            t.image_span,
            t.stack_off,
            id,
            t.user_argc,
            t.user_argv,
            t.brk_cur,
            t.cwd,
            t.cwd_len,
            t.mmap,
            t.sid,
            t.pgid,
            t.has_ctty,
            t.sig_ignored,
            t.sig_blocked,
        )
    };

    let drop_child_fds = |fds: &mut [FdEntry; MAX_FDS]| {
        for i in 0..MAX_FDS {
            fd_drop(fds[i]);
            fds[i] = FdEntry::Empty;
        }
    };

    let Some(aspace) = user::copy_user_aspace(base, span, off, brk) else {
        drop_child_fds(&mut child_fds);
        irq_restore(flags);
        return None;
    };

    let layout = match Layout::from_size_align(STACK_SIZE, 16) {
        Ok(l) => l,
        Err(_) => {
            drop_child_fds(&mut child_fds);
            irq_restore(flags);
            return None;
        }
    };

    let (slot, reuse_stack) = {
        let mut tasks = TASKS.lock();
        free_dead_orphans(&mut tasks);
        // Reuse kernel stacks left behind by reaped fork children (stack_base kept
        // in EMPTY slots) or dead kernel threads — avoids kernel-heap alloc on CI.
        let slot = (0..MAX_TASKS)
            .find(|&i| {
                let t = &tasks[i];
                (t.state == State::Unused || reapable(&tasks, i))
                    && t.user_rip == 0
                    && t.aspace == 0
                    && t.stack_base != 0
            })
            .or_else(|| tasks.iter().position(|t| t.state == State::Unused));
        let Some(slot) = slot else {
            drop(tasks);
            drop_child_fds(&mut child_fds);
            irq_restore(flags);
            return None;
        };
        let reuse = tasks[slot].stack_base != 0;
        let stack_base = tasks[slot].stack_base;
        drop(tasks);
        (slot, (reuse, stack_base))
    };

    let (stack_base, sp, top) = if reuse_stack.0 {
        let sb = reuse_stack.1;
        let sp = unsafe { seed_stack(sb as *mut u8, STACK_SIZE, trampoline as *const () as usize) };
        (sb, sp, sb + STACK_SIZE)
    } else {
        let stack = unsafe { alloc(layout) };
        if stack.is_null() {
            drop_child_fds(&mut child_fds);
            irq_restore(flags);
            return None;
        }
        let sb = stack as usize;
        let sp = unsafe { seed_stack(stack, STACK_SIZE, trampoline as *const () as usize) };
        (sb, sp, sb + STACK_SIZE)
    };
    stamp_stack_cpu(top, crate::smp::cpu_id());

    // Inherit the chroot prefix before the child is published as Ready.
    {
        let mut roots = ROOTS.lock();
        roots[slot] = roots[ppid];
    }
    let mut tasks = TASKS.lock();
    let (child_aff, kick) = fork_child_affinity(&tasks, ppid);
    tasks[slot] = Task {
        state: State::Ready,
        stack_base,
        sp,
        entry: None,
        aspace,
        kernel_stack_top: top,
        user_rip: child_regs.rip,
        user_rsp: child_regs.rsp,
        fds: child_fds,
        user_base: base,
        image_span: span,
        stack_off: off,
        ppid,
        fork_regs: Some(child_regs),
        user_argc: uargc,
        user_argv: uargv,
        brk_cur: brk,
        exec_name: [0; 32],
        exec_name_len: 0,
        cwd,
        cwd_len,
        exit_code: 0,
        term_sig: 0,
        exited: false,
        mmap,
        sid,
        pgid,
        has_ctty,
        // POSIX: the child inherits dispositions and the blocked mask, but
        // starts with no pending signals.
        sig_pending: 0,
        sig_ignored,
        sig_blocked,
        // Sequential / init→getty (no ctty): inherit. Ctty parent with an
        // active sibling (make -j / pipelines): fresh AP RR home. Exec keeps
        // this affinity. Cross-CPU wait: die() IF-on + schedule() soft TLB.
        affinity: child_aff,
        wait_key: 0,
        wake_at: 0,
        wake_pending: false,
    };
    // Before the child becomes runnable on another CPU (TASKS still held;
    // TASKS → SIG_TABLES is the lock order).
    signal_table_fork(ppid, slot);
    fpu::fork(slot);
    #[cfg(feature = "linux-compat")]
    crate::linux::on_fork(ppid, slot);
    drop(tasks);
    user::note_fork();
    irq_restore(flags);
    // Wake the child's home CPU if it is halted (another AP after RR
    // spreading; the home CPU otherwise picks it up at its next schedule).
    if kick {
        note_ready(child_aff);
    }
    Some(slot)
}

/// True if `parent` has a child that has exited (`Dead`) and not yet been
/// reaped (legacy `SIGCHLD_PENDING` syscall).
pub fn has_exited_child(parent: usize) -> bool {
    if zombie_count(parent) > 0 {
        return true;
    }
    let flags = irq_save();
    irq_off();
    let tasks = TASKS.lock();
    let mut found = false;
    for i in 0..MAX_TASKS {
        if i != parent
            && tasks[i].ppid == parent
            && tasks[i].user_rip != 0
            && (tasks[i].state == State::Dead || tasks[i].exited)
        {
            found = true;
            break;
        }
    }
    drop(tasks);
    irq_restore(flags);
    found
}

/// Yield until a child has exited, reap it, return its pid.
/// `usize::MAX` if this task has no children. If `status_out` is `Some(va)`,
/// stores the low 8 bits of the child's exit code at that user address.
/// When `nohang` is set and children exist but none are Dead yet, returns 0
/// (POSIX WNOHANG) so waitpid cannot block the dropbear reap loop — and so a
/// WNOHANG poll with no children still returns `usize::MAX` (ECHILD), not 0
/// (which would busy-spin shells that treat 0 as "try again").
///
/// `pid` restricts the wait to that child (`waitpid(pid > 0)`). The status is
/// written as one exit-code byte (legacy) or, with `status_word`, as a POSIX
/// `int` status that distinguishes a signal death (`WIFSIGNALED`).
pub fn wait_child(
    status_out: Option<usize>,
    nohang: bool,
    pid: Option<usize>,
    status_word: bool,
) -> usize {
    let parent = current_slot();
    loop {
        let mut any = false;
        let mut reap = None;
        // Read before the scan: a child exiting after it bumps the sequence
        // and `block_until` then returns at once.
        let seq = wait_seq();
        {
            let flags = irq_save();
            irq_off();
            let mut tasks = TASKS.lock();
            for i in 0..MAX_TASKS {
                if i == parent
                    || pid.is_some_and(|p| p != i)
                    || tasks[i].ppid != parent
                    || tasks[i].user_rip == 0
                    || tasks[i].state == State::Unused
                {
                    continue;
                }
                if tasks[i].state == State::Dead || tasks[i].exited {
                    reap = Some(i);
                    break;
                }
                any = true;
            }
            if let Some(i) = reap {
                let code = tasks[i].exit_code;
                let term_sig = tasks[i].term_sig;
                if reapable(&tasks, i) {
                    let stack_base = tasks[i].stack_base;
                    tasks[i] = EMPTY;
                    if stack_base != 0 {
                        tasks[i].stack_base = stack_base;
                    }
                } else {
                    // Exited but still in `die()` (freeing its address space,
                    // or not yet off its kernel stack): report the exit now and
                    // leave the slot to `free_dead_orphans`, so nothing reuses
                    // that stack until it is Dead and off it.
                    tasks[i].ppid = NO_PARENT;
                }
                drop(tasks);
                irq_restore(flags);
                note_reap(parent);
                if let Some(va) = status_out {
                    if status_word {
                        let status: u32 = if term_sig != 0 {
                            term_sig as u32
                        } else {
                            (code as u32) << 8
                        };
                        let _ = user::copy_to_user(current_aspace(), va, &status.to_le_bytes());
                    } else {
                        let _ = user::copy_to_user(current_aspace(), va, &[code]);
                    }
                }
                return i;
            }
            drop(tasks);
            irq_restore(flags);
        }
        if !any {
            return usize::MAX;
        }
        if nohang {
            return 0;
        }
        // A signal that terminates or is caught interrupts `wait` (Ctrl-C
        // while a foreground child like `curl` runs). An ignored SIGINT is
        // never pending, so an interactive oksh ignoring it survives.
        if crate::signal::interrupt_wait() {
            return usize::MAX;
        }
        block_until(key_child(parent), seq, 0);
    }
}

fn spawn_inner(
    aspace: u64,
    entry: Option<fn()>,
    user_rip: usize,
    user_rsp: usize,
    user_base: u64,
    image_span: usize,
    stack_off: u64,
    user_argc: usize,
    user_argv: usize,
    ppid: usize,
    fds: [FdEntry; MAX_FDS],
    fork_regs: Option<ForkRegs>,
) {
    let flags = irq_save();
    irq_off();
    let layout = Layout::from_size_align(STACK_SIZE, 16).expect("task stack layout");
    let stack = unsafe { alloc(layout) };
    assert!(!stack.is_null(), "task stack alloc");
    let sp = unsafe { seed_stack(stack, STACK_SIZE, trampoline as *const () as usize) };
    let top = stack as usize + STACK_SIZE;
    // BSP-created tasks start on CPU 0; schedule restamps on migrate.
    stamp_stack_cpu(top, 0);

    let mut tasks = TASKS.lock();
    let slot = tasks
        .iter()
        .position(|t| t.state == State::Unused)
        .expect("no task slot");
    let brk_cur = if user_rip != 0 {
        heap_base_for(user_base, stack_off)
    } else {
        0
    };
    // Lock order TASKS -> ROOTS (nothing takes ROOTS first and then TASKS).
    ROOTS.lock()[slot] = NO_ROOT;
    tasks[slot] = Task {
        state: State::Ready,
        stack_base: stack as usize,
        sp,
        entry,
        aspace,
        kernel_stack_top: top,
        user_rip,
        user_rsp,
        fds,
        user_base,
        image_span,
        stack_off,
        ppid,
        fork_regs,
        user_argc,
        user_argv,
        brk_cur,
        exec_name: [0; 32],
        exec_name_len: 0,
        cwd: {
            let mut c = [0u8; 256];
            c[0] = b'/';
            c
        },
        cwd_len: 1,
        exit_code: 0,
        term_sig: 0,
        exited: false,
        mmap: EMPTY_MMAP,
        sid: slot,
        pgid: slot,
        has_ctty: false,
        sig_pending: 0,
        sig_ignored: 0,
        sig_blocked: 0,
        affinity: if aspace != 0 { user_affinity() } else { None },
        wait_key: 0,
        wake_at: 0,
        wake_pending: false,
    };
    signal_table_reset(slot);
    fpu::reset(slot);
    #[cfg(feature = "linux-compat")]
    crate::linux::on_spawn(slot);
    let aff = tasks[slot].affinity;
    drop(tasks);
    irq_restore(flags);
    note_ready(aff);
}

pub fn user_exit(code: u8) -> ! {
    with_current_mut(|t| t.exit_code = code);
    die();
}

/// Terminate the current task because of `sig` (its default action).
/// `wait` reports it as signaled; byte-status callers still see `128 + sig`.
pub fn user_exit_signal(sig: u32) -> ! {
    with_current_mut(|t| {
        t.exit_code = 128u8.wrapping_add(sig as u8);
        t.term_sig = sig as u8;
    });
    die();
}

extern "C" fn trampoline() -> ! {
    // A fresh task's first run starts here instead of returning from
    // task_switch in `schedule`.
    finish_switch();
    let (entry, user_rip, user_rsp, user_argc, user_argv, fork_regs) = {
        let flags = irq_save();
        irq_off();
        let id = current_slot();
        let mut tasks = TASKS.lock();
        let t = &mut tasks[id];
        let fr = t.fork_regs.take();
        let out = (
            t.entry,
            t.user_rip,
            t.user_rsp,
            t.user_argc,
            t.user_argv,
            fr,
        );
        drop(tasks);
        irq_restore(flags);
        out
    };
    if let Some(fr) = fork_regs {
        crate::user::enter_fork(fr);
    }
    if user_rip != 0 {
        crate::user::enter(user_rip, user_rsp, user_argc, user_argv);
    }
    irq_on();
    if let Some(f) = entry {
        f();
    }
    die()
}

pub fn die() -> ! {
    irq_off();
    let mut chld_parent = usize::MAX;
    // Closed outside TASKS below: dropping pipe/pty ends wakes their peers
    // (and a pty hangup signals the session), which take TASKS themselves.
    let mut fds_to_drop: Option<[FdEntry; MAX_FDS]> = None;
    let reclaim = {
        let mut tasks = TASKS.lock();
        let id = current_slot();
        let mut out = None;
        orphan_children(&mut tasks, id);
        if tasks[id].user_rip != 0 {
            chld_parent = tasks[id].ppid;
            user::note_exit();
            fds_to_drop = Some(tasks[id].fds);
            tasks[id].fds = [FdEntry::Empty; MAX_FDS];
            let aspace = tasks[id].aspace;
            let base = tasks[id].user_base;
            let span = tasks[id].image_span;
            let off = tasks[id].stack_off;
            let brk = tasks[id].brk_cur;
            let mmap = tasks[id].mmap;
            tasks[id].aspace = 0;
            tasks[id].user_base = 0;
            tasks[id].image_span = 0;
            tasks[id].stack_off = 0;
            tasks[id].brk_cur = 0;
            tasks[id].mmap = EMPTY_MMAP;
            tasks[id].exited = true;
            if aspace != 0 {
                out = Some((aspace, base, span, off, brk, mmap));
            }
        }
        out
    };
    if let Some(fds) = fds_to_drop {
        for entry in fds {
            fd_drop(entry);
        }
    }
    // Notify the parent now that the TASKS lock is dropped: the exit status
    // is final, so the parent can reap it while this task still frees its
    // address space below (that walk is not on the parent's critical path).
    // SIGCHLD's default action is ignore, so a parent without a handler is
    // unaffected; a parent polling SIGCHLD_TAKE sees the bit.
    if chld_parent != usize::MAX {
        crate::signal::raise_sigchld(chld_parent);
        note_zombie(chld_parent);
        // The parent may be blocked in `wait`; pollers may watch for exits.
        wake(key_child(chld_parent));
        wake_any();
    }
    // Reclaim/TLB shootdown must run with IF on: remotes ACK the shootdown
    // IPI only after sti. Holding cli here deadlocked a parent waiter that was
    // also briefly cli (schedule/wait_child) when the child had been re-homed
    // onto another AP — bios triple-faulted under -smp 4 after the first
    // remote-AP exit.
    //
    // The task stays runnable (not Dead) until the reclaim has finished: a
    // Dead task is never scheduled again, so a timer preemption in the middle
    // of the (long) heap-window walk used to abandon the reclaim for good and
    // leak the whole address space — ~370 frames per forked child, the
    // per-exec "leak" that pushed the curated os-test run out of memory. With
    // `aspace` already cleared, `schedule` runs it on the kernel root. The
    // parent may already have reported the exit (`exited`), but the slot is
    // only recycled once it is Dead and off its stack (`reapable`).
    irq_on();
    if let Some((aspace, base, span, off, brk, mmap)) = reclaim {
        user::reclaim_user_aspace(aspace, base, span, off, brk, &mmap);
    }
    irq_off();
    {
        let mut tasks = TASKS.lock();
        let id = current_slot();
        tasks[id].state = State::Dead;
        tasks[id].entry = None;
        // `task_switch` stores the outgoing stack pointer here once this task
        // has left its kernel stack for good; until then it is not reapable.
        tasks[id].sp = 0;
    }
    // IRQs stay off from Dead to the switch: a preemption here would never be
    // resumed.
    schedule();
    loop {
        irq_on();
        wait();
        irq_off();
        schedule();
    }
}
