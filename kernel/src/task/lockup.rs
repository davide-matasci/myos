//! The stuck-CPU report: a CPU that stops scheduling says so on the screen.
//!
//! Every CPU passes through the scheduler at each of its ticks
//! (`smp::note_schedule`), unless it halts idle. One that stops passing
//! while not idle is stuck in the kernel, its interrupts off, or spinning
//! on the task table: the tasks queued on it never run again, the console
//! input thread among them, and the machine looks frozen though CPU 0
//! still blinks the cursor (a freeze seen on real hardware, which nothing
//! else would describe). CPU 0's tick checks the others once a second and
//! prints, once per stall, which CPU and which task:
//!
//! ```text
//! [ WARN ] cpu 3 stuck for 10 s in the kernel: pid 41 wifi (slot 43)
//! ```
//!
//! and how long the stall lasted if the CPU comes back. Where the arch
//! can interrupt a CPU whose interrupts are off (x86's NMI,
//! `arch::sample_cpu`), a line before it says where the CPU is: the
//! address it was at and, in the kernel, the code addresses on its stack
//! (likely its callers; `addr2line -e` the build's kernel turns them into
//! functions):
//!
//! ```text
//! [ WARN ] cpu 3 at 0xffffffff8012a3f0 in the kernel, called from 0xffffffff80131c22 0xffffffff800f0a17
//! ```
//!
//! The report is made in the timer interrupt, so it allocates nothing and
//! takes no lock it could wait on: the stuck CPU may hold any of them, the
//! console module's screen included (`console::status_warn_now` tries the
//! screen through `ModuleConsoleOps::status_line_try` and the serial port
//! only once that took).
//!
//! CPU 0 is watched the other way round ([`bsp_check`]): it counts the
//! ticks, every device's interrupt goes to it on x86 and its tick blinks
//! the cursor, so when it stops the machine is dead at once, cursor,
//! keyboard and ^C with it (the freeze a laptop showed during `get-myos`
//! on a USB stick). Every other CPU's tick looks at CPU 0's tick count:
//! unmoved for 10 s (its idle halts last a second at most), CPU 0 is
//! reported the same way, by the first CPU to see it, and since CPU 0 may
//! hold the console's locks the lines go out whatever is held
//! (`console::status_force`): the last words of a machine, not a report
//! to keep tidy.

use super::*;
use core::fmt::Write;

/// How long a CPU may go without scheduling before it is reported. Far
/// above any wait the kernel makes with interrupts off; a host that stops
/// a TCG vCPU's thread for this long is past what the boot tests allow.
const STUCK_NS: u64 = 10_000_000_000;
/// How often CPU 0 looks.
const CHECK_NS: u64 = 1_000_000_000;

static NEXT_CHECK: AtomicU64 = AtomicU64::new(0);
/// Per CPU: its schedule count at the last check that saw it move.
static SEEN: [AtomicU64; crate::smp::MAX_CPUS] = [const { AtomicU64::new(0) }; crate::smp::MAX_CPUS];
/// Per CPU: when its count last moved (or it was idle).
static SINCE: [AtomicU64; crate::smp::MAX_CPUS] = [const { AtomicU64::new(0) }; crate::smp::MAX_CPUS];
/// Per CPU: the stall was reported (said again when it ends).
static REPORTED: [AtomicBool; crate::smp::MAX_CPUS] = [const { AtomicBool::new(false) }; crate::smp::MAX_CPUS];

/// How many code addresses a sample keeps from the stack.
pub const SAMPLE_CALLS: usize = 8;
/// Per CPU: a sample was asked for (`arch::sample_cpu`); the NMI of any
/// other cause is not one.
static WANTED: [AtomicBool; crate::smp::MAX_CPUS] = [const { AtomicBool::new(false) }; crate::smp::MAX_CPUS];
/// Per CPU: the sample came (`lockup_sampled`), and what it found.
static SAMPLED: [AtomicBool; crate::smp::MAX_CPUS] = [const { AtomicBool::new(false) }; crate::smp::MAX_CPUS];
static SAMPLE_PC: [AtomicU64; crate::smp::MAX_CPUS] = [const { AtomicU64::new(0) }; crate::smp::MAX_CPUS];
static SAMPLE_USER: [AtomicBool; crate::smp::MAX_CPUS] = [const { AtomicBool::new(false) }; crate::smp::MAX_CPUS];
static SAMPLE_CALL: [[AtomicU64; SAMPLE_CALLS]; crate::smp::MAX_CPUS] =
    [const { [const { AtomicU64::new(0) }; SAMPLE_CALLS] }; crate::smp::MAX_CPUS];
