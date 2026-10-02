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
/// running counter: x86_64 TSC (calibrated against the PIT in [`init`]),
/// aarch64 `CNTVCT_EL0` / `CNTFRQ_EL0`, riscv64 the `time` CSR at the QEMU
/// virt timebase. Cheap (one register read) and consistent across CPUs on
/// the platforms we run on. Drives sleep deadlines and the sub-second part
/// of `gettimeofday`.
pub fn monotonic_ns() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        x86clock::now_ns()
    }
    #[cfg(target_arch = "aarch64")]
    {
        let cnt: u64;
        let frq: u64;
        unsafe {
            core::arch::asm!("isb", "mrs {c}, cntvct_el0", "mrs {f}, cntfrq_el0",
                c = out(reg) cnt, f = out(reg) frq, options(nomem, nostack));
        }
        counter_to_ns(cnt, frq.max(1))
    }
    #[cfg(target_arch = "riscv64")]
    {
        let t: u64;
        unsafe {
            core::arch::asm!("rdtime {t}", t = out(reg) t, options(nomem, nostack));
        }
        // QEMU virt: 10 MHz timebase (same assumption as the S-mode timer).
        t.wrapping_mul(100)
    }
}

/// `counter / hz` in nanoseconds without overflowing for ~hundreds of years.
#[allow(dead_code)]
fn counter_to_ns(counter: u64, hz: u64) -> u64 {
    let secs = counter / hz;
    let rem = counter % hz;
    secs.wrapping_mul(1_000_000_000)
        .wrapping_add(((rem as u128 * 1_000_000_000u128) / hz as u128) as u64)
}

/// Calibrate the clock source (x86: TSC against the PIT). Call once on the
/// BSP during boot, before anything sleeps.
pub fn init() {
    #[cfg(target_arch = "x86_64")]
    x86clock::init();
}

#[cfg(target_arch = "x86_64")]
mod x86clock {
    use core::sync::atomic::{AtomicU64, Ordering};

    /// TSC ticks per second; 0 until calibrated (then `now_ns` falls back to a
    /// nominal 1 GHz, which only mis-scales timeouts during early boot).
    static TSC_HZ: AtomicU64 = AtomicU64::new(0);
    static TSC_BASE: AtomicU64 = AtomicU64::new(0);

    #[inline]
    fn rdtsc() -> u64 {
        let lo: u32;
        let hi: u32;
        unsafe {
            core::arch::asm!("lfence", "rdtsc", out("eax") lo, out("edx") hi,
                options(nomem, nostack, preserves_flags));
        }
        ((hi as u64) << 32) | lo as u64
    }

    #[inline]
    fn outb(port: u16, v: u8) {
        unsafe {
            core::arch::asm!("out dx, al", in("dx") port, in("al") v,
                options(nomem, nostack, preserves_flags));
        }
    }

    #[inline]
    fn inb(port: u16) -> u8 {
        let v: u8;
        unsafe {
            core::arch::asm!("in al, dx", in("dx") port, out("al") v,
                options(nomem, nostack, preserves_flags));
        }
        v
    }

    pub fn now_ns() -> u64 {
        let hz = TSC_HZ.load(Ordering::Relaxed);
        let t = rdtsc().wrapping_sub(TSC_BASE.load(Ordering::Relaxed));
        if hz == 0 {
            return t; // ~1 ns per tick on QEMU's nominal 1 GHz TSC
        }
        super::counter_to_ns(t, hz)
    }

    /// Measure the TSC against PIT channel 2 (1.193182 MHz) over 10 ms.
    /// Mode 0 one-shot: OUT goes high when the count reaches zero.
    pub fn init() {
        const PIT_HZ: u64 = 1_193_182;
        const WINDOW_MS: u64 = 10;
        let count = (PIT_HZ * WINDOW_MS / 1000) as u16;
        // Gate channel 2 on, speaker off.
        let port61 = inb(0x61);
        outb(0x61, (port61 & !0x02) | 0x01);
        outb(0x43, 0xB0); // channel 2, lobyte/hibyte, mode 0, binary
        outb(0x42, count as u8);
        outb(0x42, (count >> 8) as u8);
        let t0 = rdtsc();
        let mut spins = 0u32;
        while inb(0x61) & 0x20 == 0 {
            spins += 1;
            if spins > 50_000_000 {
                break; // no PIT: keep the nominal scale
            }
            core::hint::spin_loop();
        }
        let t1 = rdtsc();
        outb(0x61, port61 & !0x03);
        let delta = t1.wrapping_sub(t0);
        // Sanity: accept 50 MHz .. 20 GHz.
        if spins <= 50_000_000 && delta > 500_000 && delta < 200_000_000 {
            TSC_HZ.store(delta * (1000 / WINDOW_MS), Ordering::Relaxed);
        }
        TSC_BASE.store(rdtsc(), Ordering::Relaxed);
    }

    pub fn tsc_hz() -> u64 {
        TSC_HZ.load(Ordering::Relaxed)
    }
}

