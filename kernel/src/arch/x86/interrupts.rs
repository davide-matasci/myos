//! IDT + x2APIC timer. Limine leaves the LAPIC enabled, so 8259 PIC IRQs
//! never arrive (CI #46 hung after "heap ok" on BIOS and UEFI).

use core::arch::x86_64::__cpuid;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Once;
use x86_64::registers::model_specific::Msr;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use super::gdt;

const TIMER_VECTOR: u8 = 32;
const SPURIOUS_VECTOR: u8 = 0xFF;
const IA32_APIC_BASE: u32 = 0x1B;
const MSR_EOI: u32 = 0x80B;
const MSR_SVR: u32 = 0x80F;
const MSR_LVT_TIMER: u32 = 0x832;
const MSR_LVT_LINT0: u32 = 0x835;
const MSR_LVT_LINT1: u32 = 0x836;
const MSR_TMICT: u32 = 0x838;
const MSR_TMDCR: u32 = 0x83E;

static IDT: Once<InterruptDescriptorTable> = Once::new();
static TIMER_FIRED: AtomicBool = AtomicBool::new(false);

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

    if !init_x2apic_timer() {
        panic!("x2APIC required (Limine leaves PIC IRQs dead)");
    }
    x86_64::instructions::interrupts::enable();
}

pub fn wait_for_interrupt_proof() {
    while !TIMER_FIRED.load(Ordering::SeqCst) {
        x86_64::instructions::hlt();
    }
}

fn has_x2apic() -> bool {
    let r = unsafe { __cpuid(1) };
    r.ecx & (1 << 21) != 0
}

fn wrmsr(reg: u32, val: u64) {
    let mut msr = Msr::new(reg);
    unsafe {
        msr.write(val);
    }
}

fn init_x2apic_timer() -> bool {
    if !has_x2apic() {
        return false;
    }
    let mut base = Msr::new(IA32_APIC_BASE);
    unsafe {
        let v = base.read();
        base.write(v | (1 << 11) | (1 << 10));
    }
    // SVR: software-enable + spurious vector. LINT0/1 masked so PIC is idle.
    wrmsr(MSR_SVR, 0x100 | SPURIOUS_VECTOR as u64);
    wrmsr(MSR_LVT_LINT0, 1 << 16);
    wrmsr(MSR_LVT_LINT1, 1 << 16);
    wrmsr(MSR_TMDCR, 0b1011); // divide by 1
    // Periodic so a tick that expires before sti stays pending.
    wrmsr(MSR_LVT_TIMER, TIMER_VECTOR as u64 | (1 << 17));
    wrmsr(MSR_TMICT, 100_000);
    true
}

extern "x86-interrupt" fn breakpoint(_frame: InterruptStackFrame) {}

extern "x86-interrupt" fn spurious(_frame: InterruptStackFrame) {}

extern "x86-interrupt" fn page_fault(frame: InterruptStackFrame, code: PageFaultErrorCode) {
    panic!("page fault {code:?} {frame:?}");
}

extern "x86-interrupt" fn general_protection(frame: InterruptStackFrame, code: u64) {
    panic!("general protection {code:#x} {frame:?}");
}

extern "x86-interrupt" fn double_fault(frame: InterruptStackFrame, _code: u64) -> ! {
    panic!("double fault: {frame:?}");
}

extern "x86-interrupt" fn timer(_frame: InterruptStackFrame) {
    TIMER_FIRED.store(true, Ordering::SeqCst);
    wrmsr(MSR_LVT_TIMER, TIMER_VECTOR as u64 | (1 << 16));
    wrmsr(MSR_EOI, 0);
}
