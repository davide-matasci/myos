//! CPU-level primitives: interrupt masking, idle, cycle counter, per-CPU id
//! registers, TLB and cache maintenance.

/// Name for `/proc/cpuinfo`.
pub const NAME: &str = "aarch64";
/// What `input` calls the local keyboard in the boot banner.
pub const KEYBOARD_NAME: &str = "virtio keyboard";
/// Whether user tasks may be homed on the BSP (see `task::user_affinity`).
pub const USER_TASKS_ON_BSP: bool = false;

/// The interrupt mask state (`DAIF`), for [`irq_restore`].
pub fn irq_save() -> u64 {
    unsafe {
        let r: u64;
        core::arch::asm!(
            "mrs {r}, daif",
            r = out(reg) r,
            options(nomem, nostack, preserves_flags)
        );
        r
    }
}

pub fn irq_restore(flags: u64) {
    unsafe {
        core::arch::asm!("msr daif, {r}", r = in(reg) flags, options(nostack));
    }
}

pub fn irq_off() {
    unsafe {
        core::arch::asm!("msr daifset, #3", options(nostack));
    }
}

pub fn irq_on() {
    unsafe {
        core::arch::asm!("msr daifclr, #3", options(nostack));
    }
}

/// Wait for an interrupt without changing the interrupt mask.
pub fn hlt() {
    unsafe {
        core::arch::asm!("wfi", options(nostack, preserves_flags));
    }
}

/// The current stack pointer.
pub fn read_sp() -> usize {
    let sp: usize;
    unsafe {
        core::arch::asm!("mov {}, sp", out(reg) sp, options(nostack, preserves_flags));
    }
    sp
}

/// A free-running cycle counter (entropy, jitter): `CNTVCT_EL0`.
pub fn cycle_counter() -> u64 {
    let cnt: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0", out(reg) cnt, options(nostack, nomem));
    }
    cnt
}

/// The hardware id of this CPU (`MPIDR_EL1` affinity fields).
pub fn hw_cpu_id() -> Option<u64> {
    let mpidr: u64;
    unsafe {
        core::arch::asm!(
            "mrs {0}, mpidr_el1",
            out(reg) mpidr,
            options(nomem, preserves_flags)
        );
    }
    // Aff3:Aff2:Aff1:Aff0; clear [31:24] (MT/U) — Linux MPIDR_HWID_BITMASK /
    // Limine MPIDR_AFFINITY_MASK so MRS matches MpInfo::mpidr.
    Some(mpidr & 0xFF_00FF_FFFF)
}

/// The logical CPU index stored in this CPU's id register (`TPIDR_EL1`,
/// written by [`set_cpu_id_reg`]); out of range before that.
pub fn cpu_id_reg() -> usize {
    let tpidr: usize;
    unsafe {
        core::arch::asm!(
            "mrs {0}, tpidr_el1",
            out(reg) tpidr,
            options(nomem, nostack, preserves_flags)
        );
    }
    tpidr
}

pub fn set_cpu_id_reg(logical: usize) {
    unsafe {
        core::arch::asm!("msr tpidr_el1, {0}", in(reg) logical, options(nomem, nostack));
    }
}

/// Re-derive the CPU id register if user code could have clobbered it
/// (nothing to do here: EL0 cannot write `TPIDR_EL1`).
pub fn sync_cpu_id_reg() {}

/// Flush this CPU's whole TLB.
pub fn flush_tlb_local() {
    unsafe {
        core::arch::asm!("dsb ishst", options(nostack));
        core::arch::asm!("tlbi vmalle1is", options(nostack));
        core::arch::asm!("dsb ish; isb", options(nostack));
    }
}

/// Make freshly written code at `start..start+size` visible to instruction
/// fetch: clean the D-cache to PoU and invalidate the I-cache.
pub fn sync_icache(start: usize, size: usize) {
    if size == 0 {
        return;
    }
    unsafe {
        let mut addr = start & !63;
        let end = start + size;
        while addr < end {
            core::arch::asm!("dc cvau, {x}", x = in(reg) addr, options(nostack));
            addr += 64;
        }
        core::arch::asm!("dsb ish", options(nostack));
        addr = start & !63;
        while addr < end {
            core::arch::asm!("ic ivau, {x}", x = in(reg) addr, options(nostack));
            addr += 64;
        }
        core::arch::asm!("dsb ish; isb", options(nostack));
        core::arch::asm!("ic ialluis; dsb ish; isb", options(nostack));
    }
}

/// Zero a 4 KiB page with word stores.
///
/// # Safety
/// `page` must point at a writable, 8-byte-aligned 4 KiB page.
pub unsafe fn zero_page(page: *mut u8) {
    let w = page as *mut u64;
    let mut i = 0;
    while i < 4096 / 8 {
        unsafe { w.add(i).write(0) };
        i += 1;
    }
}

/// Called periodically while waiting for APs to come online: the Limine
/// trampoline parks on `ldar`/`wfe`, so publish and wake it.
pub fn ap_wait_poke() {
    unsafe {
        core::arch::asm!("dsb ishst; sev; yield", options(nostack));
    }
}

/// The exception level we run at (Limine may leave us in EL2).
pub fn current_el() -> u64 {
    let el: u64;
    unsafe {
        core::arch::asm!(
            "mrs {el}, CurrentEL",
            el = out(reg) el,
            options(nomem, nostack, preserves_flags)
        );
    }
    (el >> 2) & 3
}
