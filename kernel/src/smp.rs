//! Multi-core bring-up via Limine MP + cross-CPU scheduling hooks.
//!
//! The scheduler itself lives in `task/`; this module tracks online CPUs,
//! AP entry, IPI TLB shootdown / reschedule, and `/proc/cpuinfo`.
//! See `docs/pci-acpi-smp.md`.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use crate::arch;
use crate::console;
use crate::limine_boot;

pub const MAX_CPUS: usize = 8;

#[derive(Clone, Copy)]
struct CpuInfo {
    online: bool,
    hw_id: u64,
}

static CPUS: Mutex<[CpuInfo; MAX_CPUS]> = Mutex::new(
    [CpuInfo {
        online: false,
        hw_id: 0,
    }; MAX_CPUS],
);
static ONLINE: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
static HW_IDS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static SCHED_TICKS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// Logical CPU index stashed for arches that use `tp` (riscv) / until hw id works.
static BOOT_CPU: AtomicUsize = AtomicUsize::new(0);
static AP_PROGRESS: AtomicUsize = AtomicUsize::new(0);

/// TLB shootdown barrier: sender sets remaining, each remote IPI decrements.
static TLB_REMAINING: AtomicUsize = AtomicUsize::new(0);
static TLB_LOCK: Mutex<()> = Mutex::new(());

/// Soft IPI reason bits (riscv software interrupt carries no vector).
pub const IPI_BIT_TLB: u64 = 1;
pub const IPI_BIT_RESCHED: u64 = 2;
static IPI_BITS: AtomicU64 = AtomicU64::new(0);

fn hw_cpu_id() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
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
        u64::from(apic >> 24)
    }
    #[cfg(target_arch = "aarch64")]
    {
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
        mpidr & 0xFF_00FF_FFFF
    }
    #[cfg(target_arch = "riscv64")]
    {
        // S-mode: no mhartid. Logical index lives in `tp` (set at AP entry / BSP init).
        let tp: usize;
        unsafe {
            core::arch::asm!("mv {0}, tp", out(reg) tp, options(nomem, nostack, preserves_flags));
        }
        if tp < MAX_CPUS {
            let id = HW_IDS[tp].load(Ordering::SeqCst);
            if id != 0 {
                return id;
            }
        }
        HW_IDS[0].load(Ordering::SeqCst)
    }
}

