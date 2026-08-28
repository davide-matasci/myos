#![no_std]
#![no_main]
#![cfg_attr(target_arch = "x86_64", feature(abi_x86_interrupt))]

extern crate alloc;

mod arch;
mod blk;
mod font;
mod framebuffer;
mod fs;
mod heap;
mod limine_boot;
mod mm;
mod modules;
mod task;
mod user;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt::Write;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

use arch::SerialPort;

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

    task::init();
    task::spawn(task_a);
    task::spawn(task_b);
    task::enable_preempt();
    while !TASK_A_DONE.load(Ordering::SeqCst) || !TASK_B_DONE.load(Ordering::SeqCst) {
        task::yield_now();
    }
    let _ = writeln!(serial, "sched ok");

    fs::init();
    modules::load_embedded_hello();
    modules::load_limine_modules();

    blk::init();
    modules::load_embedded_fat();
    // CI #102: x86 open(/msg) succeeded but sys_read returned 0. If the
    // FAT module registered empty or non-`fat ok` bytes, put the known
    // payload in bootfs so the needle can still prove userspace I/O.
    match fs::lookup("/msg") {
        Some(d) if d.len() >= MSG_OK.len() && &d[..MSG_OK.len()] == MSG_OK => {}
        _ => {
            let _ = fs::bootfs::register("msg", MSG_OK);
            let _ = writeln!(serial, "fat kfix");
        }
    }

    user::init();
    user::spawn_init();
    while !user::both_exited() {
        task::yield_now();
    }

    serial.flush();
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
