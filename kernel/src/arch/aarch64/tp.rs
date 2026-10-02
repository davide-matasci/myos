//! The user thread-pointer register (TLS base), for `task::tp`.


/// User code may write `tpidr_el0` directly.
pub fn read() -> Option<u64> {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, tpidr_el0", out(reg) v, options(nomem, nostack)) };
    Some(v)
}

pub fn write(v: u64) {
    unsafe { core::arch::asm!("msr tpidr_el0, {}", in(reg) v, options(nostack)) };
}
