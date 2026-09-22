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

/// TLB shootdown epoch: sender bumps, every CPU (IPI or soft `tlb_service`)
/// advances `TLB_SEEN[cpu]` after a local flush. Soft service lets remotes
/// that are briefly IF-off inside `schedule`/`wait_child` still ACK — the
/// old remaining-counter + IRQ-only ACK deadlocked cross-CPU exit reclaim.
static TLB_EPOCH: AtomicU64 = AtomicU64::new(0);
static TLB_SEEN: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
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
        if tp < MAX_CPUS && ONLINE[tp].load(Ordering::SeqCst) {
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
        // BSP-only userspace (Limine/WFI APs stay !ONLINE): ignore tp entirely.
        // LLVM freely uses x4 as a temporary during deep expand_user_elf
        // (ripgrep); trusting a clobbered-but-ONLINE-looking tp reopened the
        // sepc=0 IPF after #151 even with WFI-park. Single ONLINE hart → BOOT.
        if online_count() <= 1 {
            return BOOT_CPU.load(Ordering::SeqCst);
        }
        let tp: usize;
        unsafe {
            core::arch::asm!("mv {0}, tp", out(reg) tp, options(nomem, nostack, preserves_flags));
        }
        // #146 required ONLINE[tp]. #147 dropped it so APs could read tp before
        // mark_running — but that also trusts a *clobbered* tp in 1..MAX_CPUS-1
        // (LLVM may use x4 as a temporary; user TLS can be a small integer).
        // Require ONLINE once multiple harts are scheduled.
        if tp < MAX_CPUS && ONLINE[tp].load(Ordering::SeqCst) {
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

/// Force `tp` back to a sane kernel CPU id after a long Rust path may have
/// clobbered x4. Safe under UP (tp=0) and after AP `mark_running`.
#[cfg(target_arch = "riscv64")]
pub fn sync_tp_for_kernel() {
    // Always rewrite tp when only the BSP is ONLINE: a "valid" tp==0 early in a
    // long expand can still be clobbered later, and callers that skip sync after
    // the first check would then see garbage. Force the known-good id.
    let id = if online_count() <= 1 {
        BOOT_CPU.load(Ordering::SeqCst)
    } else {
        let tp: usize;
        unsafe {
            core::arch::asm!("mv {0}, tp", out(reg) tp, options(nomem, nostack, preserves_flags));
        }
        if tp < MAX_CPUS && ONLINE[tp].load(Ordering::SeqCst) {
            return;
        }
        BOOT_CPU.load(Ordering::SeqCst)
    };
    unsafe {
        core::arch::asm!("mv tp, {0}", in(reg) id, options(nomem, nostack, preserves_flags));
    }
}

#[cfg(not(target_arch = "riscv64"))]
pub fn sync_tp_for_kernel() {}

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

fn flush_tlb_local() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let cr3: u64;
        core::arch::asm!(
            "mov {cr3}, cr3",
            "mov cr3, {cr3}",
            cr3 = out(reg) cr3,
            options(nostack, preserves_flags),
        );
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("dsb ishst", options(nostack));
        core::arch::asm!("tlbi vmalle1is", options(nostack));
        core::arch::asm!("dsb ish; isb", options(nostack));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("sfence.vma zero, zero", options(nostack));
    }
}

/// Flush this CPU's TLB if a shootdown epoch is pending. Safe with IF off —
/// call from `schedule` so remotes ACK without needing the IPI handler.
pub fn tlb_service() {
    let cpu = cpu_id();
    if cpu >= MAX_CPUS {
        return;
    }
    let epoch = TLB_EPOCH.load(Ordering::SeqCst);
    let seen = TLB_SEEN[cpu].load(Ordering::SeqCst);
    if seen >= epoch {
        return;
    }
    flush_tlb_local();
    let _ = TLB_SEEN[cpu].fetch_update(Ordering::SeqCst, Ordering::SeqCst, |s| {
        if s >= epoch {
            None
        } else {
            Some(epoch)
        }
    });
    #[cfg(target_arch = "riscv64")]
    {
        IPI_BITS.fetch_and(!IPI_BIT_TLB, Ordering::SeqCst);
    }
}

/// IPI handler entry: same as soft service (idempotent on epoch).
pub fn tlb_ipi_ack() {
    tlb_service();
}

fn tlb_all_seen(epoch: u64) -> bool {
    for i in 0..MAX_CPUS {
        if !ONLINE[i].load(Ordering::SeqCst) {
            continue;
        }
        if TLB_SEEN[i].load(Ordering::SeqCst) < epoch {
            return false;
        }
    }
    true
}

