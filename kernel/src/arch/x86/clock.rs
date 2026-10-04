//! Clock sources: the TSC (its rate from CPUID, else calibrated against
//! the PIT) and the CMOS RTC.


/// Nanoseconds since some point before boot (TSC, calibrated in [`init`]).
pub fn monotonic_ns() -> u64 {
    x86clock::now_ns()
}

/// Find the TSC rate: CPUID leaf 15H/16H when the CPU states it, else
/// calibrated against the PIT. Once, on the BSP, before anything sleeps.
pub fn init() {
    x86clock::init();
    let hz = x86clock::tsc_hz();
    crate::console::status_info(&alloc::format!(
        "tsc: {} MHz ({})",
        hz / 1_000_000,
        x86clock::source()
    ));
}

/// Calibrated clock rate for diagnostics (TSC Hz; 0 until calibrated).
pub fn clock_hz() -> u64 {
    x86clock::tsc_hz()
}

/// Unix seconds from the platform RTC.
pub fn rtc_unix_seconds() -> Option<i64> {
    cmos::unix_seconds()
}

mod x86clock {
    use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

    /// TSC ticks per second; 0 until calibrated (then `now_ns` falls back to a
    /// nominal 1 GHz, which only mis-scales timeouts during early boot).
    static TSC_HZ: AtomicU64 = AtomicU64::new(0);
    static TSC_BASE: AtomicU64 = AtomicU64::new(0);
    /// Where the rate came from, for the boot log.
    static SOURCE: AtomicU8 = AtomicU8::new(0);
    const SOURCES: [&str; 3] = ["nominal", "cpuid", "pit"];

    pub fn source() -> &'static str {
        SOURCES[SOURCE.load(Ordering::Relaxed) as usize]
    }

    /// The TSC rate the CPU states: CPUID leaf 15H gives the core crystal
    /// clock and the TSC/crystal ratio; when the crystal's rate is not
    /// filled in (Intel before Ice Lake), leaf 16H's processor base
    /// frequency is the TSC's nominal rate. AMD has neither leaf.
    fn cpuid_tsc_hz() -> Option<u64> {
        let (max_leaf, ..) = super::super::cpu::cpuid(0, 0);
        if max_leaf < 0x15 {
            return None;
        }
        let (den, num, crystal_hz, _) = super::super::cpu::cpuid(0x15, 0);
        if den == 0 || num == 0 {
            return None;
        }
        if crystal_hz != 0 {
            return Some(u64::from(crystal_hz) * u64::from(num) / u64::from(den));
        }
        if max_leaf < 0x16 {
            return None;
        }
        let (base_mhz, ..) = super::super::cpu::cpuid(0x16, 0);
        (base_mhz != 0).then(|| u64::from(base_mhz) * 1_000_000)
    }

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
        crate::time::counter_to_ns(t, hz)
    }

    /// The CPU's stated rate, else a measurement against PIT channel 2
    /// (1.193182 MHz) over 10 ms, mode 0 one-shot: OUT goes high when the
    /// count reaches zero. A PC without a PIT keeps the nominal scale.
    pub fn init() {
        if let Some(hz) = cpuid_tsc_hz() {
            TSC_HZ.store(hz, Ordering::Relaxed);
            SOURCE.store(1, Ordering::Relaxed);
            TSC_BASE.store(rdtsc(), Ordering::Relaxed);
            return;
        }
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
            SOURCE.store(2, Ordering::Relaxed);
        }
        TSC_BASE.store(rdtsc(), Ordering::Relaxed);
    }

    pub fn tsc_hz() -> u64 {
        TSC_HZ.load(Ordering::Relaxed)
    }
}

/// Days from civil (y, m, d) to Unix epoch day (Howard Hinnant).
fn days_from_civil(mut y: i64, m: u32, d: u32) -> i64 {
    y -= if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as u64;
    era * 146097 + doe as i64 - 719468
}

fn ymd_hms_to_unix(y: i64, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
    let days = days_from_civil(y, mo, d);
    days * 86400 + (h as i64) * 3600 + (mi as i64) * 60 + s as i64
}

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
