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

/// `cpuid` leaf `leaf`, subleaf `sub`: `(eax, ebx, ecx, edx)`.
pub fn cpuid(leaf: u32, sub: u32) -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {ebx:e}, ebx",
            "pop rbx",
            inout("eax") leaf => eax,
            ebx = out(reg) ebx,
            inout("ecx") sub => ecx,
            out("edx") edx,
            options(preserves_flags),
        );
    }
    (eax, ebx, ecx, edx)
}

/// The CPU has an x2APIC (CPUID.01H:ECX[21]).
pub fn has_x2apic() -> bool {
    cpuid(1, 0).2 & (1 << 21) != 0
}

/// The hardware id of this CPU: its x2APIC id (CPUID.0BH:EDX, 32 bits) when
/// the CPU has an x2APIC, the initial APIC id (CPUID.01H:EBX[31:24])
/// otherwise. The same id Limine's MP response names the CPU by.
pub fn hw_cpu_id() -> Option<u64> {
    if has_x2apic() && cpuid(0, 0).0 >= 0xB {
        return Some(u64::from(cpuid(0xB, 0).3));
    }
    Some(u64::from(cpuid(1, 0).1 >> 24))
}

/// How [`cpu_id_reg`] reads `IA32_TSC_AUX`: with `RDMSR` until
/// [`set_cpu_id_reg`] found a cheaper instruction for it.
static ID_READ: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(ID_RDMSR);
const ID_RDMSR: u8 = 0;
const ID_RDTSCP: u8 = 1;
const ID_RDPID: u8 = 2;

/// The instruction that reads `IA32_TSC_AUX` cheapest on this CPU: `RDPID`
/// (CPUID.07H:ECX[22]), else `RDTSCP` (CPUID.80000001H:EDX[27]), else
/// `RDMSR`.
fn id_read_method() -> u8 {
    if cpuid(0, 0).0 >= 7 && cpuid(7, 0).2 & (1 << 22) != 0 {
        ID_RDPID
    } else if cpuid(0x8000_0000, 0).0 >= 0x8000_0001 && cpuid(0x8000_0001, 0).3 & (1 << 27) != 0 {
        ID_RDTSCP
    } else {
        ID_RDMSR
    }
}

/// The logical CPU index stored in this CPU's id register (`IA32_TSC_AUX`,
/// written by [`set_cpu_id_reg`]); out of range before that.
///
/// Every `current_slot` asks, several times per syscall, so the read is
/// `RDPID` or `RDTSCP` where the CPU has one: `RDMSR` is tens of times
/// their cost on hardware, and under KVM it leaves the guest every time
/// (KVM lets the two instructions through but emulates the MSR read).
pub fn cpu_id_reg() -> usize {
    let id: u64;
    unsafe {
        match ID_READ.load(core::sync::atomic::Ordering::Relaxed) {
            ID_RDPID => core::arch::asm!("rdpid {id}", id = out(reg) id, options(nomem, nostack, preserves_flags)),
            ID_RDTSCP => core::arch::asm!(
                "rdtscp",
                out("eax") _,
                out("edx") _,
                out("rcx") id,
                options(nomem, nostack, preserves_flags),
            ),
            _ => core::arch::asm!(
                "rdmsr",
                in("ecx") 0xC000_0103u32,
                out("rax") id,
                out("edx") _,
                options(nomem, nostack, preserves_flags),
            ),
        }
    }
    id as u32 as usize
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
    // The same on every CPU of a machine; the BSP stores it first.
    ID_READ.store(id_read_method(), core::sync::atomic::Ordering::Relaxed);
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

/// Flush this CPU's translation of the page at `va`.
pub fn flush_tlb_page_local(va: usize) {
    unsafe {
        core::arch::asm!("invlpg [{v}]", v = in(reg) va, options(nostack, preserves_flags));
    }
}

/// Make freshly written code at `start..start+size` visible to instruction
/// fetch: nothing to do, x86 instruction fetch sees stores through any
/// mapping (and QEMU's translator tracks them by physical page). Mapping
/// changes are flushed by their callers.
pub fn sync_icache(start: usize, size: usize) {
    let _ = (start, size);
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
