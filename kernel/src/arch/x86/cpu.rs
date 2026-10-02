//! CPU-level primitives: interrupt masking, idle, cycle counter, per-CPU id
//! registers, TLB and cache maintenance.

/// Name for `/proc/cpuinfo`.
pub const NAME: &str = "x86_64";
/// What `input` calls the local keyboard in the boot banner.
pub const KEYBOARD_NAME: &str = "PS/2 keyboard";
/// Whether user tasks may be homed on the BSP (it owns the console and the
/// UART drain; with APs available user work goes there).
pub const USER_TASKS_ON_BSP: bool = false;

/// The interrupt-enable state, for [`irq_restore`].
pub fn irq_save() -> u64 {
    unsafe {
        let r: u64;
        core::arch::asm!("pushfq; pop {r}", r = out(reg) r);
        r
    }
}

pub fn irq_restore(flags: u64) {
    unsafe {
        if flags & (1 << 9) != 0 {
            core::arch::asm!("sti", options(nostack, preserves_flags));
        } else {
            core::arch::asm!("cli", options(nostack, preserves_flags));
        }
    }
}

pub fn irq_off() {
    unsafe {
        core::arch::asm!("cli", options(nostack, preserves_flags));
    }
}

pub fn irq_on() {
    unsafe {
        core::arch::asm!("sti", options(nostack, preserves_flags));
    }
}

/// Halt until the next interrupt without changing the interrupt mask.
pub fn hlt() {
    unsafe {
        core::arch::asm!("hlt", options(nostack, preserves_flags));
    }
}

/// The current stack pointer.
pub fn read_sp() -> usize {
    let sp: usize;
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) sp, options(nostack, preserves_flags));
    }
    sp
}

/// A free-running cycle counter (entropy, jitter): the TSC.
pub fn cycle_counter() -> u64 {
    let (lo, hi): (u32, u32);
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nostack, nomem));
    }
    (lo as u64) | ((hi as u64) << 32)
}

/// The hardware id of this CPU (the initial APIC id), if the arch can read it.
pub fn hw_cpu_id() -> Option<u64> {
    let apic: u32;
    unsafe {
        core::arch::asm!(
            "mov eax, 1",
            "push rbx",
            "cpuid",
            "mov {apic:e}, ebx",
            "pop rbx",
            out("eax") _,
            apic = out(reg) apic,
            out("ecx") _,
            out("edx") _,
            options(preserves_flags),
        );
    }
    Some(u64::from(apic >> 24))
}

/// The logical CPU index stored in this CPU's id register (`IA32_TSC_AUX`,
/// written by [`set_cpu_id_reg`]); out of range before that.
pub fn cpu_id_reg() -> usize {
    // Use RDMSR (not RDTSCP) — qemu64 may lack the RDTSCP feature.
    let lo: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0xC000_0103u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, preserves_flags),
        );
    }
    lo as usize
}

pub fn set_cpu_id_reg(logical: usize) {
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") 0xC000_0103u32,
            in("eax") logical as u32,
            in("edx") 0u32,
            options(nostack, preserves_flags),
        );
    }
}

/// Re-derive the CPU id register if user code could have clobbered it
/// (nothing to do here: user code cannot write `IA32_TSC_AUX`).
pub fn sync_cpu_id_reg() {}

/// Flush this CPU's whole TLB.
pub fn flush_tlb_local() {
    unsafe {
        core::arch::asm!(
            "mov {cr3}, cr3",
            "mov cr3, {cr3}",
            cr3 = out(reg) _,
            options(nostack, preserves_flags),
        );
    }
}

/// Make freshly written code at `start..start+size` visible to instruction
/// fetch (x86 I-caches are coherent: only the TLB needs a reload).
pub fn sync_icache(start: usize, size: usize) {
    let _ = (start, size);
    flush_tlb_local();
}

/// Zero a 4 KiB page with word stores (the kernel is built unoptimized and
/// `write_bytes` lowers to a byte loop there, very slow under TCG).
///
/// # Safety
/// `page` must point at a writable, 8-byte-aligned 4 KiB page.
pub unsafe fn zero_page(page: *mut u8) {
    unsafe {
        core::arch::asm!(
            "rep stosq",
            inout("rcx") 4096 / 8 => _,
            inout("rdi") page => _,
            in("rax") 0u64,
            options(nostack, preserves_flags),
        );
    }
}

/// Called periodically while waiting for APs to come online.
pub fn ap_wait_poke() {}
