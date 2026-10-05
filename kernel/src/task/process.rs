//! The process block: what the threads of a process share, kept on the heap
//! and reached through the leader's slot. The scheduler's own record of a
//! thread is the (small, `Copy`) [`Task`]; this is the large, per-process
//! part — the fd table, the address-space layout, cwd and namespace, mmap
//! regions, job control, signal dispositions and the security context —
//! allocated when a process starts and freed when its slot is recycled.

use alloc::boxed::Box;
use alloc::vec::Vec;

use super::*;

/// Per-process state (see the module doc). Only the leader slot of a
/// process has one; its threads reach it through `Task::tgid`.
pub(super) struct Process {
    pub fds: [FdEntry; MAX_FDS],
    pub user_base: u64,
    pub image_span: usize,
    pub stack_off: u64,
    pub user_argc: usize,
    pub user_argv: usize,
    /// Current program break (end of heap).
    pub brk_cur: u64,
    /// Basename from the last successful exec (multicall argv[0] fallback).
    pub exec_name: [u8; 32],
    pub exec_name_len: u8,
    /// Absolute cwd (POSIX). Survives exec; copied on fork. Always starts with `/`.
    pub cwd: [u8; 256],
    pub cwd_len: u16,
    /// The mmap regions (in the window after the brk heap), sorted by
    /// address, none empty; at most [`MAX_MMAP_REGIONS`].
    pub mmap: Vec<MmapRegion>,
    /// The files the regions page in from; an entry no region names is free
    /// (it lets its file go when it is reused, or at exec and exit).
    pub mapped_files: [Option<crate::fs::Vnode>; MAX_MAPPED_FILES],
    /// Session id (slot of the session leader). Inherited on fork. New
    /// spawns start as their own session (`sid == slot`); `setsid` creates a
    /// fresh session for a forked child.
    pub sid: usize,
    /// Process group id (slot of the group leader). Inherited on fork. New
    /// spawns start in their own group (`pgid == slot`); `setsid` also puts
    /// the caller in a new group (`pgid = pid`).
    pub pgid: usize,
    /// Controlling terminal attached (phase-1: system console only).
    /// Inherited on fork; set by TIOCSCTTY; cleared by SYS_SETSID.
    pub has_ctty: bool,
    /// Ignored signals bitmask (SIGKILL cannot be ignored). See `signal`.
    pub sig_ignored: u32,
    /// What the process can name (`None`: the whole tree). Inherited on
    /// fork, kept across exec; `cwd` is a path in it.
    pub ns: Option<Box<super::ns::Namespace>>,
    /// The user and domain it runs as (`crate::sec`). Inherited on fork;
    /// exec and `setuser` change it.
    pub ctx: crate::sec::Ctx,
}

const fn root_cwd_buf() -> [u8; 256] {
    let mut c = [0u8; 256];
    c[0] = b'/';
    c
}

/// A fresh process: no fds, cwd `/`, no chroot, its own session and group
/// (set by the spawner).
static EMPTY_PROC: Process = Process {
    fds: [FdEntry::Empty; MAX_FDS],
    user_base: 0,
    image_span: 0,
    stack_off: 0,
    user_argc: 0,
    user_argv: 0,
    brk_cur: 0,
    exec_name: [0; 32],
    exec_name_len: 0,
    cwd: root_cwd_buf(),
    cwd_len: 1,
    mmap: Vec::new(),
    mapped_files: [const { None }; MAX_MAPPED_FILES],
    sid: 0,
    pgid: 0,
    has_ctty: false,
    sig_ignored: 0,
    ns: None,
    ctx: crate::sec::Ctx::BOOT,
};

/// A new, empty process block (heap; never staged on the kernel stack: a
/// `Process` is several KiB). The bitwise copy of [`EMPTY_PROC`] is sound:
/// its owning fields, the empty `mmap`, `ns` and `mapped_files`, own nothing.
pub(super) fn new_process() -> Box<Process> {
    let mut b = Box::<Process>::new_uninit();
    unsafe {
        core::ptr::copy_nonoverlapping(&EMPTY_PROC, b.as_mut_ptr(), 1);
        b.assume_init()
    }
}

