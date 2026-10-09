//! Round-robin kernel threads plus user processes (own CR3/TTBR0).
//! Cooperative (`yield_now`) and preemptive (timer IRQ calls the same
//! `schedule` after EOI).
//!
//! A user process is one or more threads. Each thread is a task slot; the
//! slot of the first one, the thread-group leader, is also the process: its
//! id is the pid, and it holds what the threads share (address space, fds,
//! cwd, signal dispositions, job control, exit status). See `thread`.

mod fd;
pub mod fpu;
pub mod info;
mod acct;
mod jobs;
mod lifecycle;
pub mod ns;
mod process;
mod sched;
mod signals;
mod thread;
pub mod tp;
mod vm;
pub use acct::idle_ns;
pub use fd::*;
pub use jobs::*;
pub use lifecycle::*;
use process::*;
pub use sched::*;
pub use signals::*;
pub use thread::*;
pub use vm::*;

use alloc::alloc::{alloc, Layout};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use crate::console;
use crate::pipe;
use crate::user;

use crate::arch::switch::{seed_stack, task_switch};

pub const MAX_TASKS: usize = 64;
/// Exec from a syscall runs `load_user_elf` / `copy_user_aspace` on the task
/// stack (exception frame + `[MAX_INIT_PAGES]`/`[USER_STACK_PAGES]` frame arrays).
/// 8 KiB overflowed after widening the user stack to 64 KiB; 16 KiB then overflowed
/// once `MAX_INIT_PAGES` grew to 1024 for ripgrep (`[u64; 1024]` is 8 KiB alone,
/// plus stack frames and a nested timer IRQ). Overflow hangs with no serial.
pub const STACK_SIZE: usize = 64 * 1024;
/// oksh `FDBASE` is 10 (`fcntl(F_DUPFD)` for tty/script fds). 8 was enough for
/// the tiny Rust shell; raise further for dropbear: a session holds stdio +
/// the session socket + the signal pipe (2) + three pipes for `spawn_command`
/// (6) before the child execs, so 16 was exhausted and exec failed. 64 for
/// larger programs; libgloss tracks per-fd flags up to `MYOS_MAX_FDS`.
pub const MAX_FDS: usize = 64;
// `Process::cloexec` keeps a bit per fd.
const _: () = assert!(MAX_FDS <= 64);


#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Unused,
    Ready,
    Running,
    /// Waiting for an event (see `sched::block_until`): not runnable until a
    /// `wake` matching its `wait_key`, its `wake_at` deadline, or a signal.
    Blocked,
    Dead,
    /// Taken by `claim_slot` for the task its caller installs next (a fork,
    /// a new thread): nothing else may take the slot, or its kernel stack,
    /// in between.
    Claimed,
}

/// The user registers a new task starts with: a forked child resumes after
/// the parent's syscall with the parent's registers (result 0); a new thread
/// at its entry or, for a Linux `clone`, like a forked child on its own
/// stack (built by `user::caller_regs`). Defined per arch.
pub use crate::arch::UserRegs;

/// Mappings a process can hold (Linux's default is 65530). Each shared
/// object takes a region per segment, and an allocator such as rustc's
/// (Scudo) maps and re-protects its size classes piecemeal: compiling
/// `core` needs more than 256.
pub const MAX_MMAP_REGIONS: usize = 4096;

/// Distinct files a process can have mapped at once (a file mapping past
/// this is copied in whole at `mmap` time instead of paged in).
pub const MAX_MAPPED_FILES: usize = 64;

/// A run of mmap pages with one protection and one backing. Its pages get
/// frames on first touch (`user::fault_in`): zeroed, or read from the file.
#[derive(Clone, Copy)]
pub struct MmapRegion {
    pub va: u64,
    pub pages: u32,
    /// `PROT_*` bits, plus [`MMAP_DEVICE`] or [`MMAP_SHARED`].
    pub prot: u32,
    /// 0: anonymous; else the process's `mapped_files[file - 1]`.
    pub file: u32,
    /// The file offset of the first page, in pages.
    pub fpage: u32,
}

/// [`MmapRegion::prot`] flag: the pages are a device's (a module's `mmap`
/// hook, `/dev/fb/data`), mapped shared. Unmapping (munmap, exec, exit)
/// leaves them to the device instead of freeing them, and fork maps the
/// same pages into the child instead of copying them.
pub const MMAP_DEVICE: u32 = 1 << 31;