/// How long CPU 0 waits for a sample (an NMI arrives in microseconds).
const SAMPLE_WAIT_NS: u64 = 5_000_000;

/// The arch's NMI handler on a stuck CPU: where it was. Atomics only.
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub fn lockup_sampled(cpu: usize, pc: u64, user: bool, calls: &[u64; SAMPLE_CALLS]) {
    if cpu >= crate::smp::MAX_CPUS || !WANTED[cpu].swap(false, Ordering::SeqCst) {
        return;
    }
    SAMPLE_PC[cpu].store(pc, Ordering::Relaxed);
    SAMPLE_USER[cpu].store(user, Ordering::Relaxed);
    for (slot, &c) in SAMPLE_CALL[cpu].iter().zip(calls) {
        slot.store(c, Ordering::Relaxed);
    }
    SAMPLED[cpu].store(true, Ordering::SeqCst);
}

/// Ask a stuck CPU where it is and wait a moment for the answer: the line
/// that says it, or `None` (no way to ask on this arch, or no answer).
fn sample(cpu: usize) -> Option<Line> {
    SAMPLED[cpu].store(false, Ordering::SeqCst);
    WANTED[cpu].store(true, Ordering::SeqCst);
    if !crate::arch::sample_cpu(cpu) {
        WANTED[cpu].store(false, Ordering::SeqCst);
        return None;
    }
    let end = crate::time::monotonic_ns() + SAMPLE_WAIT_NS;
    while !SAMPLED[cpu].load(Ordering::SeqCst) {
        if crate::time::monotonic_ns() > end {
            WANTED[cpu].store(false, Ordering::SeqCst);
            let mut line = Line::new();
            let _ = write!(line, "cpu {cpu} did not answer the NMI");
            return Some(line);
        }
        core::hint::spin_loop();
    }
    let mut line = Line::new();
    let pc = SAMPLE_PC[cpu].load(Ordering::Relaxed);
    if SAMPLE_USER[cpu].load(Ordering::Relaxed) {
        let _ = write!(line, "cpu {cpu} at {pc:#x} in user mode");
        return Some(line);
    }
    let _ = write!(line, "cpu {cpu} at {pc:#x} in the kernel, called from");
    for c in &SAMPLE_CALL[cpu] {
        let c = c.load(Ordering::Relaxed);
        if c != 0 {
            let _ = write!(line, " {c:#x}");
        }
    }
    Some(line)
}

/// From CPU 0's timer interrupt (`time::note_tick`).
pub fn lockup_check(now: u64) {
    if now < NEXT_CHECK.load(Ordering::Relaxed) || !PREEMPT_ON.load(Ordering::SeqCst) {
        return;
    }
    NEXT_CHECK.store(now + CHECK_NS, Ordering::Relaxed);
    for cpu in 1..crate::smp::MAX_CPUS {
        if !crate::smp::cpu_online(cpu) {
            continue;
        }
        let count = crate::smp::sched_ticks(cpu);
        if count != SEEN[cpu].load(Ordering::Relaxed) || cpu_idle(cpu) || SINCE[cpu].load(Ordering::Relaxed) == 0 {
            SEEN[cpu].store(count, Ordering::Relaxed);
            let since = SINCE[cpu].swap(now, Ordering::Relaxed);
            if REPORTED[cpu].load(Ordering::Relaxed) {
                let mut line = Line::new();
                let _ = write!(line, "cpu {cpu} went on after {} s", now.saturating_sub(since) / 1_000_000_000);
                if crate::console::status_warn_now(line.as_str()) {
                    REPORTED[cpu].store(false, Ordering::Relaxed);
                }
            }
            continue;
        }
        let stuck = now.saturating_sub(SINCE[cpu].load(Ordering::Relaxed));
        if stuck < STUCK_NS || REPORTED[cpu].load(Ordering::Relaxed) {
            continue;
        }
        // Where first: the stuck line ends a boot test at once.
        if let Some(at) = sample(cpu) {
            if !crate::console::status_warn_now(at.as_str()) {
                continue;
            }
        }
        let mut line = Line::new();
        let _ = write!(line, "cpu {cpu} stuck for {} s in the kernel: ", stuck / 1_000_000_000);
        describe(&mut line, CURRENT[cpu].load(Ordering::Relaxed));
        if crate::console::status_warn_now(line.as_str()) {
            REPORTED[cpu].store(true, Ordering::Relaxed);
        }
    }
}

