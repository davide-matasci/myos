//! IDT + the local APIC timer, IPIs and MSI-X vectors.
//!
//! The local APIC is driven in x2APIC mode (its registers are MSRs
//! 0x800..) when the CPU has one (CPUID.01H:ECX[21]: every PC of the last
//! fifteen years), in xAPIC mode (the registers memory-mapped at the
//! IA32_APIC_BASE address, mapped here at HHDM+phys) otherwise. QEMU's TCG
//! has no x2APIC ("TCG doesn't support requested feature:
//! CPUID.01H:ECX.x2apic", CI #51), so CI runs the xAPIC path; real hardware
//! takes the other. PIC IRQ0 never arrives under Limine: the timer is the
//! LAPIC's.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::Once;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use super::gdt;
use crate::limine_boot;

const TIMER_VECTOR: u8 = 32;
const IPI_TLB_VECTOR: u8 = 33;
const IPI_RESCHED_VECTOR: u8 = 34;
const SPURIOUS_VECTOR: u8 = 0xFF;
const IA32_APIC_BASE: u32 = 0x1B;
const IA32_TSC_AUX: u32 = 0xC000_0103;
const APIC_EN: u64 = 1 << 11;
const APIC_EXTD: u64 = 1 << 10;

const SVR: u32 = 0xF0;
const TPR: u32 = 0x80;
const EOI: u32 = 0xB0;
const ICR_LOW: u32 = 0x300;
const LVT_TIMER: u32 = 0x320;
const LVT_LINT0: u32 = 0x350;
const LVT_LINT1: u32 = 0x360;
const INIT_COUNT: u32 = 0x380;
const CUR_COUNT: u32 = 0x390;
const DIV: u32 = 0x3E0;

static IDT: Once<InterruptDescriptorTable> = Once::new();
static TIMER_FIRED: AtomicBool = AtomicBool::new(false);
/// The xAPIC registers' virtual address (0 in x2APIC mode).
static LAPIC: AtomicUsize = AtomicUsize::new(0);
/// x2APIC mode: registers through MSRs, decided once by the BSP.
static X2APIC: AtomicBool = AtomicBool::new(false);
/// The first x2APIC MSR; register `off` of the xAPIC page is MSR
/// `X2APIC_MSR_BASE + off / 16`.
const X2APIC_MSR_BASE: u32 = 0x800;

#[repr(align(4096))]
struct Table([u64; 512]);

static mut SCRATCH: [Table; 3] = [Table([0; 512]), Table([0; 512]), Table([0; 512])];
static mut SCRATCH_USED: usize = 0;

fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") lo,
            in("edx") hi,
            options(nostack, preserves_flags),
        );
    }
}

fn cr3_phys() -> u64 {
    let c: u64;
    unsafe {
        core::arch::asm!("mov {c}, cr3", c = out(reg) c, options(nomem, nostack, preserves_flags));
    }
    c & !0xfff
}

fn table_at(phys: u64) -> *mut [u64; 512] {
    (limine_boot::hhdm_offset() + phys) as *mut [u64; 512]
}

fn alloc_table() -> (u64, *mut [u64; 512]) {
    unsafe {
        let i = SCRATCH_USED;
        SCRATCH_USED += 1;
        assert!(i < 3, "lapic map: out of scratch page tables");
        let p = core::ptr::addr_of_mut!(SCRATCH[i]);
        let phys = limine_boot::kernel_virt_to_phys(p as usize);
        (phys, core::ptr::addr_of_mut!((*p).0))
    }
}

fn ensure(entry: &mut u64) -> *mut [u64; 512] {
    if *entry & 1 != 0 {
        assert!(*entry & (1 << 7) == 0, "lapic map: huge page in the way");
        return table_at(*entry & !0xfff);
    }
    let (phys, ptr) = alloc_table();
    *entry = phys | 0b11;
    ptr
}

