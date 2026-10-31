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
//! and how long the stall lasted if the CPU comes back. The report is
//! made in the timer interrupt, so it allocates nothing and takes no lock
//! it could wait on: the stuck CPU may hold any of them.

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
        let mut line = Line::new();
        let _ = write!(line, "cpu {cpu} stuck for {} s in the kernel: ", stuck / 1_000_000_000);
        describe(&mut line, CURRENT[cpu].load(Ordering::Relaxed));
        if crate::console::status_warn_now(line.as_str()) {
            REPORTED[cpu].store(true, Ordering::Relaxed);
        }
    }
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
    buf: [u8; 120],
    len: usize,
}

impl Line {
    fn new() -> Self {
        Line { buf: [0; 120], len: 0 }
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
