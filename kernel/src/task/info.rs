//! What `/proc/<pid>` shows of the processes and threads (`docs/proc.md`):
//! a snapshot of one process or thread, taken under one TASKS lock, so each
//! status line is consistent in itself.

use alloc::vec::Vec;

use super::*;

/// A process (a thread-group leader slot) as `/proc/<pid>/status` shows it.
pub struct ProcInfo {
    pub pid: usize,
    pub name: [u8; NAME_MAX],
    pub state: &'static str,
    /// 0 for none (`init`, a kernel thread).
    pub ppid: usize,
    pub pgid: usize,
    pub sid: usize,
    pub threads: usize,
    /// Virtual size: image, stack, heap up to the break, mmap regions.
    pub size_kib: u64,
    /// Its threads' CPU time, those that ended included.
    pub cpu_ns: u64,
    /// Its reaped children's CPU time, theirs included.
    pub child_cpu_ns: u64,
    pub start_ns: u64,
    /// A kernel thread (or the idle tasks): no address space of its own.
    pub kernel: bool,
}

/// A thread as `/proc/<pid>/task/<tid>/status` shows it.
pub struct ThreadInfo {
    pub tid: usize,
    pub pid: usize,
    pub name: [u8; NAME_MAX],
    pub state: &'static str,
    /// The CPU it runs on (its home CPU), `None` while it may run anywhere.
    pub cpu: Option<usize>,
    /// What it is blocked on (`sched::block_until`'s key), 0 when it is not.
    pub wait_key: usize,
    pub cpu_ns: u64,
    pub start_ns: u64,
    pub sig_pending: u32,
    pub sig_blocked: u32,
}

/// A slot `/proc` shows: one that holds a task.
fn live(t: &Task) -> bool {
    !matches!(t.state, State::Unused | State::Claimed)
}

fn state_name(t: &Task) -> &'static str {
    if t.exited || t.state == State::Dead {
        return "zombie";
    }
    match t.state {
        State::Running => "running",
        State::Ready => "ready",
        _ => "blocked",
    }
}

fn snapshot<R>(f: impl FnOnce(&TaskTable) -> R) -> R {
    let flags = irq_save();
    irq_off();
    let tasks = TASKS.lock();
    let r = f(&tasks);
    drop(tasks);
    irq_restore(flags);
    r
}

/// The pids of the processes, in slot order.
pub fn processes() -> Vec<usize> {
    snapshot(|tasks| (0..MAX_TASKS).filter(|&i| live(&tasks[i]) && tasks[i].tgid == i).collect())
}

/// The tids of the threads of process `pid`, the leader first.
pub fn threads_of(pid: usize) -> Option<Vec<usize>> {
    snapshot(|tasks| {
        if pid >= MAX_TASKS || !live(&tasks[pid]) || tasks[pid].tgid != pid {
            return None;
        }
        Some((0..MAX_TASKS).filter(|&i| live(&tasks[i]) && tasks[i].tgid == pid).collect())
    })
}

/// Process `pid` (a leader slot), if it exists.
pub fn process_info(pid: usize) -> Option<ProcInfo> {
    snapshot(|tasks| {
        if pid >= MAX_TASKS || !live(&tasks[pid]) || tasks[pid].tgid != pid {
            return None;
        }
        let leader = &tasks[pid];
        let mut threads = 0;
        let mut cpu_ns = 0;
        // The busiest of its threads: running, else ready, else blocked. A
        // leader that only waits for the others to end does not count.
        let mut rank = 0;
        let mut state = state_name(leader);
        for i in 0..MAX_TASKS {
            let t = &tasks[i];
            if !live(t) || t.tgid != pid {
                continue;
            }
            threads += 1;
            cpu_ns += acct::cpu_ns(i);
            if i == pid && t.wait_key == key_threads(pid) {
                continue;
            }
            let r = match t.state {
                State::Running => 3,
                State::Ready => 2,
                State::Blocked => 1,
                _ => 0,
            };
            if r > rank && !leader.exited {
                rank = r;
                state = state_name(t);
            }
        }
        let proc = tasks.proc_opt(pid);
        let size = proc.map_or(0, |p| {
            if p.user_base == 0 {
                return 0;
            }
            let heap = p.brk_cur.saturating_sub(heap_base_for(p.user_base, p.stack_off));
            let mmap: u64 = p.mmap.iter().map(|r| u64::from(r.pages) * user::PAGE as u64).sum();
            p.image_span as u64 + (user::USER_STACK_PAGES * user::PAGE) as u64 + heap + mmap
        });
        Some(ProcInfo {
            pid,
            name: leader.name,
            state,
            ppid: if leader.ppid == NO_PARENT { 0 } else { leader.ppid },
            pgid: proc.map_or(0, |p| p.pgid),
            sid: proc.map_or(0, |p| p.sid),
            threads,
            size_kib: size / 1024,
            cpu_ns: cpu_ns + proc.map_or(0, |p| p.ended_cpu_ns),
            child_cpu_ns: proc.map_or(0, |p| p.child_cpu_ns),
            start_ns: acct::start_ns(pid),
            kernel: leader.aspace == 0 && leader.user_rip == 0,
        })
    })
}

/// Thread `tid`, if it exists.
pub fn thread_info(tid: usize) -> Option<ThreadInfo> {
    snapshot(|tasks| {
        if tid >= MAX_TASKS || !live(&tasks[tid]) {
            return None;
        }
        let t = &tasks[tid];
        Some(ThreadInfo {
            tid,
            pid: t.tgid,
            name: t.name,
            state: state_name(t),
            cpu: t.affinity,
            wait_key: if t.state == State::Blocked { t.wait_key } else { 0 },
            cpu_ns: acct::cpu_ns(tid),
            start_ns: acct::start_ns(tid),
            sig_pending: t.sig_pending,
            sig_blocked: t.sig_blocked,
        })
    })
}

/// The process thread `tid` belongs to.
pub fn pid_of(tid: usize) -> Option<usize> {
    snapshot(|tasks| (tid < MAX_TASKS && live(&tasks[tid])).then(|| tasks[tid].tgid))
}