fn map_lapic(phys: u64) -> usize {
    let virt = limine_boot::hhdm_offset() + phys;
    let i4 = ((virt >> 39) & 0x1ff) as usize;
    let i3 = ((virt >> 30) & 0x1ff) as usize;
    let i2 = ((virt >> 21) & 0x1ff) as usize;
    let i1 = ((virt >> 12) & 0x1ff) as usize;
    unsafe {
        let pml4 = table_at(cr3_phys());
        let pdpt = ensure(&mut (*pml4)[i4]);
        let pd = ensure(&mut (*pdpt)[i3]);
        let pt = ensure(&mut (*pd)[i2]);
        // present, writable, PWT, PCD, NX: uncacheable MMIO
        (*pt)[i1] = phys | 0b11 | (1 << 3) | (1 << 4) | (1u64 << 63);
        core::arch::asm!("invlpg [{v}]", v = in(reg) virt, options(nostack, preserves_flags));
    }
    virt as usize
}

fn lapic_w(off: u32, val: u32) {
    if X2APIC.load(Ordering::Relaxed) {
        wrmsr(X2APIC_MSR_BASE + off / 16, u64::from(val));
        return;
    }
    let b = LAPIC.load(Ordering::SeqCst);
    unsafe {
        core::ptr::write_volatile((b + off as usize) as *mut u32, val);
    }
}

/// Enable this CPU's local APIC, in x2APIC mode when the BSP chose it, and
/// map the xAPIC registers otherwise (once: every CPU sees the same page).
fn enable_lapic() {
    let mut base = rdmsr(IA32_APIC_BASE) | APIC_EN;
    if X2APIC.load(Ordering::Relaxed) {
        base |= APIC_EXTD;
        wrmsr(IA32_APIC_BASE, base);
        return;
    }
    base &= !APIC_EXTD;
    wrmsr(IA32_APIC_BASE, base);
    if LAPIC.load(Ordering::SeqCst) == 0 {
        let va = map_lapic(base & 0xffff_f000);
        LAPIC.store(va, Ordering::SeqCst);
    }
}

pub fn init() {
    super::gdt::init();

    let idt = IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint);
        idt.page_fault.set_handler_fn(page_fault);
        idt.general_protection_fault.set_handler_fn(general_protection);
        // The exceptions a userspace instruction can raise that would
        // otherwise hit a non-present IDT gate and escalate to a double
        // fault (a `ud2` from EL0 took the whole machine down). Each kills
        // the faulting task when it came from ring 3, like #GP/#PF.
        idt.divide_error.set_handler_fn(divide_error);
        idt.debug.set_handler_fn(debug_exc);
        idt.overflow.set_handler_fn(overflow_exc);
        idt.bound_range_exceeded.set_handler_fn(bound_range);
        idt.invalid_opcode.set_handler_fn(invalid_opcode);
        idt.device_not_available.set_handler_fn(device_not_available);
        idt.x87_floating_point.set_handler_fn(x87_fp);
        idt.simd_floating_point.set_handler_fn(simd_fp);
        idt.invalid_tss.set_handler_fn(invalid_tss);
        idt.segment_not_present.set_handler_fn(segment_not_present);
        idt.stack_segment_fault.set_handler_fn(stack_segment);
        idt.alignment_check.set_handler_fn(alignment_check);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt[TIMER_VECTOR].set_handler_fn(timer);
        idt[IPI_TLB_VECTOR].set_handler_fn(ipi_tlb);
        idt[IPI_RESCHED_VECTOR].set_handler_fn(ipi_resched);
        idt[SPURIOUS_VECTOR].set_handler_fn(spurious);
        // Device vectors (MSI-X messages, I/O APIC pins): one IDT stub per
        // vector, all funnelled into `irq::dispatch`.
        let stubs: [extern "x86-interrupt" fn(InterruptStackFrame); DEVICE_VECTORS as usize] = [
            dev_irq0, dev_irq1, dev_irq2, dev_irq3, dev_irq4, dev_irq5, dev_irq6, dev_irq7, dev_irq8, dev_irq9,
            dev_irq10, dev_irq11, dev_irq12, dev_irq13, dev_irq14, dev_irq15,
        ];
        for (i, stub) in stubs.into_iter().enumerate() {
            idt[DEVICE_VECTOR_BASE + i as u8].set_handler_fn(stub);
        }
        idt
    });
    idt.load();

    wrmsr(IA32_TSC_AUX, 0);
    super::user::load_percpu_gs(0);

    X2APIC.store(super::cpu::has_x2apic(), Ordering::SeqCst);
    enable_lapic();
    crate::console::status_info(if X2APIC.load(Ordering::Relaxed) {
        "lapic: x2apic"
    } else {
        "lapic: xapic"
    });

    lapic_w(SVR, 0x100 | u32::from(SPURIOUS_VECTOR));
    lapic_w(TPR, 0);
    lapic_w(LVT_LINT0, 1 << 16);
    lapic_w(LVT_LINT1, 1 << 16);
    lapic_w(DIV, 0xB);
    // Measure first: the calibration reprograms LVT_TIMER (masked one-shot).
    let count = timer_init_count();
    // Periodic (bit 17). Do not mask after the first tick: preemption needs
    // the timer to keep firing.
    lapic_w(LVT_TIMER, u32::from(TIMER_VECTOR) | (1 << 17));
    lapic_w(INIT_COUNT, count);

    x86_64::instructions::interrupts::enable();
}