/// Logical CPU index for the caller (0 = BSP).
pub fn cpu_id() -> usize {
    #[cfg(target_arch = "riscv64")]
    {
        let tp: usize;
        unsafe {
            core::arch::asm!("mv {0}, tp", out(reg) tp, options(nomem, nostack, preserves_flags));
        }
        if tp < MAX_CPUS {
            return tp;
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        let tpidr: usize;
        unsafe {
            core::arch::asm!(
                "mrs {0}, tpidr_el1",
                out(reg) tpidr,
                options(nomem, nostack, preserves_flags)
            );
        }
        if tpidr < MAX_CPUS {
            return tpidr;
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        // Logical id written to IA32_TSC_AUX in interrupt init / AP entry.
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
        let id = lo as usize;
        if id < MAX_CPUS {
            return id;
        }
    }
    let hw = hw_cpu_id();
    for i in 0..MAX_CPUS {
        if ONLINE[i].load(Ordering::SeqCst) && HW_IDS[i].load(Ordering::SeqCst) == hw {
            return i;
        }
    }
    BOOT_CPU.load(Ordering::SeqCst)
}

pub fn cpu_online(i: usize) -> bool {
    i < MAX_CPUS && ONLINE[i].load(Ordering::SeqCst)
}

pub fn cpu_hw_id(i: usize) -> u64 {
    if i < MAX_CPUS {
        HW_IDS[i].load(Ordering::SeqCst)
    } else {
        0
    }
}

pub fn online_count() -> usize {
    let mut n = 0usize;
    for i in 0..MAX_CPUS {
        if ONLINE[i].load(Ordering::SeqCst) {
            n += 1;
        }
    }
    n.max(1)
}

pub fn sched_ticks(cpu: usize) -> u64 {
    if cpu < MAX_CPUS {
        SCHED_TICKS[cpu].load(Ordering::Relaxed)
    } else {
        0
    }
}

pub fn note_schedule() {
    let id = cpu_id();
    if id < MAX_CPUS {
        SCHED_TICKS[id].fetch_add(1, Ordering::Relaxed);
    }
}


pub fn ipi_mark_tlb() {
    IPI_BITS.fetch_or(IPI_BIT_TLB, Ordering::SeqCst);
}

pub fn ipi_mark_resched() {
    IPI_BITS.fetch_or(IPI_BIT_RESCHED, Ordering::SeqCst);
}

pub fn ipi_is_tlb() -> bool {
    IPI_BITS.load(Ordering::SeqCst) & IPI_BIT_TLB != 0
}

pub fn ipi_is_resched() -> bool {
    IPI_BITS.load(Ordering::SeqCst) & IPI_BIT_RESCHED != 0
}

fn ipi_clear_handled() {
    // Clear both; senders re-set before each blast. Slightly coarse but safe.
    IPI_BITS.store(0, Ordering::SeqCst);
}

pub fn tlb_ipi_ack() {
    let _ = TLB_REMAINING.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| {
        Some(v.saturating_sub(1))
    });
    #[cfg(target_arch = "riscv64")]
    {
        // Soft IPI reason consumed.
        IPI_BITS.fetch_and(!(IPI_BIT_TLB), Ordering::SeqCst);
    }
}

/// Invalidate this CPU's user TLB and ask every other online CPU to do the same.
pub fn tlb_shootdown() {
    let others = online_count().saturating_sub(1);
    if others == 0 {
        return;
    }
    // Serialize shootdowns. Spin on the lock instead of dropping the flush:
    // reclaim must not free frames while a peer may still cache translations.
    // Bounded — a remote inside `schedule` (IF off) cannot EOI until it
    // returns; unbounded wait deadlocked SMP bring-up under UEFI timing.
    let mut lock_spins = 0u32;
    let _guard = loop {
        if let Some(g) = TLB_LOCK.try_lock() {
            break g;
        }
        lock_spins += 1;
        if lock_spins >= 2_000_000 {
            return;
        }
        core::hint::spin_loop();
    };
    TLB_REMAINING.store(others, Ordering::SeqCst);
    #[cfg(target_arch = "riscv64")]
    ipi_mark_tlb();
    arch::ipi_tlb_shootdown();
    let mut spins = 0u32;
    while TLB_REMAINING.load(Ordering::SeqCst) > 0 && spins < 2_000_000 {
        core::hint::spin_loop();
        spins += 1;
    }
    #[cfg(target_arch = "riscv64")]
    ipi_clear_handled();
}

/// Wake idle CPUs so they notice newly-ready tasks.
pub fn kick_cpus() {
    if online_count() <= 1 {
        return;
    }
    #[cfg(target_arch = "riscv64")]
    ipi_mark_resched();
    arch::ipi_reschedule();
}

pub fn cpuinfo_text() -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::new();
    let n = online_count();
    let arch_name = {
        #[cfg(target_arch = "x86_64")]
        {
            "x86_64"
        }
        #[cfg(target_arch = "aarch64")]
        {
            "aarch64"
        }
        #[cfg(target_arch = "riscv64")]
        {
            "riscv64"
        }
    };
    push_str(&mut out, "processor_count: ");
    push_dec(&mut out, n as u64);
    push_str(&mut out, "\narch: ");
    push_str(&mut out, arch_name);
    push_str(&mut out, "\nscheduler: smp-rr\n");
    let cpus = CPUS.lock();
    for i in 0..MAX_CPUS {
        let online = ONLINE[i].load(Ordering::SeqCst);
        if !online && i != 0 {
            continue;
        }
        let c = cpus[i];
        let hw = if c.hw_id != 0 {
            c.hw_id
        } else {
            HW_IDS[i].load(Ordering::SeqCst)
        };
        push_str(&mut out, "\nprocessor\t: ");
        push_dec(&mut out, i as u64);
        push_str(&mut out, "\nhw_id\t\t: 0x");
        push_hex(&mut out, hw);
        push_str(&mut out, "\nonline\t\t: ");
        push_str(&mut out, if online || i == 0 { "yes" } else { "no" });
        push_str(&mut out, "\nschedules\t: ");
        push_dec(&mut out, SCHED_TICKS[i].load(Ordering::Relaxed));
        push_str(&mut out, "\n");
        if !ONLINE[0].load(Ordering::SeqCst) && i == 0 {
            break;
        }
    }
    out
}

