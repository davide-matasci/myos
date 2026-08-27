//! IDT + x2APIC timer.
//!
//! Limine base rev 5 leaves the local APIC enabled, LINT0/timer LVT masked,
//! and IOAPIC ExtINT masked, so PIC IRQ0 never arrives. CI #49 showed QEMU's
//! default `qemu64` CPU does not even advertise x2APIC (CPUID.1.ECX[21]);
//! CI #50 then silently fell back to PIC and hung after "irq wait". The
//! launcher now passes `-cpu qemu64,+x2apic`. APIC MMIO is not in the
//! base-rev-3+ HHDM, so the timer is programmed through x2APIC MSRs.

use core::arch::x86_64::__cpuid;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Once;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use super::gdt;

const TIMER_VECTOR: u8 = 32;
const SPURIOUS_VECTOR: u8 = 0xFF;

const IA32_APIC_BASE: u32 = 0x1B;
const X2APIC_TPR: u32 = 0x808;
const X2APIC_EOI: u32 = 0x80B;
const X2APIC_SVR: u32 = 0x80F;
const X2APIC_LVT_TIMER: u32 = 0x832;
const X2APIC_LVT_LINT0: u32 = 0x835;
const X2APIC_LVT_LINT1: u32 = 0x836;
const X2APIC_INIT_COUNT: u32 = 0x838;
const X2APIC_DIV: u32 = 0x83E;
const APIC_EN: u64 = 1 << 11;
const APIC_EXTD: u64 = 1 << 10;

static IDT: Once<InterruptDescriptorTable> = Once::new();
static TIMER_FIRED: AtomicBool = AtomicBool::new(false);

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

fn has_x2apic() -> bool {
    let r = unsafe { __cpuid(1) };
    r.ecx & (1 << 21) != 0
}

fn enable_x2apic_timer() {
    let mut base = rdmsr(IA32_APIC_BASE);
    base |= APIC_EN;
    wrmsr(IA32_APIC_BASE, base);
    base |= APIC_EXTD;
    wrmsr(IA32_APIC_BASE, base);
    unsafe {
        core::arch::asm!("mfence", options(nomem, nostack, preserves_flags));
    }

    wrmsr(X2APIC_SVR, 0x100 | u64::from(SPURIOUS_VECTOR));
    wrmsr(X2APIC_TPR, 0);
    wrmsr(X2APIC_LVT_LINT0, 1 << 16);
    wrmsr(X2APIC_LVT_LINT1, 1 << 16);
    // Divide by 1, periodic, vector 32.
    wrmsr(X2APIC_DIV, 0xB);
    wrmsr(X2APIC_LVT_TIMER, u64::from(TIMER_VECTOR) | (1 << 17));
    wrmsr(X2APIC_INIT_COUNT, 100_000);
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

    if !has_x2apic() {
        panic!("x2APIC required (QEMU needs -cpu qemu64,+x2apic)");
    }
    enable_x2apic_timer();
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
    wrmsr(X2APIC_LVT_TIMER, u64::from(TIMER_VECTOR) | (1 << 16));
    wrmsr(X2APIC_EOI, 0);
}