/// Scheduler tick rate: 100 Hz, as on aarch64 and riscv64 ([`timer_deadline`]
/// brings a sleep's wake-up forward). At 1 kHz the ticks and the vCPU exits
/// they cause cost a self-hosted core+alloc build 10-14% under TCG.
const TICK_HZ: u64 = 100;
/// Fallback when the TSC is not calibrated (no PIT): the historical
/// INIT_COUNT, "several kHz" in QEMU.
const FALLBACK_INIT_COUNT: u32 = 100_000;
static INIT_COUNT_CACHED: AtomicUsize = AtomicUsize::new(0);

/// LAPIC timer INIT_COUNT for [`TICK_HZ`], measured once on the BSP against
/// the calibrated TSC (`time::init` runs before interrupts come up). The old
/// fixed 100_000 fired 10-20k ticks per second per CPU under QEMU, and every
/// tick takes the scheduler lock. A tick also drains a UART without its
/// interrupt (`input::tick`): QEMU holds input back while the 16-byte FIFO
/// is full, so a slower drain delays typed input but loses none.
fn timer_init_count() -> u32 {
    let cached = INIT_COUNT_CACHED.load(Ordering::SeqCst);
    if cached != 0 {
        return cached as u32;
    }
    let count = if crate::time::clock_hz() == 0 {
        FALLBACK_INIT_COUNT
    } else {
        // One-shot, maximal count, masked: watch how far it counts in 20 ms.
        lapic_w(LVT_TIMER, u32::from(TIMER_VECTOR) | (1 << 16));
        lapic_w(INIT_COUNT, 0xFFFF_FFFF);
        let t0 = crate::time::monotonic_ns();
        while crate::time::monotonic_ns().wrapping_sub(t0) < 20_000_000 {
            core::hint::spin_loop();
        }
        let elapsed = u64::from(0xFFFF_FFFFu32 - lapic_r(CUR_COUNT));
        lapic_w(INIT_COUNT, 0);
        let per_tick = elapsed * 1000 / 20 / TICK_HZ;
        // Sanity: a tick needs at least a few thousand counts; cap at 2^31.
        if per_tick < 1_000 || per_tick > u64::from(u32::MAX / 2) {
            FALLBACK_INIT_COUNT
        } else {
            per_tick as u32
        }
    };
    INIT_COUNT_CACHED.store(count as usize, Ordering::SeqCst);
    count
}

/// The CPUs whose current timer period [`cut_period`] cut short.
static SHORTENED: [AtomicBool; crate::smp::MAX_CPUS] = [const { AtomicBool::new(false) }; crate::smp::MAX_CPUS];

/// Fire this CPU's timer at `deadline_ns` (monotonic) when that comes before
/// its next tick: the one LAPIC timer stays periodic, its current period is
/// cut to end at the deadline, and the tick puts the normal period back
/// (cutting it again for a deadline still ahead).
pub fn timer_deadline(deadline_ns: u64) {
    if crate::time::clock_hz() == 0 {
        return; // no calibrated count per ns: wake on the tick
    }
    x86_64::instructions::interrupts::without_interrupts(|| {
        cut_period(crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1), deadline_ns);
    });
}