fn push_str(out: &mut alloc::vec::Vec<u8>, s: &str) {
    out.extend_from_slice(s.as_bytes());
}

fn push_dec(out: &mut alloc::vec::Vec<u8>, mut v: u64) {
    if v == 0 {
        out.push(b'0');
        return;
    }
    let mut tmp = [0u8; 20];
    let mut t = 0;
    while v > 0 {
        tmp[t] = b'0' + (v % 10) as u8;
        v /= 10;
        t += 1;
    }
    while t > 0 {
        t -= 1;
        out.push(tmp[t]);
    }
}

fn push_hex(out: &mut alloc::vec::Vec<u8>, mut v: u64) {
    let mut tmp = [b'0'; 16];
    for i in (0..16).rev() {
        let n = (v & 0xf) as u8;
        tmp[i] = if n < 10 { b'0' + n } else { b'a' + (n - 10) };
        v >>= 4;
    }
    let mut s = 0;
    while s < 15 && tmp[s] == b'0' {
        s += 1;
    }
    out.extend_from_slice(&tmp[s..]);
}

/// Linker-relocated AP entry pointer (fn-item casts are unreliable at runtime).
static AP_ENTRY_PTR: unsafe extern "C" fn(&limine::mp::MpInfo) -> ! = myos_smp_ap_entry;

