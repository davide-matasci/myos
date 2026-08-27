//! IDT + 8259 PIC + PIT. A timer IRQ is the "int ok" proof.

use core::sync::atomic::{AtomicBool, Ordering};
use pic8259::ChainedPics;
use spin::{Mutex, Once};
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

use super::gdt;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = 40;

static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });
static IDT: Once<InterruptDescriptorTable> = Once::new();
static TIMER_FIRED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
#[repr(u8)]
enum Irq {
    Timer = PIC_1_OFFSET,
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
        idt[Irq::Timer as u8].set_handler_fn(timer);
        idt
    });
    idt.load();

    unsafe {
        let mut pics = PICS.lock();
        pics.initialize();
        // Unmask only IRQ0 (PIT). 1 = masked.
        pics.write_masks(0b1111_1110, 0b1111_1111);
    }
    init_pit();
    x86_64::instructions::interrupts::enable();
}

pub fn wait_for_interrupt_proof() {
    while !TIMER_FIRED.load(Ordering::SeqCst) {
        x86_64::instructions::hlt();
    }
}

extern "x86-interrupt" fn breakpoint(_frame: InterruptStackFrame) {}

extern "x86-interrupt" fn double_fault(frame: InterruptStackFrame, _code: u64) -> ! {
    panic!("double fault: {frame:?}");
}

extern "x86-interrupt" fn timer(_frame: InterruptStackFrame) {
    TIMER_FIRED.store(true, Ordering::SeqCst);
    unsafe {
        PICS.lock().notify_end_of_interrupt(Irq::Timer as u8);
    }
}

fn init_pit() {
    // Channel 0, lobyte/hibyte, mode 2 (rate generator). ~100 Hz.
    const DIVISOR: u16 = 11932;
    unsafe {
        core::arch::asm!("out dx, al", in("dx") 0x43_u16, in("al") 0x34_u8, options(nomem, nostack, preserves_flags));
        core::arch::asm!("out dx, al", in("dx") 0x40_u16, in("al") (DIVISOR as u8), options(nomem, nostack, preserves_flags));
        core::arch::asm!("out dx, al", in("dx") 0x40_u16, in("al") ((DIVISOR >> 8) as u8), options(nomem, nostack, preserves_flags));
    }
}