/// Cut this CPU's current timer period to end at `deadline_ns` if it would
/// end later, but not under 1% of a tick: the period repeats until the tick
/// handler puts the normal one back, and a period of a few counts (a
/// deadline that just passed) kept QEMU's main loop firing the timer, which
/// starved the serial input. Interrupts off.
fn cut_period(cpu: usize, deadline_ns: u64) {
    let left_ns = deadline_ns.saturating_sub(crate::time::monotonic_ns());
    let per_tick = u128::from(timer_init_count());
    let counts = (u128::from(left_ns) * per_tick * u128::from(TICK_HZ) / 1_000_000_000).max(per_tick / 100);
    if counts < u128::from(lapic_r(CUR_COUNT)) {
        SHORTENED[cpu].store(true, Ordering::Relaxed);
        lapic_w(INIT_COUNT, counts as u32);
    }
}

/// The CPUs whose tick [`timer_idle`] stopped.
static TICK_STOPPED: [AtomicBool; crate::smp::MAX_CPUS] = [const { AtomicBool::new(false) }; crate::smp::MAX_CPUS];

/// This CPU halts with nothing to run: stop its periodic tick and fire its
/// timer once, at `wake_ns` (monotonic), but not under 1% of a tick from now
/// (see [`cut_period`]). The count is clamped to 32 bits: a fast LAPIC wakes
/// sooner than asked, never later. Interrupts off.
pub fn timer_idle(wake_ns: u64) {
    if crate::time::clock_hz() == 0 {
        return; // no calibrated count per ns: keep the tick
    }
    let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
    let left_ns = wake_ns.saturating_sub(crate::time::monotonic_ns());
    let per_tick = u128::from(timer_init_count());
    let counts = (u128::from(left_ns) * per_tick * u128::from(TICK_HZ) / 1_000_000_000)
        .clamp(per_tick / 100, u128::from(u32::MAX));
    SHORTENED[cpu].store(false, Ordering::Relaxed);
    TICK_STOPPED[cpu].store(true, Ordering::Relaxed);
    // One-shot (bit 17 clear); writing INIT_COUNT starts it.
    lapic_w(LVT_TIMER, u32::from(TIMER_VECTOR));
    lapic_w(INIT_COUNT, counts as u32);
}

/// The periodic tick back on this CPU if [`timer_idle`] stopped it.
/// Interrupts off.
pub fn timer_resume() {
    let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
    if !TICK_STOPPED[cpu].swap(false, Ordering::Relaxed) {
        return;
    }
    lapic_w(LVT_TIMER, u32::from(TIMER_VECTOR) | (1 << 17));
    lapic_w(INIT_COUNT, timer_init_count());
}

/// At a tick: the normal period back after a cut one, then cut again for
/// the earliest sleep deadline still ahead (`task::next_deadline_ns`).
fn rearm_period() {
    let cpu = crate::smp::cpu_id().min(crate::smp::MAX_CPUS - 1);
    if SHORTENED[cpu].swap(false, Ordering::Relaxed) {
        lapic_w(INIT_COUNT, timer_init_count());
    }
    let deadline = crate::task::next_deadline_ns();
    if deadline != u64::MAX && deadline > crate::time::monotonic_ns() {
        cut_period(cpu, deadline);
    }
}

/// Secondary CPU: IDT already built by BSP; enable this CPU's local APIC timer.
pub fn ap_init(logical: usize) {
    x86_64::instructions::interrupts::disable();
    if let Some(idt) = IDT.get() {
        idt.load();
    }
    super::gdt::load_for_ap(logical);
    wrmsr(IA32_TSC_AUX, logical as u64);
    super::user::load_percpu_gs(logical);

    enable_lapic();
    lapic_w(SVR, 0x100 | u32::from(SPURIOUS_VECTOR));
    lapic_w(TPR, 0);
    lapic_w(LVT_LINT0, 1 << 16);
    lapic_w(LVT_LINT1, 1 << 16);
    lapic_w(DIV, 0xB);
    let count = timer_init_count();
    lapic_w(LVT_TIMER, u32::from(TIMER_VECTOR) | (1 << 17));
    lapic_w(INIT_COUNT, count);
    // Leave IF clear — `task::ap_idle_loop` enables IRQs only after CURRENT
    // and the idle task exist (otherwise an early IPI/timer schedules with
    // CURRENT==0 and corrupts the BSP's task 0).
}

