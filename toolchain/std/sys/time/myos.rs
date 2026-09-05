//! Wall clock / monotonic for myos.
//!
//! `SystemTime::now` reads `SYS_GETTIMEOFDAY` (28). Instant has no monotonic
//! source yet and stays at zero so callers (e.g. ripgrep) do not panic.

use crate::time::Duration;

const SYS_GETTIMEOFDAY: usize = 28;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct Instant(Duration);

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct SystemTime(Duration);

pub const UNIX_EPOCH: SystemTime = SystemTime(Duration::from_secs(0));

impl Instant {
    pub fn now() -> Instant {
        Instant(Duration::ZERO)
    }

    pub fn checked_sub_instant(&self, other: &Instant) -> Option<Duration> {
        self.0.checked_sub(other.0)
    }

    pub fn checked_add_duration(&self, other: &Duration) -> Option<Instant> {
        Some(Instant(self.0.checked_add(*other)?))
    }

    pub fn checked_sub_duration(&self, other: &Duration) -> Option<Instant> {
        Some(Instant(self.0.checked_sub(*other)?))
    }
}

impl SystemTime {
    pub const MAX: SystemTime = SystemTime(Duration::MAX);
    pub const MIN: SystemTime = SystemTime(Duration::ZERO);

    pub fn now() -> SystemTime {
        let mut tv = [0i64; 2];
        let ret = unsafe { raw_gettimeofday(tv.as_mut_ptr() as usize) };
        if ret == usize::MAX {
            return UNIX_EPOCH;
        }
        let secs = tv[0].max(0) as u64;
        let micros = tv[1].clamp(0, 999_999) as u32;
        SystemTime(Duration::new(secs, micros * 1000))
    }

    pub fn sub_time(&self, other: &SystemTime) -> Result<Duration, Duration> {
        self.0.checked_sub(other.0).ok_or_else(|| other.0 - self.0)
    }

    pub fn checked_add_duration(&self, other: &Duration) -> Option<SystemTime> {
        Some(SystemTime(self.0.checked_add(*other)?))
    }

    pub fn checked_sub_duration(&self, other: &Duration) -> Option<SystemTime> {
        Some(SystemTime(self.0.checked_sub(*other)?))
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn raw_gettimeofday(tv: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "syscall",
        in("rax") SYS_GETTIMEOFDAY,
        in("rdi") tv,
        in("rsi") 0usize,
        in("rdx") 0usize,
        lateout("rax") ret,
        out("rcx") _,
        out("r11") _,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "aarch64")]
unsafe fn raw_gettimeofday(tv: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "svc #0",
        in("x8") SYS_GETTIMEOFDAY,
        in("x0") tv,
        in("x1") 0usize,
        in("x2") 0usize,
        lateout("x0") ret,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "riscv64")]
unsafe fn raw_gettimeofday(tv: usize) -> usize {
    let ret: usize;
    core::arch::asm!(
        "ecall",
        in("a7") SYS_GETTIMEOFDAY,
        in("a0") tv,
        in("a1") 0usize,
        in("a2") 0usize,
        lateout("a0") ret,
        options(nostack),
    );
    ret
}
