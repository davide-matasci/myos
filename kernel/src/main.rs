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
mod kbd;
mod keymap;
mod limine_boot;
mod mm;
mod nvme;
mod pci;
mod modules;
mod pipe;
mod signal;
mod smp;
mod pty;
mod task;
mod tty;
mod time;
mod user;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

const HELLO: &str = "Hello from myos";
const MSG_OK: &[u8] = b"fat-msg\n";

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


fn fb_geom_msg(buf: &mut [u8], w: usize, h: usize) -> usize {
    // "fb WxH\n"
    let mut i = 0;
    for b in b"fb " {
        buf[i] = *b; i += 1;
    }
    i = push_usize(buf, i, w);
    buf[i] = b'x'; i += 1;
    i = push_usize(buf, i, h);
    buf[i] = b'\n'; i += 1;
    i
}

fn push_usize(buf: &mut [u8], mut i: usize, mut v: usize) -> usize {
    let mut tmp = [0u8; 20];
    let mut n = 0;
    if v == 0 {
        tmp[0] = b'0';
        n = 1;
    } else {
        while v > 0 {
            tmp[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
    }
    while n > 0 {
        n -= 1;
        buf[i] = tmp[n];
        i += 1;
    }
    i
}

fn kernel_main() -> ! {
    arch::early_init();

    let mut fb_w = 0usize;
    let mut fb_h = 0usize;
    if let Some(resp) = limine_boot::FRAMEBUFFER.response() {
        if let Some(fb) = resp.framebuffers().first() {
            fb_w = fb.width as usize;
            fb_h = fb.height as usize;
            let mut writer = framebuffer::FrameBufferWriter::from_limine(fb);
            writer.clear();
            console::init_fb(writer);
        }
    }

    console::write_banner(HELLO);
    console::write_str("\n");
    if fb_w != 0 {
        // Keep this on serial so CI logs show GOP vs VBE size + mirror mode.
        let mut buf = [0u8; 64];
        let n = fb_geom_msg(&mut buf, fb_w, fb_h);
        console::write_str(core::str::from_utf8(&buf[..n]).unwrap_or("fb?\n"));
        if console::mirrors_bytes() {
            console::write_str("fb mirror=on\n");
        } else {
            console::write_str("fb mirror=off\n");
        }
    }
    let _ = limine_boot::base_revision_supported();
    let _ = limine_boot::DTB.response();

    heap::init();
    prove_heap();

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
    // Print after both tasks finish so concurrent status_* cannot garble serial/FB.
    console::status_info("task a");
    console::status_info("task b");
    console::status_ok("scheduler");

    smp::init();
    // Prove cross-CPU scheduling: spawn workers that record cpu_id.
    smp_smoke();
    // APs stay online into userspace (per-CPU TSS / stacks / IPIs).
    for _ in 0..1000 {
        task::yield_now();
    }

    fs::init();
    fs::init_limine();
    modules::load_embedded_stubfs();
    modules::load_embedded_hello();
    modules::load_embedded_pci_enum();
    modules::load_embedded_acpi();
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

fn smp_smoke() {
    use core::sync::atomic::{AtomicU64, Ordering};
    static SEEN: AtomicU64 = AtomicU64::new(0);
    static STOP: AtomicU64 = AtomicU64::new(0);
    for _ in 0..4 {
        task::spawn(|| {
            // Stay runnable until every online CPU has been observed, so APs
            // get a chance before BSP drains the ready list.
            while STOP.load(Ordering::SeqCst) == 0 {
                let id = smp::cpu_id() as u64;
                SEEN.fetch_or(1u64 << id, Ordering::SeqCst);
                let need = smp::online_count().min(2) as u32;
                if SEEN.load(Ordering::SeqCst).count_ones() >= need {
                    break;
                }
                task::yield_now();
            }
        });
    }
    for _ in 0..50_000 {
        task::yield_now();
        let bits = SEEN.load(Ordering::SeqCst);
        if bits.count_ones() as usize >= smp::online_count().min(2) {
            break;
        }
    }
    STOP.store(1, Ordering::SeqCst);
    for _ in 0..1_000 {
        task::yield_now();
    }
    let bits = SEEN.load(Ordering::SeqCst);
    console::status_info(&alloc::format!(
        "smp sched mask={bits:#x} cpus={} ticks0={} ticks1={}",
        smp::online_count(),
        smp::sched_ticks(0),
        smp::sched_ticks(1),
    ));
}

fn task_a() {
    TASK_A_DONE.store(true, Ordering::SeqCst);
}

fn task_b() {
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