pub fn wait_for_interrupt_proof() {
    while !TIMER_FIRED.load(Ordering::SeqCst) {
        x86_64::instructions::hlt();
    }
}

extern "x86-interrupt" fn breakpoint(_frame: InterruptStackFrame) {}

/// First LAPIC vector handed to devices (PCI MSI-X, I/O APIC pins).
const DEVICE_VECTOR_BASE: u8 = 48;
const DEVICE_VECTORS: u8 = 16;
static NEXT_DEVICE_VECTOR: AtomicUsize = AtomicUsize::new(0);

/// A fresh device vector (its `irq::dispatch` number), or `None` when all
/// are taken.
pub fn alloc_vector() -> Option<u8> {
    let n = NEXT_DEVICE_VECTOR.fetch_add(1, Ordering::SeqCst);
    (n < DEVICE_VECTORS as usize).then(|| DEVICE_VECTOR_BASE + n as u8)
}

macro_rules! dev_stub {
    ($name:ident, $n:expr) => {
        extern "x86-interrupt" fn $name(_frame: InterruptStackFrame) {
            crate::irq::dispatch(u32::from(DEVICE_VECTOR_BASE) + $n);
            lapic_w(EOI, 0);
        }
    };
}
dev_stub!(dev_irq0, 0);
dev_stub!(dev_irq1, 1);
dev_stub!(dev_irq2, 2);
dev_stub!(dev_irq3, 3);
dev_stub!(dev_irq4, 4);
dev_stub!(dev_irq5, 5);
dev_stub!(dev_irq6, 6);
dev_stub!(dev_irq7, 7);
dev_stub!(dev_irq8, 8);
dev_stub!(dev_irq9, 9);
dev_stub!(dev_irq10, 10);
dev_stub!(dev_irq11, 11);
dev_stub!(dev_irq12, 12);
dev_stub!(dev_irq13, 13);
dev_stub!(dev_irq14, 14);
dev_stub!(dev_irq15, 15);

/// Program MSI-X table entry 0 of the PCI function to raise a fresh LAPIC
/// vector on the BSP, and enable MSI-X. Returns the vector (`irq::dispatch`
/// number). The caller still has to point the device's queue at entry 0
/// (virtio: `queue_msix_vector`).
pub fn pci_msix_setup(bus: u8, slot: u8, func: u8) -> Option<u32> {
    const CAP_MSIX: u32 = 0x11;
    let cfg = |off: u8| crate::pci::cfg_read32(bus, slot, func, off);
    let status = (cfg(4) >> 16) as u16;
    if status & 0x10 == 0 {
        return None;
    }
    let mut cap = (cfg(0x34) & 0xFC) as u8;
    let mut hops = 0;
    while cap != 0 && hops < 48 {
        hops += 1;
        let w0 = cfg(cap);
        if w0 & 0xFF == CAP_MSIX {
            break;
        }
        cap = ((w0 >> 8) & 0xFC) as u8;
    }
    if cap == 0 || cfg(cap) & 0xFF != CAP_MSIX {
        return None;
    }
    let table = cfg(cap.wrapping_add(4));
    let bir = (table & 0x7) as u8;
    let offset = (table & !0x7) as u64;
    let (va, size) = crate::pci::bar_map(bus, slot, func, bir)?;
    if offset.saturating_add(16) > size {
        return None;
    }
    // The message address names the destination in 8 bits: a BSP with a
    // larger x2APIC id would need interrupt remapping to be reached.
    let apic_id = crate::smp::cpu_hw_id(0) as u32;
    if apic_id > 0xFF {
        return None;
    }
    let vector = u32::from(alloc_vector()?);
    let entry = va + offset as usize;
    unsafe {
        // Mask the entry while programming it (vector control bit 0).
        core::ptr::write_volatile((entry + 12) as *mut u32, 1);
        core::ptr::write_volatile(entry as *mut u32, 0xFEE0_0000 | (apic_id << 12));
        core::ptr::write_volatile((entry + 4) as *mut u32, 0);
        core::ptr::write_volatile((entry + 8) as *mut u32, vector);
        core::ptr::write_volatile((entry + 12) as *mut u32, 0);
    }
    // Message control (cap+2): bit 15 MSI-X enable, bit 14 function mask.
    let w0 = cfg(cap);
    let ctrl = ((w0 >> 16) as u16 | 0x8000) & !0x4000;
    crate::pci::cfg_write32(bus, slot, func, cap, (w0 & 0xFFFF) | (u32::from(ctrl) << 16));
    Some(vector)
}

