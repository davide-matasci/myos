//! IDT + xAPIC timer via MMIO.
//!
//! TCG (GitHub Actions) does not implement x2APIC: CI #51 printed
//! "TCG doesn't support requested feature: CPUID.01H:ECX.x2apic".
//! PIC IRQ0 also never arrives under Limine. Map the local APIC
//! (phys from IA32_APIC_BASE) at HHDM+phys and program the xAPIC timer.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::Once;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use super::gdt;
use crate::limine_boot;

const TIMER_VECTOR: u8 = 32;
const SPURIOUS_VECTOR: u8 = 0xFF;
const IA32_APIC_BASE: u32 = 0x1B;
const APIC_EN: u64 = 1 << 11;
const APIC_EXTD: u64 = 1 << 10;

const SVR: u32 = 0xF0;
const TPR: u32 = 0x80;
const EOI: u32 = 0xB0;
const LVT_TIMER: u32 = 0x320;
const LVT_LINT0: u32 = 0x350;
const LVT_LINT1: u32 = 0x360;
const INIT_COUNT: u32 = 0x380;
const DIV: u32 = 0x3E0;

static IDT: Once<InterruptDescriptorTable> = Once::new();
static TIMER_FIRED: AtomicBool = AtomicBool::new(false);
static LAPIC: AtomicUsize = AtomicUsize::new(0);

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
            options(nomem, nostack, preserves_flags),
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
    let b = LAPIC.load(Ordering::SeqCst);
    unsafe {
        core::ptr::write_volatile((b + off as usize) as *mut u32, val);
    }
}

pub fn init() {
    super::gdt::init();

    let idt = IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint);
        idt.page_fault.set_handler_fn(page_fault);
        idt.general_protection_fault.set_handler_fn(general_protection);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt[TIMER_VECTOR].set_handler_fn(timer);
        idt[SPURIOUS_VECTOR].set_handler_fn(spurious);
        idt
    });
    idt.load();

    let mut base = rdmsr(IA32_APIC_BASE);
    base |= APIC_EN;
    base &= !APIC_EXTD;
    wrmsr(IA32_APIC_BASE, base);
    let phys = base & 0xffff_f000;
    let va = map_lapic(phys);
    LAPIC.store(va, Ordering::SeqCst);

    lapic_w(SVR, 0x100 | u32::from(SPURIOUS_VECTOR));
    lapic_w(TPR, 0);
    lapic_w(LVT_LINT0, 1 << 16);
    lapic_w(LVT_LINT1, 1 << 16);
    lapic_w(DIV, 0xB);
    // Periodic (bit 17). Do not mask after the first tick: preemption needs
    // the timer to keep firing. INIT_COUNT ~100_000 is several kHz in QEMU.
    lapic_w(LVT_TIMER, u32::from(TIMER_VECTOR) | (1 << 17));
    lapic_w(INIT_COUNT, 100_000);

    x86_64::instructions::interrupts::enable();
}

pub fn wait_for_interrupt_proof() {
    while !TIMER_FIRED.load(Ordering::SeqCst) {
        x86_64::instructions::hlt();
    }
}

extern "x86-interrupt" fn breakpoint(_frame: InterruptStackFrame) {}

extern "x86-interrupt" fn spurious(_frame: InterruptStackFrame) {}

extern "x86-interrupt" fn double_fault(frame: InterruptStackFrame, _code: u64) -> ! {
    panic!("double fault: {frame:?}");
}

extern "x86-interrupt" fn general_protection(frame: InterruptStackFrame, code: u64) {
    panic!("general protection: code={code:#x} {frame:?}");
}

extern "x86-interrupt" fn page_fault(frame: InterruptStackFrame, code: PageFaultErrorCode) {
    panic!("page fault: {code:?} {frame:?}");
}

extern "x86-interrupt" fn timer(_frame: InterruptStackFrame) {
    TIMER_FIRED.store(true, Ordering::SeqCst);
    lapic_w(EOI, 0);
    crate::task::schedule();
}