/// [`MmapRegion::prot`] flag: a shared mapping of a regular file
/// (`MAP_SHARED` through a writable fd): its pages are the page cache's
/// frames for the file's, written (`fs::pagecache::map_shared`) and
/// written back to the file when the mapping goes, or by `msync`.
pub const MMAP_SHARED: u32 = 1 << 30;


/// The scheduler's record of one thread (see [`Process`] for what the
/// threads of a process share). Small and `Copy`: it is written whole into
/// its slot on spawn and reset with [`EMPTY`] on reap.
#[derive(Clone, Copy)]
struct Task {
    state: State,
    #[allow(dead_code)]
    stack_base: usize,
    sp: usize,
    entry: Option<fn()>,
    aspace: u64,
    kernel_stack_top: usize,
    user_rip: usize,
    user_rsp: usize,
    ppid: usize,
    start_regs: Option<UserRegs>,
    exit_code: u8,
    /// Signal that killed the task (0 = it exited normally).
    term_sig: u8,
    /// Set by `die()` as soon as the task has exited (status final, parent
    /// notified), while it still frees its address space; `wait_child` may
    /// report it from then on. The slot is recycled only once it is Dead and
    /// off its kernel stack (`reapable`).
    exited: bool,
    /// Pending signals bitmask (bit N = signal N, N in 1..31). See `signal`.
    sig_pending: u32,
    /// Blocked signal mask (SYS_SIGPROCMASK, `SIG_BLOCK`/`SIG_SETMASK`).
    /// Blocked does not mean discarded: still accumulates in `sig_pending`,
    /// delivered when unblocked.
    sig_blocked: u32,
    /// `None` = runnable on any CPU (the kernel's threads); `Some(cpu)` =
    /// its home, the CPU it runs on: pinned for an idle task, for a user
    /// task until an idle CPU takes it (`sched::pull`).
    affinity: Option<usize>,
    /// What a `Blocked` task waits for (`sched::WAIT_ANY` = any event).
    wait_key: usize,
    /// Monotonic-ns deadline of a `Blocked` task, 0 = none.
    wake_at: u64,
    /// The process's `ITIMER_REAL` (in the leader's slot): when its next
    /// `SIGALRM` is due (monotonic ns, 0 = disarmed) and the interval it
    /// re-arms with (0 = once). A fork starts disarmed; exec keeps it.
    alarm_at: u64,
    alarm_every: u64,
    /// A wake arrived while the task was still leaving a CPU (mid task
    /// switch); `finish_switch` turns it into `Ready`.
    wake_pending: bool,
    /// The process (thread-group leader slot) this thread belongs to; its
    /// own slot for a single-threaded process. The process-wide state (the
    /// [`Process`] block: address-space layout, fds, cwd, job control,
    /// `sig_ignored`) and the exit status are only kept in the leader's slot.
    tgid: usize,
    /// Set in the leader once the process is ending (`exit_group`): its
    /// other threads are being killed, and the exit status is final.
    group_exit: bool,
    /// The trap frame of the syscall the task is in while it is off its CPU
    /// (`arch::syscall_frame`, which `schedule` saves and restores): a
    /// syscall that blocks resumes with its own frame wherever it runs.
    syscall_frame: usize,
    /// What `/proc` calls it (`docs/proc.md`): the program it last exec'd,
    /// what its creator was called, or a kernel thread's name; NUL-padded.
    name: [u8; NAME_MAX],
}

const EMPTY: Task = Task {
    state: State::Unused,
    stack_base: 0,
    sp: 0,
    entry: None,
    aspace: 0,
    kernel_stack_top: 0,
    user_rip: 0,
    user_rsp: 0,
    ppid: 0,
    start_regs: None,
    exit_code: 0,
    term_sig: 0,
    exited: false,
    sig_pending: 0,
    sig_blocked: 0,
    affinity: None,
    wait_key: 0,
    wake_at: 0,
    alarm_at: 0,
    alarm_every: 0,
    wake_pending: false,
    tgid: 0,
    group_exit: false,
    syscall_frame: 0,
    name: [0; NAME_MAX],
};

/// The longest task name (Linux's `TASK_COMM_LEN` less its NUL).
pub const NAME_MAX: usize = 15;

/// Name `t` (cut to [`NAME_MAX`] bytes).
fn set_name(t: &mut Task, name: &[u8]) {
    let n = name.len().min(NAME_MAX);
    t.name = [0; NAME_MAX];
    t.name[..n].copy_from_slice(&name[..n]);
}