extern "x86-interrupt" fn spurious(_frame: InterruptStackFrame) {}

fn read_cr2() -> u64 {
    let cr2: u64;
    unsafe {
        core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack, preserves_flags));
    }
    cr2
}

extern "x86-interrupt" fn double_fault(frame: InterruptStackFrame, _code: u64) -> ! {
    super::exception::x86_double_fault(
        frame.instruction_pointer.as_u64(),
        frame.stack_pointer.as_u64(),
    );
}

/// A vector with no error code: kill the task if it faulted in ring 3.
macro_rules! user_exc {
    ($name:ident, $label:literal) => {
        extern "x86-interrupt" fn $name(frame: InterruptStackFrame) {
            let user = frame.code_segment.0 & 3 == 3;
            super::exception::x86_exception(
                $label,
                frame.instruction_pointer.as_u64(),
                frame.stack_pointer.as_u64(),
                0,
                user,
            );
        }
    };
}
/// A vector that pushes an error code.
macro_rules! user_exc_code {
    ($name:ident, $label:literal) => {
        extern "x86-interrupt" fn $name(frame: InterruptStackFrame, code: u64) {
            let user = frame.code_segment.0 & 3 == 3;
            super::exception::x86_exception(
                $label,
                frame.instruction_pointer.as_u64(),
                frame.stack_pointer.as_u64(),
                code,
                user,
            );
        }
    };
}
user_exc!(divide_error, "divide error");
user_exc!(debug_exc, "debug");
user_exc!(overflow_exc, "overflow");
user_exc!(bound_range, "bound range exceeded");
user_exc!(invalid_opcode, "invalid opcode");
user_exc!(device_not_available, "device not available");
user_exc!(x87_fp, "x87 floating point");
user_exc!(simd_fp, "simd floating point");
user_exc_code!(invalid_tss, "invalid tss");
user_exc_code!(segment_not_present, "segment not present");
user_exc_code!(stack_segment, "stack segment fault");
user_exc_code!(alignment_check, "alignment check");

extern "x86-interrupt" fn general_protection(frame: InterruptStackFrame, code: u64) {
    let user = frame.code_segment.0 & 3 == 3;
    // For a kernel #GP, the prologue's `push rbp` saved the faulting frame's
    // RBP at [rbp]; read it for SSE-alignment diagnosis. Not from ring 3: RBP
    // may still hold the user's value there (0 → a kernel page fault here).
    let mut saved_rbp: u64 = 0;
    if !user {
        unsafe {
            core::arch::asm!(
                "mov {}, [rbp]",
                out(reg) saved_rbp,
                options(nostack, preserves_flags),
            );
        }
    }
    super::exception::x86_general_protection(
        frame.instruction_pointer.as_u64(),
        frame.stack_pointer.as_u64(),
        code,
        saved_rbp,
        user,
    );
}

extern "x86-interrupt" fn page_fault(frame: InterruptStackFrame, code: PageFaultErrorCode) {
    // A user mmap page not touched yet (from userspace, or a kernel copy of
    // a user buffer): page it in and retry the access.
    let access = if code.contains(PageFaultErrorCode::INSTRUCTION_FETCH) {
        crate::user::Access::Exec
    } else if code.contains(PageFaultErrorCode::CAUSED_BY_WRITE) {
        crate::user::Access::Write
    } else {
        crate::user::Access::Read
    };
    // A page not mapped yet, or a store from userspace to a present page
    // the page cache shares read-only (`fault_in` copies it, or says it is
    // a real violation).
    let cow = code.contains(PageFaultErrorCode::USER_MODE) && access == crate::user::Access::Write;
    if (!code.contains(PageFaultErrorCode::PROTECTION_VIOLATION) || cow) && crate::user::fault_in(read_cr2() as usize, access) {
        return;
    }
    super::exception::x86_page_fault(
        read_cr2(),
        frame.instruction_pointer.as_u64(),
        frame.stack_pointer.as_u64(),
        code.bits() as u64,
        code.contains(PageFaultErrorCode::USER_MODE),
    );
}

