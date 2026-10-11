//! The layer's trace (`docs/linux-compat.md`, Debugging), set by writing a
//! level to `/proc/linux_trace` (it starts at `0`, off):
//!
//! - `1`: every syscall the layer does not implement is reported on the
//!   console as it answers `ENOSYS`, with the calling process, the number
//!   (the arch's own numbering) and the first three arguments. A program
//!   that gives up quietly then says what it asked for.
//! - `2`: the same, and every thread's last [`KEPT`] syscalls are kept
//!   with their results and its count of calls. Each write publishes what
//!   was kept so far after the level, so a program that hangs or spins
//!   shows which thread is busy and what each one last asked for (a call
//!   still waiting has no result yet). Nothing is printed per call: the
//!   console would scroll faster than anyone can read.

use alloc::string::String;
use core::fmt::Write;
use core::sync::atomic::{AtomicU8, Ordering};

use myos_abi::Lock;

use super::abi::{err, ENOSYS};
use crate::k::{api, task, MAX_TASKS};

static LEVEL: AtomicU8 = AtomicU8::new(0);

const NAME: &str = "linux_trace";

/// Calls kept per thread at level 2.
const KEPT: usize = 8;

#[derive(Clone, Copy)]
struct Call {
    nr: usize,
    args: [usize; 4],
    ret: Option<usize>,
}

/// One thread's last calls: `calls[n % KEPT]` is its `n`th.
struct Ring {
    pid: usize,
    count: u64,
    calls: [Call; KEPT],
}

const NO_CALL: Call = Call { nr: 0, args: [0; 4], ret: None };

static RINGS: [Lock<Ring>; MAX_TASKS] =
    [const { Lock::new(Ring { pid: 0, count: 0, calls: [NO_CALL; KEPT] }) }; MAX_TASKS];

/// Publish `/proc/linux_trace`, writable by whoever the policy lets write
/// `/proc` (`docs/security.md`: root).
pub fn init() {
    publish();
    let _ = api().proc_set_writer(NAME, Some(write));
}

/// The level, then at level 2 the kept calls by thread.
fn publish() {
    let level = LEVEL.load(Ordering::Relaxed);
    let mut text = alloc::format!("{level}\n");
    if level == 2 {
        for (tid, ring) in RINGS.iter().enumerate() {
            let r = ring.lock();
            if r.count == 0 {
                continue;
            }
            let _ = writeln!(text, "tid {tid} pid {}: {} calls", r.pid, r.count);
            for n in r.count.saturating_sub(KEPT as u64)..r.count {
                let c = r.calls[n as usize % KEPT];
                let [a0, a1, a2, a3] = c.args;
                let _ = write!(text, "  {} ({a0:#x}, {a1:#x}, {a2:#x}, {a3:#x})", c.nr);
                match c.ret {
                    Some(v) => show_ret(&mut text, v),
                    None => text.push_str(" ...\n"),
                }
            }
        }
    }
    let _ = api().proc_register(NAME, text.as_bytes());
}

fn show_ret(text: &mut String, v: usize) {
    match v as isize {
        e @ -4095..0 => _ = writeln!(text, " = {e}"),
        _ => _ = writeln!(text, " = {v:#x}"),
    }
}

/// A write of `0`, `1` or `2` (surrounding white space ignored) sets the
/// level; anything else is refused.
unsafe extern "C" fn write(data: *const u8, len: usize) -> i32 {
    // Copied before it is looked at: the buffer is the writer's.
    let mut buf = [0u8; 8];
    if data.is_null() || len == 0 || len > buf.len() {
        return -1;
    }
    unsafe { core::ptr::copy_nonoverlapping(data, buf.as_mut_ptr(), len) };
    let level = match buf[..len].trim_ascii() {
        [d @ b'0'..=b'2'] => d - b'0',
        _ => return -1,
    };
    LEVEL.store(level, Ordering::Relaxed);
    publish();
    len as i32
}

/// A syscall starts: kept at level 2. The answer goes to [`leave`] with
/// what this returns.
pub fn enter(nr: usize, a: &[usize; 6]) -> Option<u64> {
    if LEVEL.load(Ordering::Relaxed) != 2 {
        return None;
    }
    let mut r = RINGS.get(task::current_tid())?.lock();
    let pid = task::current_pid();
    if r.pid != pid {
        // The slot's previous task was another process.
        r.pid = pid;
        r.count = 0;
    }
    let n = r.count;
    r.calls[n as usize % KEPT] = Call { nr, args: [a[0], a[1], a[2], a[3]], ret: None };
    r.count += 1;
    Some(n)
}

/// The syscall [`enter`] kept as its `n`th answered `ret`.
pub fn leave(n: Option<u64>, ret: usize) {
    let Some(n) = n else { return };
    let Some(ring) = RINGS.get(task::current_tid()) else { return };
    let mut r = ring.lock();
    // Still kept: only this thread adds to its ring.
    if n < r.count && r.count - n <= KEPT as u64 {
        r.calls[n as usize % KEPT].ret = Some(ret);
    }
}

/// The answer to a syscall the layer does not implement (`ENOSYS`),
/// reported first when the trace is on.
pub fn unimplemented(nr: usize, a: &[usize; 6]) -> usize {
    if LEVEL.load(Ordering::Relaxed) != 0 {
        api().write_str(&alloc::format!(
            "linux: pid {} syscall {nr} ({:#x}, {:#x}, {:#x}): ENOSYS\n",
            task::current_pid(),
            a[0],
            a[1],
            a[2]
        ));
    }
    err(ENOSYS)
}