static TASKS: Mutex<TaskTable> = Mutex::new(TaskTable::new());

/// Current task slot per CPU. `usize::MAX` until the CPU's first task is
/// installed (`init` / `ap_idle_bringup`), so `slot_on_cpu` never mistakes
/// task 0 for "running on an offline CPU".
static CURRENT: [AtomicUsize; crate::smp::MAX_CPUS] =
    [const { AtomicUsize::new(usize::MAX) }; crate::smp::MAX_CPUS];
static PREEMPT_ON: AtomicBool = AtomicBool::new(false);
static SERIAL: Mutex<()> = Mutex::new(());
static KERNEL_ASPACE: AtomicU64 = AtomicU64::new(0);
static LOADED_ASPACE: [AtomicU64; crate::smp::MAX_CPUS] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

fn loaded_aspace() -> u64 {
    let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
    LOADED_ASPACE[cpu].load(Ordering::SeqCst)
}

fn set_loaded_aspace(a: u64) {
    let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
    LOADED_ASPACE[cpu].store(a, Ordering::SeqCst);
}

/// Has a CPU other than this one `aspace` loaded (a thread of its process
/// runs there, or ran there last)? Its TLB may then hold translations of it.
pub fn aspace_loaded_elsewhere(aspace: u64) -> bool {
    let me = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
    (0..crate::smp::MAX_CPUS).any(|i| i != me && LOADED_ASPACE[i].load(Ordering::SeqCst) == aspace)
}

/// Some CPU, this one included, has `aspace` loaded.
pub fn aspace_loaded_anywhere(aspace: u64) -> bool {
    LOADED_ASPACE.iter().any(|a| a.load(Ordering::SeqCst) == aspace)
}

fn current_slot() -> usize {
    let cpu = crate::smp::cpu_id();
    CURRENT[cpu.min(crate::smp::MAX_CPUS - 1)].load(Ordering::SeqCst)
}

fn set_current_slot(slot: usize) {
    let cpu = crate::smp::cpu_id();
    CURRENT[cpu.min(crate::smp::MAX_CPUS - 1)].store(slot, Ordering::SeqCst);
}

pub fn init() {
    let a = user::read_aspace();
    KERNEL_ASPACE.store(a, Ordering::SeqCst);
    set_loaded_aspace(a);
    let flags = irq_save();
    irq_off();
    let mut tasks = TASKS.lock();
    tasks[0].state = State::Running;
    tasks[0].sp = 0;
    // BSP keeps kernel_main; user/kernel workers may run anywhere.
    tasks[0].affinity = Some(0);
    set_current_slot(0);
    drop(tasks);
    irq_restore(flags);
}

/// x86: round-robin home CPU across APs when SMP is online. Skip CPU 0 —
/// BSP shares irq/console/kernel_main with user under RR and CI bios hung
/// after histrecall (`80c6ff1`, `-smp 2`). With `-smp 4` (≥2 APs):
/// `spawn_user` RR-assigns top-level tasks; fork inherits unless the parent
/// has a controlling tty **and** already has an active child (interactive
/// `make -j` / pipelines) — then the new child takes a fresh AP RR home.
/// The ctty gate matters: init forks long-lived `netd` then `getty`; without
/// it, `parent_has_active_child` treated netd as parallel work and RR-spread
/// getty onto a remote AP (UEFI user #PF cr2=kernel after fork exec; bios
/// burned the 600s wall mid interactive). Exec does **not** re-home:
/// sequential shell/smoke fork+exec+wait stays same-CPU (blanket post-exec
/// RR made UEFI CI burn the 600s QEMU wall). Cross-CPU wait (pipelines /
/// make) stays safe via `die` IF-on reclaim + soft TLB service in
/// `schedule`. A Ready task waiting behind a busy home is taken by an idle
/// CPU, which becomes its home (`sched::pull`); a task runs on one CPU at
/// a time either way, and only a CPU it last ran on lists its address
/// space in `LOADED_ASPACE`.

pub fn kernel_aspace() -> u64 {
    KERNEL_ASPACE.load(Ordering::SeqCst)
}

