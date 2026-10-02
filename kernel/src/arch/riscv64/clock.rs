//! Clock sources: the `time` CSR and the goldfish RTC.

/// Nanoseconds since some point before boot: the `time` CSR at the QEMU
/// virt 10 MHz timebase (same assumption as the S-mode timer).
pub fn monotonic_ns() -> u64 {
    let t: u64;
    unsafe {
        core::arch::asm!("rdtime {t}", t = out(reg) t, options(nomem, nostack));
    }
    t.wrapping_mul(100)
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
