//! CPU-level primitives: interrupt masking, idle, cycle counter, per-CPU id
//! registers, TLB and cache maintenance.

/// Name for `/proc/cpuinfo`.
pub const NAME: &str = "riscv64";
/// What `input` calls the local keyboard in the boot banner.
pub const KEYBOARD_NAME: &str = "virtio keyboard";
/// Whether user tasks may be homed on the BSP: yes, `-smp 2` leaves only
/// one other hart.
pub const USER_TASKS_ON_BSP: bool = true;

const SIE: u64 = 1 << 1;

/// The interrupt-enable state (`sstatus`), for [`irq_restore`].
pub fn irq_save() -> u64 {
    unsafe {
        let r: u64;
        core::arch::asm!(
            "csrr {r}, sstatus",
            r = out(reg) r,
            options(nomem, nostack, preserves_flags)
        );
        r
    }
}

pub fn irq_restore(flags: u64) {
    unsafe {
        if flags & SIE != 0 {
            core::arch::asm!("csrs sstatus, {}", in(reg) SIE, options(nostack));
        } else {
            core::arch::asm!("csrc sstatus, {}", in(reg) SIE, options(nostack));
        }
    }
}

pub fn irq_off() {
    unsafe {
        core::arch::asm!("csrc sstatus, {}", in(reg) SIE, options(nostack));
    }
}

pub fn irq_on() {
    unsafe {
        core::arch::asm!("csrs sstatus, {}", in(reg) SIE, options(nostack));
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
        core::arch::asm!("mv {}, sp", out(reg) sp, options(nostack, preserves_flags));
    }
    sp
}

/// A free-running cycle counter (entropy, jitter): `rdcycle`.
pub fn cycle_counter() -> u64 {
    let cyc: u64;
    unsafe {
        core::arch::asm!("rdcycle {}", out(reg) cyc, options(nostack, nomem));
    }
    cyc
}

/// S-mode cannot read `mhartid`: the hart id comes from Limine's MP response.
pub fn hw_cpu_id() -> Option<u64> {
    None
}

/// The logical hart index in `tp`: set at BSP init / AP entry, reserved by
/// LLVM (never a temporary), and reloaded from the kernel-stack footer by the
/// trap vector on every trap from U-mode (where `tp` is the user TLS pointer).
pub fn cpu_id_reg() -> usize {
    let tp: usize;
    unsafe {
        core::arch::asm!("mv {0}, tp", out(reg) tp, options(nomem, nostack, preserves_flags));
    }
    tp
}

pub fn set_cpu_id_reg(logical: usize) {
    unsafe {
        core::arch::asm!("mv tp, {0}", in(reg) logical, options(nomem, nostack, preserves_flags));
    }
}

/// Safety net before entering U-mode: an out-of-range `tp` (which the trap
/// vector would otherwise re-derive from the stack footer anyway) is reset to
/// the BSP id. With `tp` reserved by LLVM and reloaded on every U-mode trap
/// this should never fire.
pub fn sync_cpu_id_reg() {
    if cpu_id_reg() < crate::smp::MAX_CPUS {
        return;
    }
    set_cpu_id_reg(crate::smp::boot_cpu());
}

/// Flush this hart's whole TLB.
pub fn flush_tlb_local() {
    unsafe {
        core::arch::asm!("sfence.vma zero, zero", options(nostack));
    }
}

/// Flush this hart's translation of the page at `va`.
pub fn flush_tlb_page_local(va: usize) {
    unsafe {
        core::arch::asm!("sfence.vma {v}, zero", v = in(reg) va, options(nostack));
    }
}

/// Make freshly written code visible to instruction fetch.
pub fn sync_icache(start: usize, size: usize) {
    let _ = start;
    if size != 0 {
        unsafe {
            core::arch::asm!("fence.i", options(nostack));
        }
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

/// Called periodically while waiting for APs to come online.
pub fn ap_wait_poke() {}
