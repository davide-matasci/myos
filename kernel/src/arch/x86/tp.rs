//! The user thread-pointer register (TLS base), for `task::tp`.


use core::sync::atomic::{AtomicU64, Ordering};

const IA32_FS_BASE: u32 = 0xC000_0100;

/// The FS base each CPU has loaded, to skip redundant `wrmsr`s.
static LOADED: [AtomicU64; crate::smp::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::smp::MAX_CPUS];

/// User code cannot change the FS base itself (no FSGSBASE), so the
/// saved value is always the current one.
pub fn read() -> Option<u64> {
    None
}

pub fn write(v: u64) {
    let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
    if LOADED[cpu].load(Ordering::Relaxed) == v {
        return;
    }
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") IA32_FS_BASE,
            in("eax") v as u32,
            in("edx") (v >> 32) as u32,
            options(nostack, preserves_flags),
        );
    }
    LOADED[cpu].store(v, Ordering::Relaxed);
}