/// Record BSP and bring secondary CPUs online via Limine MP.
pub fn init() {
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("mv tp, zero", options(nomem, nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr tpidr_el1, xzr", options(nomem, nostack));
    }

    let hw = {
        #[cfg(target_arch = "riscv64")]
        {
            // Prefer Limine BSP hartid when available.
            if let Some(resp) = limine_boot::MP.response() {
                resp.bsp_hartid
            } else {
                0
            }
        }
        #[cfg(not(target_arch = "riscv64"))]
        {
            hw_cpu_id()
        }
    };
    HW_IDS[0].store(hw, Ordering::SeqCst);
    ONLINE[0].store(true, Ordering::SeqCst);
    BOOT_CPU.store(0, Ordering::SeqCst);
    {
        let mut cpus = CPUS.lock();
        cpus[0] = CpuInfo {
            online: true,
            hw_id: hw,
        };
    }

    console::status_progress("smp");
    let Some(resp) = limine_boot::MP.response() else {
        console::status_info("smp: no Limine MP (UP)");
        return;
    };
    let mp_cpus = resp.cpus();
    if mp_cpus.len() <= 1 {
        console::status_ok("smp: 1 CPU");
        return;
    }

    // aarch64: Limine lists APs but writing goto_address still does not
    // reliably enter myos_smp_ap_entry on QEMU virt+UEFI (BSP then waits /
    // hangs under release). Keep APs parked for boot-mini; GIC SGI IPI stubs
    // remain. Retry handoff when Limine/QEMU park loop is proven.
    #[cfg(target_arch = "aarch64")]
    {
        console::status_ok(&alloc::format!(
            "smp: 1 CPU ({} parked)",
            mp_cpus.len().saturating_sub(1)
        ));
        return;
    }

    let mut next = 1usize;
    for cpu in mp_cpus.iter() {
        #[cfg(target_arch = "x86_64")]
        let (is_bsp, hw_id) = (cpu.lapic_id == resp.bsp_lapic_id, u64::from(cpu.lapic_id));
        #[cfg(target_arch = "aarch64")]
        let (is_bsp, hw_id) = (cpu.mpidr == resp.bsp_mpidr, cpu.mpidr);
        #[cfg(target_arch = "riscv64")]
        let (is_bsp, hw_id) = (cpu.hartid == resp.bsp_hartid, cpu.hartid);
        if is_bsp {
            continue;
        }
        if next >= MAX_CPUS {
            break;
        }
        let logical = next;
        HW_IDS[logical].store(hw_id, Ordering::SeqCst);
        {
            let mut guard = CPUS.lock();
            guard[logical] = CpuInfo {
                online: false,
                hw_id,
            };
        }
        core::sync::atomic::fence(Ordering::SeqCst);
        cpu.bootstrap(AP_ENTRY_PTR, logical as u64);
        // Limine aarch64 park uses LDAR on goto_addr (not WFE). Clean the
        // MpInfo cache line to PoC so an AP that briefly had D-cache off
        // (or a non-coherent view) observes the Release store; DSB+SEV for
        // any WFE path.
        #[cfg(target_arch = "aarch64")]
        unsafe {
            // `cpu` is &&MpInfo; clean the MpInfo (goto_addr @ +24).
            let p = core::ptr::from_ref(*cpu) as usize;
            let goto = p + 24;
            core::arch::asm!(
                "dc cvac, {0}",
                "dc cvac, {1}",
                "dsb sy",
                "sev",
                in(reg) p,
                in(reg) goto,
                options(nostack),
            );
        }
        next += 1;
    }

    let want = next;
    // aarch64 TCG: if Limine never runs goto_address, AP_PROGRESS stays 0 —
    // fail fast instead of spinning for minutes (CI boot-mini timeout).
    let max_spins: u32 = {
        #[cfg(target_arch = "aarch64")]
        {
            2_000_000
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            20_000_000
        }
    };
    let mut spins = 0u32;
    while online_count() < want && spins < max_spins {
        core::hint::spin_loop();
        spins += 1;
        #[cfg(target_arch = "aarch64")]
        {
            if spins == 100_000 && AP_PROGRESS.load(Ordering::SeqCst) == 0 {
                // Still no AP entry — further waiting is futile on this platform.
                break;
            }
            if spins % 100_000 == 0 {
                unsafe {
                    core::arch::asm!("dsb sy; sev", options(nostack));
                }
            }
        }
    }
    let got = online_count();
    if got < want {
        console::status_info(&alloc::format!(
            "smp: {got}/{want} CPUs (AP progress={})",
            AP_PROGRESS.load(Ordering::SeqCst)
        ));
    }
    console::status_ok(&alloc::format!("smp: {got} CPUs online"));
    #[cfg(target_arch = "riscv64")]
    if got > 1 {
        crate::arch::enable_ipi();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn myos_smp_ap_entry(info: &limine::mp::MpInfo) -> ! {
    AP_PROGRESS.store(1, Ordering::SeqCst);
    // Mask IRQs until this CPU's IDT/timer are programmed.
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr daifset, #3", options(nomem, nostack));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("csrc sstatus, {}", in(reg) 1 << 1, options(nomem, nostack));
    }
    AP_PROGRESS.store(2, Ordering::SeqCst);

    let logical = info.extra_argument() as usize;
    AP_PROGRESS.store(3 + logical, Ordering::SeqCst);
    if logical == 0 || logical >= MAX_CPUS {
        loop {
            crate::arch::wait_interrupt();
        }
    }

    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!(
            "mv tp, {0}",
            in(reg) logical,
            options(nomem, nostack, preserves_flags)
        );
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!(
            "msr tpidr_el1, {0}",
            in(reg) logical,
            options(nomem, nostack)
        );
    }

    AP_PROGRESS.store(10, Ordering::SeqCst);
    crate::arch::ap_init(logical);
    AP_PROGRESS.store(20, Ordering::SeqCst);
    // ONLINE is set inside ap_idle_loop once CURRENT/idle exist — marking
    // online earlier let the BSP IPI us into schedule with CURRENT==0.
    core::sync::atomic::fence(Ordering::SeqCst);

    crate::task::ap_idle_loop(logical)
}

pub fn mark_running(logical: usize) {
    if logical < MAX_CPUS {
        ONLINE[logical].store(true, Ordering::SeqCst);
        let mut cpus = CPUS.lock();
        cpus[logical].online = true;
    }
}
