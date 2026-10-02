//! Round-robin kernel threads plus user processes (own CR3/TTBR0).
//! Cooperative (`yield_now`) and preemptive (timer IRQ calls the same
//! `schedule` after EOI).

mod fd;
pub mod fpu;
mod jobs;
mod lifecycle;
mod sched;
mod signals;
mod vm;
pub use fd::*;
pub use jobs::*;
pub use lifecycle::*;
pub use sched::*;
pub use signals::*;
pub use vm::*;

use alloc::alloc::{alloc, Layout};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use crate::console;
use crate::pipe;
use crate::user;

#[cfg(target_arch = "x86_64")]
mod switch_x86;
#[cfg(target_arch = "x86_64")]
use switch_x86::{seed_stack, task_switch};

#[cfg(target_arch = "aarch64")]
mod switch_aarch64;
#[cfg(target_arch = "aarch64")]
use switch_aarch64::{seed_stack, task_switch};

#[cfg(target_arch = "riscv64")]
mod switch_riscv64;
#[cfg(target_arch = "riscv64")]
use switch_riscv64::{seed_stack, task_switch};

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
const MAX_FDS: usize = 64;

/// Stamp the owning logical CPU id at the base of a kernel stack so U-mode
/// trap entry can reload `tp` without trusting user TLS (see riscv64 trap
/// vector). Word 0 of the stack allocation is reserved for this footer.
#[cfg(target_arch = "riscv64")]
pub fn stamp_stack_cpu(kstack_top: usize, cpu: usize) {
    if kstack_top < STACK_SIZE {
        return;
    }
    let cpu = cpu.min(crate::smp::MAX_CPUS - 1);
    unsafe {
        ((kstack_top - STACK_SIZE) as *mut usize).write(cpu);
    }
}

#[cfg(not(target_arch = "riscv64"))]
pub fn stamp_stack_cpu(_kstack_top: usize, _cpu: usize) {}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Unused,
    Ready,
    Running,
    /// Waiting for an event (see `sched::block_until`): not runnable until a
    /// `wake` matching its `wait_key`, its `wake_at` deadline, or a signal.
    Blocked,
    Dead,
}

/// User register snapshot so a forked child can resume after the syscall
/// with the same callee-saved state the parent had (rax/x0 forced to 0).
/// `#[repr(C)]` is required: `enter_fork_x86` historically used fixed
/// offsets into this struct; keep a stable layout even if that path changes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ForkRegs {
    pub rip: usize,
    pub rsp: usize,
    #[cfg(target_arch = "x86_64")]
    pub rbx: u64,
    #[cfg(target_arch = "x86_64")]
    pub rbp: u64,
    #[cfg(target_arch = "x86_64")]
    pub r12: u64,
    #[cfg(target_arch = "x86_64")]
    pub r13: u64,
    #[cfg(target_arch = "x86_64")]
    pub r14: u64,
    #[cfg(target_arch = "x86_64")]
    pub r15: u64,
    /// Full `lower_sync` frame (x0..x30, elr, spsr, sp_el0). Index 31 unused.
    #[cfg(target_arch = "aarch64")]
    pub frame: [u64; 36],
    /// Trap frame (x0..x31, sepc, sstatus, user sp). Index 35 unused.
    #[cfg(target_arch = "riscv64")]
    pub frame: [u64; 36],
}

const fn root_cwd_buf() -> [u8; 256] {
    let mut c = [0u8; 256];
    c[0] = b'/';
    c
}

/// Enough for a dynamically linked program: each shared object takes a
/// region per segment.
pub const MAX_MMAP_REGIONS: usize = 64;

#[derive(Clone, Copy)]
pub struct MmapRegion {
    pub va: u64,
    pub pages: u32,
    pub prot: u32,
}

