//! Clock sources: the generic timer counter and the PL031 RTC.



/// Nanoseconds since some point before boot (`CNTVCT_EL0` at `CNTFRQ_EL0`).
pub fn monotonic_ns() -> u64 {
    let cnt: u64;
    let frq: u64;
    unsafe {
        core::arch::asm!("isb", "mrs {c}, cntvct_el0", "mrs {f}, cntfrq_el0",
            c = out(reg) cnt, f = out(reg) frq, options(nomem, nostack));
    }
    crate::time::counter_to_ns(cnt, frq.max(1))
}

/// Nothing to calibrate: the counter frequency is architectural.
pub fn init() {}

/// No calibrated rate to report.
pub fn clock_hz() -> u64 {
    0
}

/// Unix seconds from the platform RTC.
pub fn rtc_unix_seconds() -> Option<i64> {
    pl031::unix_seconds()
}

/// PL031 RTC base from the device tree (`arm,pl031`); 0 = none.
pub fn set_rtc_base(base: usize) {
    pl031::BASE.store(base, core::sync::atomic::Ordering::SeqCst);
}

mod pl031 {
    use core::sync::atomic::{AtomicUsize, Ordering};

    pub(super) static BASE: AtomicUsize = AtomicUsize::new(0);
    const RTCDR: usize = 0x00; // data register: Unix seconds

    pub fn unix_seconds() -> Option<i64> {
        let base = BASE.load(Ordering::Relaxed);
        if base == 0 {
            return None;
        }
        let ptr = (base + RTCDR) as *const u32;
        let secs = unsafe { core::ptr::read_volatile(ptr) };
        Some(secs as i64)
    }
}
