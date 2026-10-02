//! Limine MP glue: AP entry and the per-hart ids in the MP response.

use limine::mp::MpInfo;
use limine::request::MpResponse;

/// Where Limine starts a hart: mask interrupts until its trap vector and
/// timer are programmed, then hand over to the generic entry.
pub unsafe extern "C" fn ap_entry(info: &MpInfo) -> ! {
    unsafe {
        core::arch::asm!("csrc sstatus, {}", in(reg) 1u64 << 1, options(nostack));
        crate::smp::ap_entry_rust(info)
    }
}

/// Start `cpu` at `entry` with `extra` as its argument.
pub fn ap_bootstrap(cpu: &MpInfo, entry: unsafe extern "C" fn(&MpInfo) -> !, extra: u64) {
    cpu.bootstrap(entry, extra);
}

/// A hart's id as the MP response names it.
pub fn mp_cpu_id(cpu: &MpInfo) -> u64 {
    cpu.hartid
}

pub fn mp_bsp_id(resp: &MpResponse) -> u64 {
    resp.bsp_hartid
}