/// A forked child's process block: a copy of `src` with the fds re-opened
/// (shared file positions and pipes), the exec name cleared.
pub(super) fn fork_process(src: &Process) -> Box<Process> {
    let mut b = Box::<Process>::new_uninit();
    let mut b = unsafe {
        let p = b.as_mut_ptr();
        core::ptr::copy_nonoverlapping(src, p, 1);
        // The bitwise copy shares `src`'s region list, namespace and mapped
        // files: give the child its own.
        core::ptr::write(&raw mut (*p).mmap, src.mmap.clone());
        core::ptr::write(&raw mut (*p).ns, src.ns.clone());
        core::ptr::write(&raw mut (*p).mapped_files, src.mapped_files.clone());
        b.assume_init()
    };
    for fd in b.fds.iter_mut() {
        *fd = fd_clone(*fd);
    }
    b.exec_name = [0; 32];
    b.exec_name_len = 0;
    b
}

/// The task table under the `TASKS` lock: every slot's scheduler record and
/// the process block of the slots that lead a process.
pub(super) struct TaskTable {
    tasks: [Task; MAX_TASKS],
    procs: [Option<Box<Process>>; MAX_TASKS],
}

impl TaskTable {
    pub const fn new() -> Self {
        Self {
            tasks: [EMPTY; MAX_TASKS],
            procs: [const { None }; MAX_TASKS],
        }
    }

    pub fn iter(&self) -> core::slice::Iter<'_, Task> {
        self.tasks.iter()
    }

    pub fn iter_mut(&mut self) -> core::slice::IterMut<'_, Task> {
        self.tasks.iter_mut()
    }

    /// The process block of leader slot `pid` (a user process).
    pub fn proc(&self, pid: usize) -> &Process {
        match self.procs[pid].as_deref() {
            Some(p) => p,
            None => self.no_proc(pid),
        }
    }

    /// The process block of slot `pid`, if it leads a process.
    pub fn proc_opt_mut(&mut self, pid: usize) -> Option<&mut Process> {
        self.procs.get_mut(pid).and_then(|p| p.as_deref_mut())
    }

    pub fn proc_mut(&mut self, pid: usize) -> &mut Process {
        if self.procs[pid].is_none() {
            self.no_proc(pid);
        }
        self.procs[pid].as_deref_mut().unwrap()
    }

    /// A slot that leads no process was asked for its block: a kernel
    /// thread, an idle task and a non-leader thread have none, a recycled
    /// slot lost its; say which and who was running.
    fn no_proc(&self, pid: usize) -> ! {
        let t = &self.tasks[pid];
        panic!(
            "task slot {pid} has no process block (state {} user_rip {:#x} tgid {} ppid {}; current slot {} on cpu {})",
            t.state as u8,
            t.user_rip,
            t.tgid,
            t.ppid,
            super::current_slot(),
            crate::smp::cpu_id(),
        )
    }

    /// The process block of slot `pid`, if it leads a process (kernel
    /// threads, idle tasks and non-leader threads have none).
    pub fn proc_opt(&self, pid: usize) -> Option<&Process> {
        self.procs.get(pid).and_then(|p| p.as_deref())
    }

    /// Give slot `slot` the process block `p` (it becomes a process leader).
    pub fn install_proc(&mut self, slot: usize, p: Box<Process>) {
        self.procs[slot] = Some(p);
    }

    /// Ignored-signal mask of the process led by `pid` (0 for a slot that
    /// leads none).
    pub fn sig_ignored(&self, pid: usize) -> u32 {
        self.proc_opt(pid).map_or(0, |p| p.sig_ignored)
    }

    /// Free slot `slot` for reuse: back to [`EMPTY`] (keeping its kernel
    /// stack, see `claim_slot`) and without a process block.
    pub fn recycle(&mut self, slot: usize) {
        let stack_base = self.tasks[slot].stack_base;
        self.tasks[slot] = EMPTY;
        self.tasks[slot].stack_base = stack_base;
        self.procs[slot] = None;
    }
}

impl core::ops::Index<usize> for TaskTable {
    type Output = Task;
    fn index(&self, i: usize) -> &Task {
        &self.tasks[i]
    }
}

impl core::ops::IndexMut<usize> for TaskTable {
    fn index_mut(&mut self, i: usize) -> &mut Task {
        &mut self.tasks[i]
    }
}