/// Calibrated clock rate for diagnostics (x86: TSC Hz; 0 elsewhere/unknown).
pub fn clock_hz() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        x86clock::tsc_hz()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
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
    #[cfg(target_arch = "x86_64")]
    {
        cmos::unix_seconds()
    }
    #[cfg(target_arch = "aarch64")]
    {
        pl031::unix_seconds()
    }
    #[cfg(target_arch = "riscv64")]
    {
        goldfish::unix_seconds()
    }
}

/// Days from civil (y, m, d) to Unix epoch day (Howard Hinnant).
#[cfg(target_arch = "x86_64")]
fn days_from_civil(mut y: i64, m: u32, d: u32) -> i64 {
    y -= if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as u64;
    era * 146097 + doe as i64 - 719468
}

#[cfg(target_arch = "x86_64")]
fn ymd_hms_to_unix(y: i64, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
    let days = days_from_civil(y, mo, d);
    days * 86400 + (h as i64) * 3600 + (mi as i64) * 60 + s as i64
}

#[cfg(target_arch = "x86_64")]
mod cmos {
    use super::ymd_hms_to_unix;

    const CMOS_ADDR: u16 = 0x70;
    const CMOS_DATA: u16 = 0x71;

    #[inline]
    fn outb(port: u16, value: u8) {
        unsafe {
            core::arch::asm!(
                "out dx, al",
                in("dx") port,
                in("al") value,
                options(nomem, nostack, preserves_flags)
            );
        }
    }

    #[inline]
    fn inb(port: u16) -> u8 {
        let value: u8;
        unsafe {
            core::arch::asm!(
                "in al, dx",
                in("dx") port,
                out("al") value,
                options(nomem, nostack, preserves_flags)
            );
        }
        value
    }

    fn cmos_read(reg: u8) -> u8 {
        // NMI disable bit = 0x80; keep clear for QEMU.
        outb(CMOS_ADDR, reg);
        inb(CMOS_DATA)
    }

    fn bcd_to_bin(v: u8) -> u8 {
        (v & 0x0f) + ((v >> 4) * 10)
    }

    pub fn unix_seconds() -> Option<i64> {
        // UIP wait: keep this tiny. Under QEMU TCG each `in`/`out` is
        // expensive; the old 10_000-iteration spin dominated every
        // gettimeofday when UIP looked set (or when callers hammered us).
        // A few polls is enough — if UIP is still set we read anyway
        // (worst case a torn BCD field, corrected on the next cache refresh).
        for _ in 0..32 {
            if cmos_read(0x0a) & 0x80 == 0 {
                break;
            }
        }
        let s = cmos_read(0x00);
        let mi = cmos_read(0x02);
        let h = cmos_read(0x04);
        let d = cmos_read(0x07);
        let mo = cmos_read(0x08);
        let y = cmos_read(0x09);
        let century = cmos_read(0x32);
        let status_b = cmos_read(0x0b);

        let (sec, min, hour, day, month, year, cent) = if status_b & 0x04 != 0 {
            // Binary mode
            (s, mi, h, d, mo, y, century)
        } else {
            (
                bcd_to_bin(s),
                bcd_to_bin(mi),
                bcd_to_bin(h),
                bcd_to_bin(d),
                bcd_to_bin(mo),
                bcd_to_bin(y),
                bcd_to_bin(century),
            )
        };

        if month < 1 || month > 12 || day < 1 || day > 31 || hour > 23 || min > 59 || sec > 60 {
            return None;
        }
        let full_year = if cent != 0 {
            (cent as i64) * 100 + year as i64
        } else {
            // QEMU usually sets century; fall back to 2000+.
            2000 + year as i64
        };
        Some(ymd_hms_to_unix(
            full_year,
            month as u32,
            day as u32,
            hour as u32,
            min as u32,
            sec as u32,
        ))
    }
}

#[cfg(target_arch = "aarch64")]
mod pl031 {
    /// QEMU virt PL031 RTC base (Identity-mapped in paging::map_devices).
    const PL031_BASE: usize = 0x0901_0000;
    const RTCDR: usize = 0x00; // data register: Unix seconds

    pub fn unix_seconds() -> Option<i64> {
        let ptr = (PL031_BASE + RTCDR) as *const u32;
        let secs = unsafe { core::ptr::read_volatile(ptr) };
        Some(secs as i64)
    }
}

#[cfg(target_arch = "riscv64")]
mod goldfish {
    /// QEMU virt goldfish RTC at 0x101000 (nanoseconds since Unix epoch).
    const RTC_BASE: usize = 0x0010_1000;
    const TIME_LOW: usize = 0x00;
    const TIME_HIGH: usize = 0x04;

    pub fn unix_seconds() -> Option<i64> {
        let low = unsafe { core::ptr::read_volatile((RTC_BASE + TIME_LOW) as *const u32) } as u64;
        let high =
            unsafe { core::ptr::read_volatile((RTC_BASE + TIME_HIGH) as *const u32) } as u64;
        let ns = (high << 32) | low;
        Some((ns / 1_000_000_000) as i64)
    }
}