/// Invalidate this CPU's user TLB and ask every other online CPU to do the same.
pub fn tlb_shootdown() {
    let others = online_count().saturating_sub(1);
    if others == 0 {
        flush_tlb_local();
        return;
    }
    // Serialize shootdowns. Spin on the lock instead of dropping the flush:
    // reclaim must not free frames while a peer may still cache translations.
    // Bounded lock wait — then proceed under the epoch barrier anyway.
    let mut lock_spins = 0u32;
    let _guard = loop {
        if let Some(g) = TLB_LOCK.try_lock() {
            break Some(g);
        }
        // Help the holder: remotes IF-off in schedule still need to ACK.
        tlb_service();
        lock_spins += 1;
        if lock_spins >= 2_000_000 {
            break None;
        }
        core::hint::spin_loop();
    };
    let epoch = TLB_EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
    tlb_service();
    #[cfg(target_arch = "riscv64")]
    ipi_mark_tlb();
    arch::ipi_tlb_shootdown();
    let mut spins = 0u32;
    while !tlb_all_seen(epoch) && spins < 2_000_000 {
        // Soft-ACK path for peers stuck briefly cli in schedule/wait.
        tlb_service();
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

/// Quiet WFI park for riscv Limine APs (never marked ONLINE).
#[cfg(target_arch = "riscv64")]
static AP_PARK_ENTRY_PTR: unsafe extern "C" fn(&limine::mp::MpInfo) -> ! = myos_smp_ap_park;

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
    TLB_SEEN[0].store(TLB_EPOCH.load(Ordering::SeqCst), Ordering::SeqCst);
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

    // aarch64: real Limine goto_address bring-up (see bootstrap + naked entry
    // below). Do not park APs — TTBR0 device map is synced in ap_init.

    // riscv64: dual-hart DTB is required (OpenSBI BSP hartid=1 otherwise
    // Limine-panics). Full AP bring-up (ONLINE + ap_idle_loop) hung mid-`/ok`
    // (PR #150). Leaving APs in Limine's busy-spin still hit the ripgrep
    // `sepc=0` IPF under `-smp 2`. Hand each AP a SIE-masked WFI loop in
    // myos text *without* marking ONLINE — quiet park, no scheduler/IPI.
    #[cfg(target_arch = "riscv64")]
    {
        let mut parked = 0usize;
        for cpu in mp_cpus.iter() {
            let is_bsp = cpu.hartid == resp.bsp_hartid;
            if is_bsp {
                continue;
            }
            if parked + 1 >= MAX_CPUS {
                break;
            }
            let logical = parked + 1;
            HW_IDS[logical].store(cpu.hartid, Ordering::SeqCst);
            {
                let mut guard = CPUS.lock();
                guard[logical] = CpuInfo {
                    online: false,
                    hw_id: cpu.hartid,
                };
            }
            core::sync::atomic::fence(Ordering::SeqCst);
            // extra_argument is ignored by the park stub; pass logical for symmetry.
            cpu.bootstrap(AP_PARK_ENTRY_PTR, logical as u64);
            parked += 1;
        }
        // Wait until each AP has finished quieting (sie/stimecmp) and entered
        // the WFI loop — AP_PROGRESS 1 = entered stub, 2 = interrupts retired.
        // Continuing into userspace while an AP still busy-spins on pending STIP
        // is exactly the Limine-spin contention #151 meant to kill.
        for _ in 0..2_000_000 {
            if AP_PROGRESS.load(Ordering::SeqCst) >= 2 {
                break;
            }
            core::hint::spin_loop();
        }
        console::status_ok(&alloc::format!(
            "smp: 1 CPU ({} wfi-parked)",
            parked
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
        #[cfg(target_arch = "aarch64")]
        {
            // Limine 12.x aarch64 trampoline parks on `ldar` of goto_addr at
            // MpInfo+24, then `eret`s to that VA with X0=&MpInfo. Publish with
            // STLR (matches LDAR) and DC CVAC the line to PoC — required if an
            // AP briefly ran with D-cache off during trampoline bring-up.
            // Use a raw code address (not an fn-item temporary) so the stored
            // pointer is the higher-half `myos_smp_ap_entry` symbol.
            aarch64_publish_goto(*cpu, AP_ENTRY_PTR as *const () as usize, logical as u64);
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            cpu.bootstrap(AP_ENTRY_PTR, logical as u64);
        }
        next += 1;
    }

    let want = next;
    // Bound the wait so a broken handoff cannot hang CI forever, but give
    // QEMU TCG time to schedule parked APs through their LDAR/YIELD loop.
    let max_spins: u32 = 50_000_000;
    let mut spins = 0u32;
    while online_count() < want && spins < max_spins {
        core::hint::spin_loop();
        spins += 1;
        #[cfg(target_arch = "aarch64")]
        {
            if spins % 50_000 == 0 {
                unsafe {
                    core::arch::asm!("dsb ishst; sev; yield", options(nostack));
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

/// SIE-masked WFI forever. Used when we must silence Limine's AP busy-spin
/// without joining the scheduler (ONLINE stays false — no IPI / sscratch races).
#[cfg(target_arch = "riscv64")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn myos_smp_ap_park(_info: &limine::mp::MpInfo) -> ! {
    AP_PROGRESS.store(1, Ordering::SeqCst);
    unsafe {
        // Quiet park must make WFI *sleep*, not busy-spin. RISC-V WFI is allowed
        // to complete whenever an interrupt is *pending*, even with SIE clear.
        // Limine/OpenSBI often leave STIE + a pending timer (STIP) on secondary
        // harts — clearing only sstatus.SIE then turns this loop into another
        // satp-visible RAM hammer, which reopens the ripgrep sepc=0 expand flake
        // under QEMU -smp 2 (seen again on PR #153 tip after mm site tags).
        // Retire enables, push stimecmp to infinity (sstc), and clear SSIP.
        core::arch::asm!(
            "csrc sstatus, {sie_bit}",
            "csrw sie, zero",
            "csrw stimecmp, {far}",
            "csrc sip, {ssip}",
            sie_bit = in(reg) 1u64 << 1,
            far = in(reg) u64::MAX,
            ssip = in(reg) 1u64 << 1,
            options(nomem, nostack),
        );
    }
    // Publish "quiet" so BSP does not enter userspace while we still drain STIP.
    AP_PROGRESS.store(2, Ordering::SeqCst);
    loop {
        core::arch::asm!("wfi", options(nomem, nostack, preserves_flags));
    }
}

/// Limine aarch64 trampoline `eret`s here with X0=&MpInfo, SP=Limine stack,
/// DAIF masked, CPACR.FPEN=0, and VBAR cleared. A naked stub marks progress
/// and enables FP before any Rust prologue can touch NEON or the stack frame.
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn myos_smp_ap_entry(info: &limine::mp::MpInfo) -> ! {
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
        "b {rust}",
        flag = sym AP_PROGRESS,
        rust = sym myos_smp_ap_entry_rust,
    );
}

#[cfg(not(target_arch = "aarch64"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn myos_smp_ap_entry(info: &limine::mp::MpInfo) -> ! {
    unsafe { myos_smp_ap_entry_rust(info) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn myos_smp_ap_entry_rust(info: &limine::mp::MpInfo) -> ! {
    AP_PROGRESS.store(1, Ordering::SeqCst);
    // Mask IRQs until this CPU's IDT/timer are programmed.
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr daifset, #0xf", options(nomem, nostack));
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

/// Publish `goto_address` the way Limine's aarch64 trampoline observes it.
#[cfg(target_arch = "aarch64")]
fn aarch64_publish_goto(cpu: &limine::mp::MpInfo, entry: usize, extra: u64) {
    // Layout: processor_id(4)+res(4)+mpidr(8)+stack/reserved(8)+goto(8)+extra(8)
    let base = core::ptr::from_ref(cpu) as usize;
    let extra_ptr = (base + 32) as *mut u64;
    let goto_ptr = (base + 24) as *mut usize;
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

pub fn mark_offline(logical: usize) {
    if logical < MAX_CPUS {
        ONLINE[logical].store(false, Ordering::SeqCst);
        let mut cpus = CPUS.lock();
        cpus[logical].online = false;
    }
}

pub fn mark_running(logical: usize) {
    if logical < MAX_CPUS {
        // Catch up to the current shootdown epoch before advertising ONLINE so
        // an in-flight tlb_all_seen wait cannot hang on SEEN=0 forever.
        TLB_SEEN[logical].store(TLB_EPOCH.load(Ordering::SeqCst), Ordering::SeqCst);
        ONLINE[logical].store(true, Ordering::SeqCst);
        let mut cpus = CPUS.lock();
        cpus[logical].online = true;
    }
}
