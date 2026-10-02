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
pub(crate) static AP_PROGRESS: AtomicUsize = AtomicUsize::new(0);

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

/// The hardware id of this CPU: from the arch where it can read one, else
/// the id Limine reported for the logical index in the CPU id register.
fn hw_cpu_id() -> u64 {
    if let Some(id) = arch::hw_cpu_id() {
        return id;
    }
    let reg = arch::cpu_id_reg();
    if reg < MAX_CPUS && ONLINE[reg].load(Ordering::SeqCst) {
        let id = HW_IDS[reg].load(Ordering::SeqCst);
        if id != 0 {
            return id;
        }
    }
    HW_IDS[0].load(Ordering::SeqCst)
}

/// Logical CPU index for the caller (0 = BSP).
pub fn cpu_id() -> usize {
    // The per-CPU id register (`tp` / `TPIDR_EL1` / `IA32_TSC_AUX`) holds
    // the logical index once `set_cpu_id_reg` ran at BSP init / AP entry;
    // before that it is out of range and the hardware id is matched below.
    let reg = arch::cpu_id_reg();
    if reg < MAX_CPUS {
        return reg;
    }
    let hw = hw_cpu_id();
    for i in 0..MAX_CPUS {
        if ONLINE[i].load(Ordering::SeqCst) && HW_IDS[i].load(Ordering::SeqCst) == hw {
            return i;
        }
    }
    BOOT_CPU.load(Ordering::SeqCst)
}

/// The BSP's logical index (0), for `arch::sync_cpu_id_reg`.
#[allow(dead_code)]
pub fn boot_cpu() -> usize {
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


#[allow(dead_code)]
pub fn ipi_mark_tlb() {
    IPI_BITS.fetch_or(IPI_BIT_TLB, Ordering::SeqCst);
}

#[allow(dead_code)]
pub fn ipi_mark_resched() {
    IPI_BITS.fetch_or(IPI_BIT_RESCHED, Ordering::SeqCst);
}

#[allow(dead_code)]
pub fn ipi_is_tlb() -> bool {
    IPI_BITS.load(Ordering::SeqCst) & IPI_BIT_TLB != 0
}

#[allow(dead_code)]
pub fn ipi_is_resched() -> bool {
    IPI_BITS.load(Ordering::SeqCst) & IPI_BIT_RESCHED != 0
}

#[allow(dead_code)]
fn ipi_clear_handled() {
    // Clear both; senders re-set before each blast. Slightly coarse but safe.
    IPI_BITS.store(0, Ordering::SeqCst);
}

fn flush_tlb_local() {
    arch::flush_tlb_local();
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
    let _ = TLB_SEEN[cpu].try_update(Ordering::SeqCst, Ordering::SeqCst, |s| {
        if s >= epoch {
            None
        } else {
            Some(epoch)
        }
    });
    IPI_BITS.fetch_and(!IPI_BIT_TLB, Ordering::SeqCst);
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
    ipi_mark_tlb();
    arch::ipi_tlb_shootdown();
    let mut spins = 0u32;
    while !tlb_all_seen(epoch) && spins < 2_000_000 {
        // Soft-ACK path for peers stuck briefly cli in schedule/wait.
        tlb_service();
        core::hint::spin_loop();
        spins += 1;
    }
    ipi_clear_handled();
}

/// Wake one halted CPU (reschedule IPI) so it notices a newly-ready task.
pub fn kick_cpu(cpu: usize) {
    if cpu >= MAX_CPUS || !ONLINE[cpu].load(Ordering::SeqCst) || cpu == cpu_id() {
        return;
    }
    ipi_mark_resched();
    arch::ipi_reschedule_cpu(cpu);
}

/// Wake idle CPUs so they notice newly-ready tasks.
pub fn kick_cpus() {
    if online_count() <= 1 {
        return;
    }
    ipi_mark_resched();
    arch::ipi_reschedule();
}

pub fn cpuinfo_text() -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::new();
    let n = online_count();
    let arch_name = arch::NAME;
    push_str(&mut out, "processor_count: ");
    push_dec(&mut out, n as u64);
    push_str(&mut out, "\narch: ");
    push_str(&mut out, arch_name);
    push_str(&mut out, "\nscheduler: smp-rr\nblocked_tasks: ");
    push_dec(&mut out, crate::task::blocked_count() as u64);
    push_str(&mut out, "\nclock_hz: ");
    push_dec(&mut out, crate::time::clock_hz());
    push_str(&mut out, "\nuptime_ms: ");
    push_dec(&mut out, crate::time::monotonic_ns() / 1_000_000);
    push_str(&mut out, "\n");
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
        push_str(&mut out, "\nidle_halts\t: ");
        push_dec(&mut out, crate::task::idle_halts(i));
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
static AP_ENTRY_PTR: unsafe extern "C" fn(&limine::mp::MpInfo) -> ! = arch::ap_entry;


/// Record BSP and bring secondary CPUs online via Limine MP.
pub fn init() {
    arch::set_cpu_id_reg(0);

    // Where the arch cannot read its own hardware id (riscv64 S-mode), take
    // the BSP id Limine reports.
    let hw = arch::hw_cpu_id()
        .or_else(|| limine_boot::MP.response().map(arch::mp_bsp_id))
        .unwrap_or(0);
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

    let mut next = 1usize;
    for cpu in mp_cpus.iter() {
        let hw_id = arch::mp_cpu_id(cpu);
        if hw_id == arch::mp_bsp_id(resp) {
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
        arch::ap_bootstrap(cpu, AP_ENTRY_PTR, logical as u64);
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
        if spins % 50_000 == 0 {
            arch::ap_wait_poke();
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
    if got > 1 {
        arch::enable_ipi();
    }
}

/// Rust side of AP entry: `arch::ap_entry` (the symbol Limine jumps to)
/// masks interrupts and lands here with Limine's `MpInfo`.
pub(crate) unsafe extern "C" fn ap_entry_rust(info: &limine::mp::MpInfo) -> ! {
    AP_PROGRESS.store(2, Ordering::SeqCst);

    let logical = info.extra_argument() as usize;
    AP_PROGRESS.store(3 + logical, Ordering::SeqCst);
    if logical == 0 || logical >= MAX_CPUS {
        loop {
            crate::arch::wait_interrupt();
        }
    }

    arch::set_cpu_id_reg(logical);

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
        // Catch up to the current shootdown epoch before advertising ONLINE so
        // an in-flight tlb_all_seen wait cannot hang on SEEN=0 forever.
        TLB_SEEN[logical].store(TLB_EPOCH.load(Ordering::SeqCst), Ordering::SeqCst);
        ONLINE[logical].store(true, Ordering::SeqCst);
        let mut cpus = CPUS.lock();
        cpus[logical].online = true;
    }
}
