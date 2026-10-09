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
//! The wall clock is the monotonic clock plus an offset taken from the RTC
//! ([`monotonic_ns`] + `WALL_OFFSET_NS`): it advances smoothly between RTC
//! reads, which happen at most every 50 ms. The offset moves only when the
//! RTC and the wall clock disagree by two whole seconds (the monotonic clock
//! is calibrated and the RTC has one-second resolution), so `tv_usec` never
//! jumps at a second edge, where it used to restart from the edge the next
//! RTC read happened to see (a 300 ms sleep measured 144 ms or 336 ms).
//!
//! `settimeofday` ([`set_wall`]) does not write the RTC: it keeps the
//! difference to the RTC's time (`SET_ADJUST_NS`) and adds it from then on,
//! until the next boot.

use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

/// CPU 0's timer interrupts (fewer than 100 a second while it idles
/// without its tick). Only a seed for the RNG.
static TICKS: AtomicU64 = AtomicU64::new(0);
/// `wall_ns = monotonic_ns + WALL_OFFSET_NS`; valid once the RTC was read.
static WALL_VALID: AtomicBool = AtomicBool::new(false);
static WALL_OFFSET_NS: AtomicI64 = AtomicI64::new(0);
/// What `settimeofday` added to the RTC's time (0 until it is called).
static SET_ADJUST_NS: AtomicI64 = AtomicI64::new(0);
/// Monotonic ns of the last RTC read.
static RTC_READ_AT_NS: AtomicU64 = AtomicU64::new(0);

/// Re-read the RTC at most this often.
const RTC_TTL_NS: u64 = 50_000_000;
/// The RTC moves the wall clock only when they disagree by this much: the
/// offset is taken within an RTC second, so one second of disagreement is
/// normal.
const RESYNC_SECS: i64 = 2;

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

/// When CPU 0's next tick blinks the cursor (an idle CPU 0 wakes for it).
pub fn next_blink_ns() -> u64 {
    NEXT_BLINK_NS.load(Ordering::Relaxed)
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

/// Busy-wait `ns` nanoseconds (a device's settle time, where nothing may
/// sleep).
pub fn spin_ns(ns: u64) {
    let end = monotonic_ns() + ns;
    while monotonic_ns() < end {
        core::hint::spin_loop();
    }
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

/// `(tv_sec, tv_usec)` for `SYS_GETTIMEOFDAY`: the monotonic clock shifted
/// to Unix time by the RTC offset, so it is smooth and only ever goes
/// forward between RTC re-syncs (`RESYNC_SECS`).
pub fn timeval() -> Option<(i64, i64)> {
    let now = monotonic_ns();
    let wall = wall_ns(now)?;
    Some((wall / 1_000_000_000, (wall % 1_000_000_000) / 1_000))
}

/// Set the wall clock to `secs` + `usec` (Unix time). The RTC keeps its own
/// time; the difference holds until the next boot.
pub fn set_wall(secs: i64, usec: i64) -> bool {
    if !(0..1_000_000).contains(&usec) || secs < 0 {
        return false;
    }
    let Some(target) = secs.checked_mul(1_000_000_000).and_then(|ns| ns.checked_add(usec * 1_000)) else {
        return false;
    };
    let Some(rtc_wall) = rtc_wall_ns(monotonic_ns()) else {
        return false;
    };
    SET_ADJUST_NS.store(target.wrapping_sub(rtc_wall), Ordering::Relaxed);
    true
}

/// Unix nanoseconds at monotonic `now`: the RTC's time plus what
/// `settimeofday` set.
fn wall_ns(now: u64) -> Option<i64> {
    Some(rtc_wall_ns(now)?.wrapping_add(SET_ADJUST_NS.load(Ordering::Relaxed)))
}

/// The RTC's Unix nanoseconds at monotonic `now`, reading the RTC when the
/// offset is not known yet or is due for a check.
fn rtc_wall_ns(now: u64) -> Option<i64> {
    let valid = WALL_VALID.load(Ordering::Relaxed);
    let due = now.saturating_sub(RTC_READ_AT_NS.load(Ordering::Relaxed)) >= RTC_TTL_NS;
    if !valid || due {
        let rtc = read_rtc_seconds()?;
        RTC_READ_AT_NS.store(now, Ordering::Relaxed);
        let offset = WALL_OFFSET_NS.load(Ordering::Relaxed);
        let wall_secs = (now as i64).wrapping_add(offset) / 1_000_000_000;
        if !valid || (rtc - wall_secs).abs() >= RESYNC_SECS {
            WALL_OFFSET_NS.store(rtc.wrapping_mul(1_000_000_000).wrapping_sub(now as i64), Ordering::Relaxed);
            WALL_VALID.store(true, Ordering::Relaxed);
        }
    }
    Some((now as i64).wrapping_add(WALL_OFFSET_NS.load(Ordering::Relaxed)))
}

fn read_rtc_seconds() -> Option<i64> {
    crate::arch::clock::rtc_unix_seconds()
}
