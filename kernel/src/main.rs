#![no_std]
#![no_main]
#![cfg_attr(target_arch = "x86_64", feature(abi_x86_interrupt))]

extern crate alloc;

mod arch;
#[cfg(target_arch = "x86_64")]
mod font;
#[cfg(target_arch = "x86_64")]
mod framebuffer;
mod heap;
mod modules;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt::Write;
use core::panic::PanicInfo;

use arch::SerialPort;

const HELLO: &str = "Hello from myos";

#[cfg(target_arch = "x86_64")]
const BOOTLOADER_CONFIG: bootloader_api::BootloaderConfig = {
    let mut config = bootloader_api::BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(bootloader_api::config::Mapping::Dynamic);
    config
};

#[cfg(target_arch = "x86_64")]
bootloader_api::entry_point!(kernel_main_x86, config = &BOOTLOADER_CONFIG);

#[cfg(target_arch = "x86_64")]
fn kernel_main_x86(boot_info: &'static mut bootloader_api::BootInfo) -> ! {
    let mut serial = SerialPort::new();
    let _ = writeln!(serial, "{HELLO}");

    if let Some(fb) = boot_info.framebuffer.as_mut() {
        let mut writer = framebuffer::FrameBufferWriter::new(fb);
        writer.clear();
        let _ = writeln!(writer, "{HELLO}");
    } else {
        let _ = writeln!(serial, "no framebuffer");
    }

    arch::init_paging(boot_info);
    heap::init();
    prove_heap(&mut serial);

    arch::init_interrupts();
    arch::wait_for_interrupt_proof();
    let _ = writeln!(serial, "int ok");

    modules::load_embedded_hello();

    serial.flush();
    arch::exit_qemu(arch::QEMU_SUCCESS);
    arch::halt();
}

/// Called from `_start` in `arch/aarch64/boot.rs`.
#[cfg(target_arch = "aarch64")]
pub(crate) fn kernel_main_aarch64() -> ! {
    let mut serial = SerialPort::new();
    let _ = writeln!(serial, "{HELLO}");

    arch::init_paging();
    heap::init();
    prove_heap(&mut serial);

    arch::init_interrupts();
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