fn lapic_r(off: u32) -> u32 {
    if X2APIC.load(Ordering::Relaxed) {
        return rdmsr(X2APIC_MSR_BASE + off / 16) as u32;
    }
    let b = LAPIC.load(Ordering::SeqCst);
    unsafe { core::ptr::read_volatile((b + off as usize) as *const u32) }
}

const ICR_HIGH: u32 = 0x310;

fn send_ipi_apic(apic_id: u32, vector: u8) {
    if X2APIC.load(Ordering::Relaxed) {
        // One 64-bit ICR: the destination in bits 63:32, fixed delivery,
        // physical mode, assert; no delivery-status bit to wait on.
        wrmsr(
            X2APIC_MSR_BASE + ICR_LOW / 16,
            (u64::from(apic_id) << 32) | u64::from(vector) | (1 << 14),
        );
        return;
    }
    if LAPIC.load(Ordering::SeqCst) == 0 {
        return;
    }
    while lapic_r(ICR_LOW) & (1 << 12) != 0 {
        core::hint::spin_loop();
    }
    // Destination in ICR high bits 31:24 (xAPIC).
    lapic_w(ICR_HIGH, apic_id << 24);
    // Fixed delivery, physical mode, assert.
    lapic_w(ICR_LOW, u32::from(vector) | (1 << 14));
    while lapic_r(ICR_LOW) & (1 << 12) != 0 {
        core::hint::spin_loop();
    }
}

fn send_ipi_others(vector: u8) {
    let self_id = crate::smp::cpu_id();
    for i in 0..crate::smp::MAX_CPUS {
        if i == self_id || !crate::smp::cpu_online(i) {
            continue;
        }
        let apic = crate::smp::cpu_hw_id(i) as u32;
        send_ipi_apic(apic, vector);
    }
}

pub fn ipi_tlb_shootdown() {
    send_ipi_others(IPI_TLB_VECTOR);
}

pub fn ipi_reschedule() {
    send_ipi_others(IPI_RESCHED_VECTOR);
}

/// Reschedule IPI to one logical CPU (wakes it from `hlt`).
pub fn ipi_reschedule_cpu(cpu: usize) {
    if crate::smp::cpu_online(cpu) {
        send_ipi_apic(crate::smp::cpu_hw_id(cpu) as u32, IPI_RESCHED_VECTOR);
    }
}

fn flush_tlb_local() {
    unsafe {
        core::arch::asm!(
            "mov {cr3}, cr3",
            "mov cr3, {cr3}",
            cr3 = out(reg) _,
            options(nostack, preserves_flags),
        );
    }
}

extern "x86-interrupt" fn ipi_tlb(_frame: InterruptStackFrame) {
    flush_tlb_local();
    crate::smp::tlb_ipi_ack();
    lapic_w(EOI, 0);
}

extern "x86-interrupt" fn ipi_resched(frame: InterruptStackFrame) {
    lapic_w(EOI, 0);
    crate::task::schedule();
    if frame.code_segment.0 & 3 == 3 {
        crate::signal::on_user_preempted();
    }
}

extern "x86-interrupt" fn timer(frame: InterruptStackFrame) {
    TIMER_FIRED.store(true, Ordering::SeqCst);
    crate::time::note_tick();
    crate::rng::stir_tick();
    // The BSP stages UART input when the UART has no interrupt.
    if crate::smp::cpu_id() == 0 {
        crate::input::tick();
    }
    crate::task::timer_tick();
    rearm_period();
    lapic_w(EOI, 0);
    crate::task::schedule();
    if frame.code_segment.0 & 3 == 3 {
        crate::signal::on_user_preempted();
    }
}