/// Drop `aspace` from every CPU that has it loaded, then TLB-shootdown before
/// reclaim frees frames.
pub fn unload_user_aspace(aspace: u64) {
    if aspace == 0 {
        return;
    }
    let k = KERNEL_ASPACE.load(Ordering::SeqCst);
    // Local first.
    if loaded_aspace() == aspace {
        user::switch_aspace(k);
        set_loaded_aspace(k);
    }
    // Remotes: if they still list this root, nudge a reschedule so `schedule`
    // drops the CR3 (aspace switch happens before Ready). Cap kicks — blasting
    // 100k IPIs livelocked the peer under CI bios timing (shell stuck after
    // histrecall with RR home CPUs).
    if crate::smp::online_count() > 1 {
        let mut kicks = 0u32;
        let mut live = false;
        for spin in 0..50_000u32 {
            live = false;
            for i in 0..crate::smp::MAX_CPUS {
                if LOADED_ASPACE[i].load(Ordering::SeqCst) == aspace {
                    live = true;
                    break;
                }
            }
            if !live {
                break;
            }
            // Soft-ACK while waiting so a peer IF-off in schedule still
            // progresses a shootdown if we must issue one below.
            crate::smp::tlb_service();
            if kicks < 8 && (spin == 0 || spin % 64 == 0) {
                crate::smp::kick_cpus();
                kicks += 1;
            }
            core::hint::spin_loop();
        }
        // A CPU drops the root from its TLB when it switches away from it
        // (`schedule` loads another root first, which flushes), and lists
        // it in LOADED_ASPACE only while it has it: once no CPU lists it,
        // no remote TLB holds it (the local switch above flushed us), on
        // every arch, a task pulled to another CPU included. Skip the
        // global IPI barrier then — it dominated exit/reclaim cost under
        // -smp 4 TCG; still shoot down if a remote refused to drop the root.
        // This gate is load-bearing: a global shootdown per exit dominated
        // exit cost (observed: 1 shootdown/s and a ~10× interactive crawl
        // under -smp 4). die() now reclaims before the task is marked Dead,
        // so a preempted reclaim resumes instead of being abandoned.
        if live {
            crate::smp::tlb_shootdown();
        }
    }
}

#[allow(dead_code)]
pub fn current_id() -> usize {
    current_slot()
}

/// Parent task slot of the running task (`getppid`).
pub fn current_ppid() -> usize {
    let flags = irq_save();
    irq_off();
    let tasks = TASKS.lock();
    let p = tasks[tasks[current_slot()].tgid].ppid;
    drop(tasks);
    irq_restore(flags);
    p
}

/// When the running task is a user thread, its saved PC and stack pointer.
pub fn current_user_pc_sp() -> Option<(usize, usize)> {
    with_thread_mut(|t| {
        if t.user_rip != 0 {
            Some((t.user_rip, t.user_rsp))
        } else {
            None
        }
    })
}

pub fn current_aspace() -> u64 {
    let flags = irq_save();
    irq_off();
    let id = current_slot();
    let a = TASKS.lock()[id].aspace;
    irq_restore(flags);
    a
}

/// Kernel stack top for the running task (riscv64 sscratch / x86 rsp0).
#[allow(dead_code)]
pub fn current_kernel_stack_top() -> usize {
    let flags = irq_save();
    irq_off();
    let id = current_slot();
    let t = TASKS.lock()[id].kernel_stack_top;
    irq_restore(flags);
    t
}

/// The word above a kernel stack's lowest one (riscv64 keeps its CPU footer
/// in the first), which an overflow overwrites before it leaves the stack.
/// `schedule` checks it on the task leaving a CPU and panics naming that
/// task, instead of letting the overflow corrupt whatever the heap placed
/// below the stack: another task's saved context, say, which fails much
/// later with a jump to 0 in a task that did nothing wrong.
const STACK_CANARY: usize = 0x6b73_7461_636b_2121; // "kstack!!"

pub(crate) fn arm_stack(top: usize) {
    unsafe { ((top - STACK_SIZE + 8) as *mut usize).write_volatile(STACK_CANARY) }
}

/// True for a stack whose canary is in place (and for no stack at all: the
/// boot CPU's first task runs on Limine's).
pub(crate) fn stack_intact(top: usize) -> bool {
    top == 0 || unsafe { ((top - STACK_SIZE + 8) as *const usize).read_volatile() } == STACK_CANARY
}

/// For a fault report: whether the current task's kernel stack canary is in
/// place; `None` when the scheduler lock is held (the fault may be under it).
pub fn current_stack_intact() -> Option<bool> {
    let id = current_slot();
    let top = TASKS.try_lock()?[id].kernel_stack_top;
    Some(stack_intact(top))
}