/// CPU 0's tick count as the other CPUs last saw it, when it last moved
/// (0: never looked), whether its stall was reported, and which CPU is
/// reporting it (one at a time).
static BSP_TICKS: AtomicU64 = AtomicU64::new(0);
static BSP_SINCE: AtomicU64 = AtomicU64::new(0);
static BSP_REPORTED: AtomicBool = AtomicBool::new(false);
static BSP_REPORTING: AtomicBool = AtomicBool::new(false);

/// From the timer interrupt of every CPU but 0 (`time::note_tick`): CPU
/// 0's ticks must move.
pub fn bsp_check(now: u64) {
    if !PREEMPT_ON.load(Ordering::SeqCst) {
        return;
    }
    let ticks = crate::time::ticks();
    if ticks != BSP_TICKS.load(Ordering::Relaxed) || BSP_SINCE.load(Ordering::Relaxed) == 0 {
        BSP_TICKS.store(ticks, Ordering::Relaxed);
        let since = BSP_SINCE.swap(now, Ordering::Relaxed);
        if BSP_REPORTED.swap(false, Ordering::Relaxed) {
            let mut line = Line::new();
            let _ = write!(line, "cpu 0 went on after {} s", now.saturating_sub(since) / 1_000_000_000);
            crate::console::status_force(line.as_str());
        }
        return;
    }
    let stuck = now.saturating_sub(BSP_SINCE.load(Ordering::Relaxed));
    if stuck < STUCK_NS || BSP_REPORTED.load(Ordering::Relaxed) || BSP_REPORTING.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(at) = sample(0) {
        crate::console::status_force(at.as_str());
    }
    let mut line = Line::new();
    let _ = write!(line, "cpu 0 stuck for {} s, no ticks: ", stuck / 1_000_000_000);
    describe(&mut line, CURRENT[0].load(Ordering::Relaxed));
    crate::console::status_force(line.as_str());
    BSP_REPORTED.store(true, Ordering::Relaxed);
    BSP_REPORTING.store(false, Ordering::SeqCst);
}

/// `pid P NAME (slot S)`, or the slot alone when the task table is held
/// (by the stuck CPU, likely: which says something too).
fn describe(line: &mut Line, slot: usize) {
    let Some(tasks) = TASKS.try_lock() else {
        let _ = write!(line, "slot {slot}, the task table held");
        return;
    };
    if slot >= MAX_TASKS {
        let _ = write!(line, "slot {slot}");
        return;
    }
    let t = &tasks[slot];
    let n = t.name.iter().position(|&b| b == 0).unwrap_or(t.name.len());
    let name = core::str::from_utf8(&t.name[..n]).unwrap_or("?");
    let _ = write!(line, "pid {} {} (slot {slot})", t.tgid, name);
}

/// A report line on the stack: the heap's lock may be the stuck CPU's.
struct Line {
    buf: [u8; 240],
    len: usize,
}

impl Line {
    fn new() -> Self {
        Line { buf: [0; 240], len: 0 }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // Whole characters only, so the line stays UTF-8.
        for c in s.chars() {
            let mut b = [0; 4];
            let e = c.encode_utf8(&mut b);
            if self.len + e.len() > self.buf.len() {
                return Err(core::fmt::Error);
            }
            self.buf[self.len..self.len + e.len()].copy_from_slice(e.as_bytes());
            self.len += e.len();
        }
        Ok(())
    }
}
