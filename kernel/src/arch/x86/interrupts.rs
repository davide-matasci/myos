//! IDT + local APIC timer (x2APIC) or 8259 PIC + PIT.
//!
//! Limine base rev 5 leaves the local APIC enabled, LINT0 and the timer LVT
//! masked, and IOAPIC ExtINT masked, so PIC IRQ0 never arrives. Clearing
//! IA32_APIC_BASE.EN also #GPs in x2APIC mode and does not reconnect the PIC
//! on QEMU. Drive the APIC timer through x2APIC MSRs instead (APIC MMIO at
//! 0xFEE00000 is not in the base-rev-3+ HHDM).

use core::sync::atomic::{AtomicBool, Ordering};
use pic8259::ChainedPics;
use spin::{Mutex, Once};
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use super::gdt;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = 40;
const TIMER_VECTOR: u8 = 32;
const SPURIOUS_VECTOR: u8 = 0xFF;

const IA32_APIC_BASE: u32 = 0x1B;
const X2APIC_TPR: u32 = 0x808;
const X2APIC_EOI: u32 = 0x80B;
const X2APIC_SVR: u32 = 0x80F;
const X2APIC_LVT_TIMER: u32 = 0x832;
const X2APIC_INIT_COUNT: u32 = 0x838;
const X2APIC_DIV: u32 = 0x83E;
const APIC_EN: u64 = 1 << 11;
const APIC_EXTD: u64 = 1 << 10;

static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });
static IDT: Once<InterruptDescriptorTable> = Once::new();
static TIMER_FIRED: AtomicBool = AtomicBool::new(false);
static USE_X2APIC: AtomicBool = AtomicBool::new(false);

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
    let r = unsafe { core::arch::x86_64::__cpuid(1) };
    r.ecx & (1 << 21) != 0
}

fn enable_x2apic_timer() {
    let mut base = rdmsr(IA32_APIC_BASE);
    base |= APIC_EN;
    wrmsr(IA32_APIC_BASE, base);
    base |= APIC_EXTD;
    wrmsr(IA32_APIC_BASE, base);

    wrmsr(X2APIC_SVR, 0x1FF);
    wrmsr(X2APIC_TPR, 0);
    // Divide by 1, periodic, vector 32.
    wrmsr(X2APIC_DIV, 0xB);
    wrmsr(X2APIC_LVT_TIMER, u64::from(TIMER_VECTOR) | (1 << 17));
    wrmsr(X2APIC_INIT_COUNT, 1_000_000);
    USE_X2APIC.store(true, Ordering::SeqCst);
}

fn disable_apic_then_pic() {
    // Must leave x2APIC (clear EXTD, keep EN) before clearing EN.
    let mut base = rdmsr(IA32_APIC_BASE);
    if base & APIC_EXTD != 0 {
        base &= !APIC_EXTD;
        wrmsr(IA32_APIC_BASE, base);
    }
    base &= !APIC_EN;
    wrmsr(IA32_APIC_BASE, base);

    unsafe {
        let mut pics = PICS.lock();
        pics.initialize();
        pics.write_masks(0b1111_1110, 0b1111_1111);
    }
    init_pit();
}

fn init_pit() {
    const DIVISOR: u16 = 11932;
    unsafe {
        core::arch::asm!("out dx, al", in("dx") 0x43_u16, in("al") 0x34_u8, options(nomem, nostack, preserves_flags));
        core::arch::asm!("out dx, al", in("dx") 0x40_u16, in("al") (DIVISOR as u8), options(nomem, nostack, preserves_flags));
        core::arch::asm!("out dx, al", in("dx") 0x40_u16, in("al") ((DIVISOR >> 8) as u8), options(nomem, nostack, preserves_flags));
    }
}

pub fn init() {
    super::gdt::init();

    let idt = IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt.general_protection_fault.set_handler_fn(general_protection);
        idt.page_fault.set_handler_fn(page_fault);
        idt[TIMER_VECTOR].set_handler_fn(timer);
        idt[SPURIOUS_VECTOR].set_handler_fn(spurious);
        idt
    });
    idt.load();

    if has_x2apic() {
        enable_x2apic_timer();
    } else {
        disable_apic_then_pic();
    }
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
    if USE_X2APIC.load(Ordering::SeqCst) {
        wrmsr(X2APIC_LVT_TIMER, u64::from(TIMER_VECTOR) | (1 << 16));
        wrmsr(X2APIC_EOI, 0);
    } else {
        unsafe {
            PICS.lock().notify_end_of_interrupt(TIMER_VECTOR);
        }
    }
}
