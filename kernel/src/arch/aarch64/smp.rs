//! Limine MP glue: AP entry and the per-CPU ids in the MP response.

use limine::mp::MpInfo;
use limine::request::MpResponse;

/// Limine's aarch64 trampoline `eret`s here with X0=&MpInfo, SP=Limine
/// stack, DAIF masked, CPACR.FPEN=0, and VBAR cleared. A naked stub marks
/// progress and enables FP before any Rust prologue can touch NEON or the
/// stack frame, masks all exceptions and hands over to the generic entry.
#[unsafe(naked)]
pub unsafe extern "C" fn ap_entry(info: &MpInfo) -> ! {
    core::arch::naked_asm!(
        // x0 = &MpInfo (must preserve into rust entry)
        "adrp x1, {flag}",
        "add x1, x1, :lo12:{flag}",
        "mov x2, #1",
        "stlr x2, [x1]",
        // Enable FP/SIMD (Limine left CPACR/CPTR clear)
        "mrs x2, cpacr_el1",
        "orr x2, x2, #(3 << 20)",
        "msr cpacr_el1, x2",
        "isb",
        "msr daifset, #0xf",
        "b {rust}",
        flag = sym crate::smp::AP_PROGRESS,
        rust = sym crate::smp::ap_entry_rust,
    );
}

/// Start `cpu` at `entry` with `extra` as its argument.
///
/// Limine 12.x's aarch64 trampoline parks on `ldar` of goto_addr at
/// MpInfo+24, then `eret`s to that VA with X0=&MpInfo. Publish with STLR
/// (matches LDAR) and DC CVAC the line to PoC — required if an AP briefly
/// ran with D-cache off during trampoline bring-up. Use a raw code address
/// (not an fn-item temporary) so the stored pointer is the higher-half
/// entry symbol.
pub fn ap_bootstrap(cpu: &MpInfo, entry: unsafe extern "C" fn(&MpInfo) -> !, extra: u64) {
    // Layout: processor_id(4)+res(4)+mpidr(8)+stack/reserved(8)+goto(8)+extra(8)
    let base = core::ptr::from_ref(cpu) as usize;
    let extra_ptr = (base + 32) as *mut u64;
    let goto_ptr = (base + 24) as *mut usize;
    let entry = entry as *const () as usize;
    unsafe {
        // extra_argument first (Relaxed), then goto with STLR.
        core::ptr::write_volatile(extra_ptr, extra);
        core::arch::asm!(
            "stlr {entry}, [{goto}]",
            "dc cvac, {base}",
            "dc cvac, {goto}",
            "dsb ish",
            "sev",
            entry = in(reg) entry,
            goto = in(reg) goto_ptr,
            base = in(reg) base,
            options(nostack),
        );
    }
}

/// A CPU's hardware id as the MP response names it (MPIDR affinity).
pub fn mp_cpu_id(cpu: &MpInfo) -> u64 {
    cpu.mpidr
}

pub fn mp_bsp_id(resp: &MpResponse) -> u64 {
    resp.bsp_mpidr
}

/// IPIs need no late enable here (the GIC is programmed in `ap_init`).
pub fn enable_ipi() {}