const EMPTY_MMAP: [MmapRegion; MAX_MMAP_REGIONS] = [MmapRegion {
    va: 0,
    pages: 0,
    prot: 0,
}; MAX_MMAP_REGIONS];

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
    fds: [FdEntry; MAX_FDS],
    user_base: u64,
    image_span: usize,
    stack_off: u64,
    ppid: usize,
    fork_regs: Option<ForkRegs>,
    user_argc: usize,
    user_argv: usize,
    /// Current program break (end of heap). 0 for kernel threads.
    brk_cur: u64,
    /// Basename from the last successful exec (multicall argv[0] fallback).
    exec_name: [u8; 32],
    exec_name_len: u8,
    /// Absolute cwd (POSIX). Survives exec; copied on fork. Always starts with `/`.
    cwd: [u8; 256],
    cwd_len: u16,
    exit_code: u8,
    /// Signal that killed the task (0 = it exited normally).
    term_sig: u8,
    /// Set by `die()` as soon as the task has exited (status final, parent
    /// notified), while it still frees its address space; `wait_child` may
    /// report it from then on. The slot is recycled only once it is Dead and
    /// off its kernel stack (`reapable`).
    exited: bool,
    /// Anonymous mmap windows (after the brk heap).
    mmap: [MmapRegion; MAX_MMAP_REGIONS],
    /// Session id (task slot of the session leader). Inherited on fork.
    /// New spawns start as their own session (`sid == slot`); `setsid` creates
    /// a fresh session for a forked child.
    sid: usize,
    /// Process group id (task slot of the group leader). Inherited on fork.
    /// New spawns start in their own group (`pgid == slot`); `setsid` also
    /// puts the caller in a new group (`pgid = pid`).
    pgid: usize,
    /// Controlling terminal attached (phase-1: system console only).
    /// Inherited on fork; set by TIOCSCTTY; cleared by SYS_SETSID.
    has_ctty: bool,
    /// Pending signals bitmask (bit N = signal N, N in 1..31). See `signal`.
    sig_pending: u32,
    /// Ignored signals bitmask (SIGKILL cannot be ignored). See `signal`.
    sig_ignored: u32,
    /// Blocked signal mask (SYS_SIGPROCMASK, `SIG_BLOCK`/`SIG_SETMASK`).
    /// Blocked does not mean discarded: still accumulates in `sig_pending`,
    /// delivered when unblocked.
    sig_blocked: u32,
    /// `None` = runnable on any CPU; `Some(cpu)` = pinned (idle threads).
    affinity: Option<usize>,
    /// What a `Blocked` task waits for (`sched::WAIT_ANY` = any event).
    wait_key: usize,
    /// Monotonic-ns deadline of a `Blocked` task, 0 = none.
    wake_at: u64,
    /// A wake arrived while the task was still leaving a CPU (mid task
    /// switch); `finish_switch` turns it into `Ready`.
    wake_pending: bool,
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
    fds: [FdEntry::Empty; MAX_FDS],
    user_base: 0,
    image_span: 0,
    stack_off: 0,
    ppid: 0,
    fork_regs: None,
    user_argc: 0,
    user_argv: 0,
    brk_cur: 0,
    exec_name: [0; 32],
    exec_name_len: 0,
    cwd: root_cwd_buf(),
    cwd_len: 1,
    exit_code: 0,
    term_sig: 0,
    exited: false,
    mmap: EMPTY_MMAP,
    sid: 0,
    pgid: 0,
    has_ctty: false,
    sig_pending: 0,
    sig_ignored: 0,
    sig_blocked: 0,
    affinity: None,
    wait_key: 0,
    wake_at: 0,
    wake_pending: false,
};

static TASKS: Mutex<[Task; MAX_TASKS]> = Mutex::new([EMPTY; MAX_TASKS]);

/// Longest chroot prefix (real absolute path) a task can carry.
pub const ROOT_CAP: usize = 128;

/// chroot(2) prefix per task slot: absolute real path of the task's `/`, or
/// empty (`len == 0`) for the real root. Inherited on fork, kept across exec;
/// `cwd` is relative to it (the path the task itself sees). Kept out of
/// [`Task`] so the by-value `Task` temporaries (fork builds one on the 64 KiB
/// kernel stack) do not grow.
#[derive(Clone, Copy)]
struct Root {
    buf: [u8; ROOT_CAP],
    len: u8,
}

const NO_ROOT: Root = Root { buf: [0; ROOT_CAP], len: 0 };

static ROOTS: Mutex<[Root; MAX_TASKS]> = Mutex::new([NO_ROOT; MAX_TASKS]);

fn set_slot_root(slot: usize, root: Root) {
    let flags = irq_save();
    irq_off();
    ROOTS.lock()[slot] = root;
    irq_restore(flags);
}

fn slot_root(slot: usize) -> Root {
    let flags = irq_save();
    irq_off();
    let r = ROOTS.lock()[slot];
    irq_restore(flags);
    r
}

static CURRENT: [AtomicUsize; crate::smp::MAX_CPUS] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];
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
/// `schedule`. Live migration (`affinity: None`) remains off. Other arches
/// float.

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
        // x86: affinity pin ⇒ once no CPU lists this root in LOADED_ASPACE,
        // no remote TLB holds it (local CR3 switch above already flushed us).
        // Skip the global IPI barrier — it dominated exit/reclaim cost under
        // -smp 4 TCG. Still shoot down if a remote refused to drop the root
        // (float / bug), and on other arches that may migrate.
        // aarch64: same invariant holds (user_affinity() pins all user tasks
        // to the BSP), so `live` is false here and the barrier is skipped.
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

