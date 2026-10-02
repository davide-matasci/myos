//! Wall-clock time from platform RTC (QEMU virt / PC CMOS).
//!
//! Used by `SYS_GETTIMEOFDAY` so userspace TLS can verify certificate
//! notBefore/notAfter against real Unix time.
//!
//! ## Performance (CI #873)
//!
//! Userspace `poll`/`select`/connect timeouts busy-wait on `gettimeofday`
//! (`pollselect.c`, `socket.c`). Returning `tv_usec = 0` made those loops
//! only observe second edges, so they called the syscall as fast as TCG
//! would run until the RTC second flipped — fine on aarch64 (one MMIO
//! load) but catastrophic on x86: each call did up to 10_000 CMOS port
//! I/O waits for the UIP bit, then 8 more `in`/`out`s. That is the main
//! reason x86 TCG spent minutes in `git commit` / https / the first
//! os-test `tcc` while aarch64 finished the same work in seconds.
//!
//! We cache RTC seconds and take `tv_usec` from the monotonic clock
//! ([`monotonic_ns`]) so time advances within a second without hammering the
//! CMOS.

use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

/// Monotonic tick count (timer IRQ). Not wall-calibrated; used for cache
/// freshness and approximate sub-second timestamps.
static TICKS: AtomicU64 = AtomicU64::new(0);
static CACHE_VALID: AtomicBool = AtomicBool::new(false);
static CACHED_SECS: AtomicI64 = AtomicI64::new(0);
/// Monotonic ns when `CACHED_SECS` was last refreshed from the RTC.
static CACHED_AT_NS: AtomicU64 = AtomicU64::new(0);
/// Monotonic ns when `CACHED_SECS` last changed (start of this second, approx).
static SEC_START_NS: AtomicU64 = AtomicU64::new(0);

/// Re-read the RTC at most this often.
const CACHE_TTL_NS: u64 = 50_000_000;

/// Blink phase flip cadence.
const BLINK_INTERVAL_NS: u64 = 500_000_000;
static NEXT_BLINK_NS: AtomicU64 = AtomicU64::new(0);

/// Called from each arch timer IRQ (before `schedule`). Only the BSP's ticks
/// count: every CPU has its own timer, so summing them would make the count
/// run `n_cpus` times too fast.
#[inline]
pub fn note_tick() {
    if crate::smp::cpu_id() != 0 {
        return;
    }
    TICKS.fetch_add(1, Ordering::Relaxed);
    let now = monotonic_ns();
    if now >= NEXT_BLINK_NS.load(Ordering::Relaxed) {
        NEXT_BLINK_NS.store(now + BLINK_INTERVAL_NS, Ordering::Relaxed);
        crate::console::cursor_blink();
    }
}

#[inline]
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}


// --- Monotonic clock ---------------------------------------------------------

/// Nanoseconds since some point before boot, from the architecture's free
/// running counter (see `arch::clock`: x86_64 TSC calibrated against the PIT,
/// aarch64 `CNTVCT_EL0` / `CNTFRQ_EL0`, riscv64 the `time` CSR). Cheap (one
/// register read) and consistent across CPUs on the platforms we run on.
/// Drives sleep deadlines and the sub-second part of `gettimeofday`.
#[inline]
pub fn monotonic_ns() -> u64 {
    crate::arch::clock::monotonic_ns()
}

/// `counter / hz` in nanoseconds without overflowing for ~hundreds of years.
#[allow(dead_code)]
pub(crate) fn counter_to_ns(counter: u64, hz: u64) -> u64 {
    let secs = counter / hz;
    let rem = counter % hz;
    secs.wrapping_mul(1_000_000_000)
        .wrapping_add(((rem as u128 * 1_000_000_000u128) / hz as u128) as u64)
}

/// Calibrate the clock source (x86: TSC against the PIT). Call once on the
/// BSP during boot, before anything sleeps.
pub fn init() {
    crate::arch::clock::init();
}

/// Calibrated clock rate for diagnostics (x86: TSC Hz; 0 elsewhere/unknown).
pub fn clock_hz() -> u64 {
    crate::arch::clock::clock_hz()
}

/// Read Unix seconds since 1970-01-01 UTC from the platform RTC (cached).
pub fn unix_seconds() -> Option<i64> {
    timeval().map(|(s, _)| s)
}

/// `(tv_sec, tv_usec)` for `SYS_GETTIMEOFDAY`. `tv_usec` is synthesized
/// from timer ticks so userspace elapsed-time loops make progress inside
/// a wall-clock second without re-entering the RTC path every call.
pub fn timeval() -> Option<(i64, i64)> {
    let now = monotonic_ns();
    let secs = cached_unix_seconds(now)?;
    let start = SEC_START_NS.load(Ordering::Relaxed);
    let delta = now.saturating_sub(start);
    // Microseconds into the current RTC second; the RTC is re-read at least
    // every CACHE_TTL_NS so this stays within a second of the true edge.
    let usec = core::cmp::min(delta / 1_000, 999_999) as i64;
    Some((secs, usec))
}

fn cached_unix_seconds(now: u64) -> Option<i64> {
    if CACHE_VALID.load(Ordering::Relaxed) {
        let at = CACHED_AT_NS.load(Ordering::Relaxed);
        if now.saturating_sub(at) < CACHE_TTL_NS {
            return Some(CACHED_SECS.load(Ordering::Relaxed));
        }
    }
    let secs = read_rtc_seconds()?;
    let prev = CACHED_SECS.load(Ordering::Relaxed);
    let was = CACHE_VALID.load(Ordering::Relaxed);
    CACHED_SECS.store(secs, Ordering::Relaxed);
    CACHED_AT_NS.store(now, Ordering::Relaxed);
    if !was || secs != prev {
        SEC_START_NS.store(now, Ordering::Relaxed);
    }
    CACHE_VALID.store(true, Ordering::Relaxed);
    Some(secs)
}

fn read_rtc_seconds() -> Option<i64> {
    crate::arch::clock::rtc_unix_seconds()
}
