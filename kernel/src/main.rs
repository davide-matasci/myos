#![no_std]
#![no_main]
#![cfg_attr(target_arch = "x86_64", feature(abi_x86_interrupt))]

extern crate alloc;

mod arch;
mod blk;
mod console;
mod exception;
mod font;
mod framebuffer;
mod fs;
mod heap;
mod input;
mod limine_boot;
mod mm;
mod nvme;
mod pci;
mod modules;
mod pipe;
mod task;
mod time;
mod user;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

const HELLO: &str = "Hello from myos";
const MSG_OK: &[u8] = b"fat ok\n";

static TASK_A_DONE: AtomicBool = AtomicBool::new(false);
static TASK_B_DONE: AtomicBool = AtomicBool::new(false);

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    r#"
    .section .text._start,"ax",@progbits
    .globl _start
_start:
    call {main}
    "#,
    main = sym kernel_main_riscv64,
);

#[cfg(target_arch = "riscv64")]
#[unsafe(no_mangle)]
extern "C" fn kernel_main_riscv64() -> ! {
    kernel_main()
}

#[cfg(not(target_arch = "riscv64"))]
#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    kernel_main()
}

fn kernel_main() -> ! {
    arch::early_init();

    if let Some(resp) = limine_boot::FRAMEBUFFER.response() {
        if let Some(fb) = resp.framebuffers().first() {
            let mut writer = framebuffer::FrameBufferWriter::from_limine(fb);
            writer.clear();
            console::init_fb(writer);
        }
    }

    console::write_banner(HELLO);
    console::write_str("\n");
    let _ = limine_boot::base_revision_supported();
    let _ = limine_boot::DTB.response();

    heap::init();
    prove_heap();

    console::status_progress("interrupts");
    arch::init_interrupts();
    arch::wait_for_interrupt_proof();
    console::status_ok("interrupts");

    task::init();
    task::spawn(task_a);
    task::spawn(task_b);
    task::enable_preempt();
    while !TASK_A_DONE.load(Ordering::SeqCst) || !TASK_B_DONE.load(Ordering::SeqCst) {
        task::yield_now();
    }
    console::status_ok("scheduler");

    fs::init();
    fs::init_limine();
    modules::load_embedded_stubfs();
    modules::load_embedded_hello();
    modules::load_limine_modules();

    blk::init();
    nvme::init();
    modules::load_embedded_virtio_net();
    modules::load_embedded_netfs();
    modules::load_embedded_fat();
    modules::load_embedded_ext2();
    // /msg lives on bootfs; /ok mounts /dev/vda as fat at /fat.
    let _ = fs::register("bootfs", "msg", MSG_OK);
    console::status_ok("fat message");

    user::init();
    input::init();
    if input::keyboard_present() {
        #[cfg(target_arch = "x86_64")]
        console::write_info(
            "\nstdin: PS/2 keyboard + serial (COM1 38400 8N1). \
             Output is mirrored to the screen.\n\n",
        );
        #[cfg(target_arch = "aarch64")]
        console::write_info(
            "\nstdin: virtio keyboard + serial (PL011). \
             Output is mirrored to the screen.\n\n",
        );
    } else {
        console::write_info(
            "\nstdin: serial (x86 COM1 38400 8N1, AArch64 PL011). \
             Output is mirrored to the screen when a framebuffer exists.\n\
             Connect USB-serial if no keyboard was detected.\n\
             UTM SE: add virtio-keyboard-device in QEMU settings.\n\n",
        );
    }
    user::spawn_init();
    while !user::both_exited() {
        task::yield_now();
    }

    console::flush();
    arch::exit_qemu(arch::QEMU_SUCCESS);
    arch::halt();
}

fn task_a() {
    task::print("task a\n");
    TASK_A_DONE.store(true, Ordering::SeqCst);
}

fn task_b() {
    task::print("task b\n");
    TASK_B_DONE.store(true, Ordering::SeqCst);
}

fn prove_heap() {
    let boxed = Box::new(41u32);
    let mut v = Vec::new();
    v.push(*boxed + 1);
    let _ = boxed;
    let _ = v;
    console::status_ok("heap");
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    console::status_fail(&alloc::format!("panic: {info}"));
    console::flush();
    arch::exit_qemu(arch::QEMU_FAILURE);
    arch::halt();
}
