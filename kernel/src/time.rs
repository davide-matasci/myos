//! Wall-clock time from platform RTC (QEMU virt / PC CMOS).
//!
//! Used by `SYS_GETTIMEOFDAY` so userspace TLS can verify certificate
//! notBefore/notAfter against real Unix time.

/// Read Unix seconds since 1970-01-01 UTC from the platform RTC.
pub fn unix_seconds() -> Option<i64> {
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
        // Wait briefly for update-in-progress to clear.
        for _ in 0..10000 {
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
