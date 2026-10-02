//! Clock sources: the `time` CSR (at the device tree's
//! `timebase-frequency`) and the goldfish RTC.

use core::sync::atomic::{AtomicU64, Ordering};

/// `time` CSR rate; QEMU `virt`'s 10 MHz until `set_timebase` runs.
static TIMEBASE_HZ: AtomicU64 = AtomicU64::new(10_000_000);

pub fn set_timebase(hz: u64) {
    TIMEBASE_HZ.store(hz.max(1), Ordering::SeqCst);
}

pub fn timebase_hz() -> u64 {
    TIMEBASE_HZ.load(Ordering::Relaxed)
}

/// Nanoseconds since some point before boot: the `time` CSR at the
/// timebase (the S-mode timer uses the same clock).
pub fn monotonic_ns() -> u64 {
    let t: u64;
    unsafe {
        core::arch::asm!("rdtime {t}", t = out(reg) t, options(nomem, nostack));
    }
    crate::time::counter_to_ns(t, timebase_hz())
}

/// Goldfish RTC base from the device tree (`google,goldfish-rtc`); 0 = none.
pub fn set_rtc_base(base: usize) {
    goldfish::BASE.store(base, Ordering::SeqCst);
}

/// Nothing to calibrate.
pub fn init() {}

/// No calibrated rate to report.
pub fn clock_hz() -> u64 {
    0
}

/// Unix seconds from the platform RTC.
pub fn rtc_unix_seconds() -> Option<i64> {
    goldfish::unix_seconds()
}

mod goldfish {
    use core::sync::atomic::{AtomicUsize, Ordering};

    /// Nanoseconds since the Unix epoch in two 32-bit halves.
    pub(super) static BASE: AtomicUsize = AtomicUsize::new(0);
    const TIME_LOW: usize = 0x00;
    const TIME_HIGH: usize = 0x04;

    pub fn unix_seconds() -> Option<i64> {
        let base = BASE.load(Ordering::Relaxed);
        if base == 0 {
            return None;
        }
        let low = unsafe { core::ptr::read_volatile((base + TIME_LOW) as *const u32) } as u64;
        let high =
            unsafe { core::ptr::read_volatile((base + TIME_HIGH) as *const u32) } as u64;
        let ns = (high << 32) | low;
        Some((ns / 1_000_000_000) as i64)
    }
}
