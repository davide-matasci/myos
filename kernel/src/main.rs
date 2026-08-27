#![no_std]
#![no_main]
#![cfg_attr(target_arch = "x86_64", feature(abi_x86_interrupt))]

extern crate alloc;

mod arch;
mod font;
mod framebuffer;
mod heap;
mod limine_boot;
mod modules;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt::Write;
use core::panic::PanicInfo;

use arch::SerialPort;

const HELLO: &str = "Hello from myos";

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    kernel_main()
}

fn kernel_main() -> ! {
    arch::early_init();

    let mut serial = SerialPort::new();
    let _ = writeln!(serial, "{HELLO}");
    let _ = limine_boot::base_revision_supported();
    let _ = limine_boot::DTB.response();

    if let Some(resp) = limine_boot::FRAMEBUFFER.response() {
        if let Some(fb) = resp.framebuffers().first() {
            let mut writer = framebuffer::FrameBufferWriter::from_limine(fb);
            writer.clear();
            let _ = writeln!(writer, "{HELLO}");
        }
    }

    heap::init();
    prove_heap(&mut serial);

    let _ = writeln!(serial, "irq init");
    arch::init_interrupts();
    let _ = writeln!(serial, "irq wait");
    arch::wait_for_interrupt_proof();
    let _ = writeln!(serial, "int ok");

    modules::load_embedded_hello();

    serial.flush();
    arch::exit_qemu(arch::QEMU_SUCCESS);
    arch::halt();
}

fn prove_heap(serial: &mut SerialPort) {
    let boxed = Box::new(41u32);
    let mut v = Vec::new();
    v.push(*boxed + 1);
    let _ = boxed;
    let _ = v;
    let _ = writeln!(serial, "heap ok");
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let mut serial = SerialPort::new();
    let _ = writeln!(serial, "panic: {info}");
    serial.flush();
    arch::exit_qemu(arch::QEMU_FAILURE);
    arch::halt();
}
