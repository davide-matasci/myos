//! Limine MP glue: AP entry and the per-CPU ids in the MP response.

use limine::mp::MpInfo;
use limine::request::MpResponse;

/// Where Limine starts an AP: mask interrupts until this CPU's IDT and timer
/// are programmed, then hand over to the generic entry.
pub unsafe extern "C" fn ap_entry(info: &MpInfo) -> ! {
    unsafe {
        core::arch::asm!("cli", options(nostack, preserves_flags));
        crate::smp::ap_entry_rust(info)
    }
}

/// Start `cpu` at `entry` with `extra` as its argument.
pub fn ap_bootstrap(cpu: &MpInfo, entry: unsafe extern "C" fn(&MpInfo) -> !, extra: u64) {
    cpu.bootstrap(entry, extra);
}

/// A CPU's hardware id as the MP response names it (the LAPIC id).
pub fn mp_cpu_id(cpu: &MpInfo) -> u64 {
    u64::from(cpu.lapic_id)
}

pub fn mp_bsp_id(resp: &MpResponse) -> u64 {
    u64::from(resp.bsp_lapic_id)
}

/// IPIs need no late enable here (the LAPIC is programmed in `ap_init`).
pub fn enable_ipi() {}