/// The current process is past the point of no return of an exec.
pub fn mark_execd() {
    with_process_mut(|t| t.execd = true);
}

pub fn set_exec_name(name: &[u8]) {
    with_process_mut(|t| {
        let n = name.len().min(t.exec_name.len());
        t.exec_name[..n].copy_from_slice(&name[..n]);
        t.exec_name_len = n as u8;
    });
}

/// Name the calling thread in `/proc` (exec: the program's basename).
pub fn set_current_name(name: &[u8]) {
    with_thread_mut(|t| set_name(t, name));
}

/// The program the current process runs from now on (`/proc/self/exe`):
/// the real path of the file exec loaded.
pub fn set_exe(path: &str) {
    with_process_mut(|t| t.exe = alloc::string::String::from(path));
}

/// The real path of the program the current process runs (`None` before
/// an exec, and for a kernel thread).
pub fn exe_path() -> Option<alloc::string::String> {
    with_process_opt(|p| p.map(|p| p.exe.clone()).filter(|e| !e.is_empty()))
}

pub fn exec_name(out: &mut [u8]) -> usize {
    with_process_mut(|t| {
        let n = t.exec_name_len as usize;
        let n = n.min(out.len()).min(t.exec_name.len());
        out[..n].copy_from_slice(&t.exec_name[..n]);
        n
    })
}

/// The cwd, in the process's view of the tree: where its directory is now,
/// as its namespace names it. 0 when that directory is gone (or the
/// namespace no longer names it): relative paths then lead nowhere.
pub fn cwd(out: &mut [u8]) -> usize {
    let (node, n) = with_process_mut(|t| {
        let n = (t.cwd_len as usize).min(out.len()).min(t.cwd.len());
        out[..n].copy_from_slice(&t.cwd[..n]);
        (t.cwd_node.clone(), n)
    });
    let Some(node) = node else {
        return n;
    };
    let Some(real) = crate::fs::vfs::node_path(&node) else {
        return 0;
    };
    let virt = with_ns(|ns| match ns {
        None => Some(real),
        Some(ns) => ns.to_virtual(&real),
    });
    match virt {
        Some(v) if v.len() <= out.len() => {
            out[..v.len()].copy_from_slice(v.as_bytes());
            v.len()
        }
        _ => 0,
    }
}

/// True when the current process has a namespace (sees part of the tree).
pub fn has_ns() -> bool {
    with_process_opt(|p| p.is_some_and(|p| p.ns.is_some()))
}

/// Run `f` on the current process's namespace (`None`: the whole tree).
pub fn with_ns<R>(f: impl FnOnce(Option<&ns::Namespace>) -> R) -> R {
    with_process_opt(|p| f(p.and_then(|p| p.ns.as_deref())))
}

/// Give the current process the namespace `n` (`None`: the whole tree).
pub fn set_ns(n: Option<ns::Namespace>) {
    let n = n.map(alloc::boxed::Box::new);
    with_process_opt(|p| {
        if let Some(p) = p {
            p.ns = n;
        }
    });
}

/// What the current process's namespace lets it do to `real` (everything
/// without one, or outside a process).
pub fn ns_rights(real: &str) -> crate::sec::Rights {
    with_ns(|n| n.map_or(crate::sec::Rights::ALL, |n| n.rights(real)))
}

/// The current process's user and domain (`None` for the kernel's own
/// threads: no checks).
pub fn sec_ctx() -> Option<crate::sec::Ctx> {
    with_process_opt(|p| p.map(|p| p.ctx))
}

/// Run the current process as `ctx` (all its threads).
pub fn set_sec_ctx(ctx: crate::sec::Ctx) {
    with_process_opt(|p| {
        if let Some(p) = p {
            p.ctx = ctx;
        }
    });
}

/// The user and domain process `pid` runs as.
pub fn sec_ctx_of(pid: usize) -> Option<crate::sec::Ctx> {
    let flags = irq_save();
    irq_off();
    let tasks = TASKS.lock();
    let ctx = tasks.proc_opt(pid).map(|p| p.ctx);
    drop(tasks);
    irq_restore(flags);
    ctx
}

/// Re-map every process's user and domain (a new policy, `crate::sec::load`).
pub fn remap_sec_ctx(mut f: impl FnMut(crate::sec::Ctx) -> crate::sec::Ctx) {
    let flags = irq_save();
    irq_off();
    let mut tasks = TASKS.lock();
    for pid in 0..MAX_TASKS {
        if let Some(p) = tasks.proc_opt_mut(pid) {
            p.ctx = f(p.ctx);
        }
    }
    drop(tasks);
    irq_restore(flags);
}

