#![no_std]
#![no_main]
#![cfg_attr(target_arch = "x86_64", feature(abi_x86_interrupt))]

extern crate alloc;

mod console;
mod arch;
mod blk;
mod font;
mod framebuffer;
mod fs;
mod heap;
mod input;
mod limine_boot;
mod mm;
mod modules;
mod task;
mod user;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

const HELLO: &str = "Hello from myos";
const MSG_OK: &[u8] = b"fat ok\n";

static TASK_A_DONE: AtomicBool = AtomicBool::new(false);
static TASK_B_DONE: AtomicBool = AtomicBool::new(false);

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

    console::write_str(HELLO);
    console::write_str("\n");
    let _ = limine_boot::base_revision_supported();
    let _ = limine_boot::DTB.response();

    heap::init();
    prove_heap();

    console::write_str("irq init\n");
    arch::init_interrupts();
    console::write_str("irq wait\n");
    arch::wait_for_interrupt_proof();
    console::write_str("int ok\n");

    task::init();
    task::spawn(task_a);
    task::spawn(task_b);
    task::enable_preempt();
    while !TASK_A_DONE.load(Ordering::SeqCst) || !TASK_B_DONE.load(Ordering::SeqCst) {
        task::yield_now();
    }
    console::write_str("sched ok\n");

    fs::init();
    modules::load_embedded_hello();
    modules::load_limine_modules();

    blk::init();
    modules::load_embedded_fat();
    // CI #104: FAT copied 7 bytes (`fat n7`) and kfix did not fire, but
    // user/ok still read n==0. Re-point `/msg` at kernel rodata so the
    // vnode is not the module's leaked heap slice.
    let _ = fs::bootfs::register("msg", MSG_OK);
    console::write_str("fat kreg\n");

    user::init();
    input::init();
    console::write_str(
        "\nSerial console: shell I/O (x86 COM1 38400 8N1, AArch64 PL011).\n\
         Connect a USB-serial adapter; the monitor shows a copy of serial output.\n\n",
    );
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
    console::write_str("heap ok\n");
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    console::write_str(&alloc::format!("panic: {info}\n"));
    console::flush();
    arch::exit_qemu(arch::QEMU_FAILURE);
    arch::halt();
}