/// Parent task slot of the running task (Linux layer `getppid`).
#[cfg(feature = "linux-compat")]
pub fn current_ppid() -> usize {
    let flags = irq_save();
    irq_off();
    let p = TASKS.lock()[current_slot()].ppid;
    irq_restore(flags);
    p
}

/// When the running task is a user process, its saved PC and stack pointer.
pub fn current_user_pc_sp() -> Option<(usize, usize)> {
    with_current_mut(|t| {
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
#[cfg(not(target_arch = "aarch64"))]
pub fn current_kernel_stack_top() -> usize {
    let flags = irq_save();
    irq_off();
    let id = current_slot();
    let t = TASKS.lock()[id].kernel_stack_top;
    irq_restore(flags);
    t
}

pub fn set_exec_name(name: &[u8]) {
    with_current_mut(|t| {
        let n = name.len().min(t.exec_name.len());
        t.exec_name[..n].copy_from_slice(&name[..n]);
        t.exec_name_len = n as u8;
    });
}

pub fn exec_name(out: &mut [u8]) -> usize {
    with_current_mut(|t| {
        let n = t.exec_name_len as usize;
        let n = n.min(out.len()).min(t.exec_name.len());
        out[..n].copy_from_slice(&t.exec_name[..n]);
        n
    })
}

pub fn cwd(out: &mut [u8]) -> usize {
    with_current_mut(|t| {
        let n = t.cwd_len as usize;
        let n = n.min(out.len()).min(t.cwd.len());
        out[..n].copy_from_slice(&t.cwd[..n]);
        n
    })
}

/// chroot prefix of the current task (real absolute path); 0 bytes = real `/`.
pub fn root(out: &mut [u8]) -> usize {
    let r = slot_root(current_slot());
    let n = (r.len as usize).min(out.len());
    out[..n].copy_from_slice(&r.buf[..n]);
    n
}

/// True when the current task is chrooted (non-empty prefix).
pub fn has_root() -> bool {
    let flags = irq_save();
    irq_off();
    let jailed = ROOTS.lock()[current_slot()].len != 0;
    irq_restore(flags);
    jailed
}

/// Set the chroot prefix (canonical real absolute path; `/` clears it).
pub fn set_root(path: &[u8]) -> bool {
    if path.is_empty() || path[0] != b'/' || path.len() > ROOT_CAP {
        return false;
    }
    let path = if path == b"/" { &path[..0] } else { path };
    let mut r = NO_ROOT;
    r.buf[..path.len()].copy_from_slice(path);
    r.len = path.len() as u8;
    set_slot_root(current_slot(), r);
    true
}

/// Set absolute cwd. `path` must be a canonical absolute path (`/` or `/…`).
pub fn set_cwd(path: &[u8]) -> bool {
    if path.is_empty() || path[0] != b'/' || path.len() > 256 {
        return false;
    }
    with_current_mut(|t| {
        t.cwd = [0; 256];
        t.cwd[..path.len()].copy_from_slice(path);
        t.cwd_len = path.len() as u16;
    });
    true
}

pub fn save_user_context(rip: usize, rsp: usize) {
    with_current_mut(|t| {
        t.user_rip = rip;
        t.user_rsp = rsp;
    });
}

fn with_current_mut<R>(f: impl FnOnce(&mut Task) -> R) -> R {
    // Timer schedule also takes TASKS; nesting that while IF=1 deadlocks the
    // same CPU (seen as a hang after fork child's open/read).
    let flags = irq_save();
    irq_off();
    let mut tasks = TASKS.lock();
    let id = current_slot();
    let out = f(&mut tasks[id]);
    drop(tasks);
    irq_restore(flags);
    out
}

/// Slot count for iterating tasks (signals, process groups).
pub fn task_slots() -> usize {
    MAX_TASKS
}

/// Live user process: has a user image and is not Unused/Dead.
pub fn is_live_user(id: usize) -> bool {
    if id >= MAX_TASKS {
        return false;
    }
    let flags = irq_save();
    irq_off();
    let ok = {
        let tasks = TASKS.lock();
        let t = &tasks[id];
        t.user_rip != 0 && matches!(t.state, State::Ready | State::Running | State::Blocked)
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