/// Set the cwd: the directory `node`, which it follows wherever it moves,
/// or, for a directory a namespace makes up (no node), the canonical
/// absolute `path` in the process's view (`/` or `/…`).
pub fn set_cwd(path: &[u8], node: Option<crate::fs::Vnode>) -> bool {
    if path.is_empty() || path[0] != b'/' || path.len() > 256 {
        return false;
    }
    let old = with_process_mut(|t| {
        t.cwd = [0; 256];
        t.cwd[..path.len()].copy_from_slice(path);
        t.cwd_len = path.len() as u16;
        core::mem::replace(&mut t.cwd_node, node)
    });
    drop(old);
    true
}

pub fn save_user_context(rip: usize, rsp: usize) {
    with_thread_mut(|t| {
        t.user_rip = rip;
        t.user_rsp = rsp;
    });
}

/// Run `f` on the current process block: the state the running thread
/// shares with the other threads of its process (see [`Process`]).
/// Run `f` on the current process, if the running thread belongs to one
/// (`None` for the kernel's own threads).
fn with_process_opt<R>(f: impl FnOnce(Option<&mut Process>) -> R) -> R {
    let flags = irq_save();
    irq_off();
    let mut tasks = TASKS.lock();
    let pid = tasks[current_slot()].tgid;
    let out = f(tasks.proc_opt_mut(pid));
    drop(tasks);
    irq_restore(flags);
    out
}

fn with_process_mut<R>(f: impl FnOnce(&mut Process) -> R) -> R {
    // Timer schedule also takes TASKS; nesting that while IF=1 deadlocks the
    // same CPU (seen as a hang after fork child's open/read).
    let flags = irq_save();
    irq_off();
    let mut tasks = TASKS.lock();
    let pid = tasks[current_slot()].tgid;
    let out = f(tasks.proc_mut(pid));
    drop(tasks);
    irq_restore(flags);
    out
}

/// Run `f` on the current process's leader slot: the `Task` that carries
/// the process's exit status.
fn with_leader_mut<R>(f: impl FnOnce(&mut Task) -> R) -> R {
    let flags = irq_save();
    irq_off();
    let mut tasks = TASKS.lock();
    let pid = tasks[current_slot()].tgid;
    let out = f(&mut tasks[pid]);
    drop(tasks);
    irq_restore(flags);
    out
}

/// Run `f` on the running thread's own slot (its registers and stacks).
fn with_thread_mut<R>(f: impl FnOnce(&mut Task) -> R) -> R {
    let flags = irq_save();
    irq_off();
    let mut tasks = TASKS.lock();
    let out = f(&mut tasks[current_slot()]);
    drop(tasks);
    irq_restore(flags);
    out
}

/// The current process id: the slot of the running thread's leader.
pub fn current_pid() -> usize {
    let flags = irq_save();
    irq_off();
    let tasks = TASKS.lock();
    let pid = tasks[current_slot()].tgid;
    drop(tasks);
    irq_restore(flags);
    pid
}

/// Slot count for iterating tasks (signals, process groups).
pub fn task_slots() -> usize {
    MAX_TASKS
}

/// Live user process: has a user image, is not Unused/Dead and has not
/// exited (a reaped child can still be freeing its address space in `die`).
pub fn is_live_user(id: usize) -> bool {
    if id >= MAX_TASKS {
        return false;
    }
    let flags = irq_save();
    irq_off();
    let ok = {
        let tasks = TASKS.lock();
        let t = &tasks[id];
        t.user_rip != 0 && !t.exited && matches!(t.state, State::Ready | State::Running | State::Blocked)
    };
    irq_restore(flags);
    ok
}

pub fn print(s: &str) {
    let flags = irq_save();
    irq_off();
    {
        let _hold = SERIAL.lock();
        console::write_str(s);
    }
    irq_restore(flags);
}

pub fn print_bytes(bytes: &[u8]) {
    match core::str::from_utf8(bytes) {
        Ok(s) => print(s),
        Err(_) => {
            let flags = irq_save();
            irq_off();
            {
                let _hold = SERIAL.lock();
                for &b in bytes {
                    console::write_byte(b);
                }
            }
            irq_restore(flags);
        }
    }
}
