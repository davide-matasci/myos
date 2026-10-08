#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(target_arch = "x86_64", feature(abi_x86_interrupt))]

extern crate alloc;

mod acpi;
mod arch;
mod blk;
mod console;
// Its accessors are used by the aarch64 / riscv64 arch code only.
#[allow(dead_code)]
mod dt;
mod exception;
mod fs;
mod heap;
mod input;
mod irq;
mod limine_boot;
/// Optional Linux syscall compatibility layer (`--features linux_compat`).
mod personality;
mod mm;
mod pci;
mod modules;
mod pipe;
mod platform;
mod power;
mod rng;
mod sec;
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

/// Entered from `arch::_start` once Limine has handed over.
pub(crate) fn kernel_main() -> ! {
    arch::early_init();
    // The board description comes first: device bases, interrupt routing
    // and clocks are read from it (aarch64, riscv64), nothing is assumed.
    // The ACPI tables fill it, then the device tree (`platform`).
    dt::init();
    platform::init();
    if let Err(what) = arch::apply_platform(platform::get()) {
        console::write_str("fatal: ");
        console::write_str(what);
        console::write_str("\n");
        arch::halt();
    }

    let mut fb_w = 0usize;
    let mut fb_h = 0usize;
    if let Some(resp) = limine_boot::FRAMEBUFFER.response() {
        if let Some(fb) = resp.framebuffers().first() {
            fb_w = fb.width as usize;
            fb_h = fb.height as usize;
            console::set_framebuffer(myos_abi::FramebufferInfo {
                addr: fb.address() as u64,
                width: fb.width,
                height: fb.height,
                pitch: fb.pitch,
                bpp: fb.bpp,
                r_shift: fb.red_mask_shift,
                g_shift: fb.green_mask_shift,
                b_shift: fb.blue_mask_shift,
                r_size: fb.red_mask_size,
                g_size: fb.green_mask_size,
                b_size: fb.blue_mask_size,
            });
        }
    }

    console::write_banner(HELLO);
    console::write_str("\n");
    if fb_w != 0 {
        // Keep this on serial so CI logs show the GOP or VBE size.
        let mut buf = [0u8; 64];
        let n = fb_geom_msg(&mut buf, fb_w, fb_h);
        console::write_str(core::str::from_utf8(&buf[..n]).unwrap_or("fb?\n"));
    }
    let _ = limine_boot::base_revision_supported();
    if let Some(model) = platform::get().model {
        console::write_str("board: ");
        console::write_str(model.value);
        console::write_str("\n");
    }

    heap::init();
    prove_heap();

    // The sources that described the board; a component the tree describes
    // differently from the tables is a firmware bug worth a line.
    let p = platform::get();
    console::status_ok(&alloc::format!("platform: {}", platform::sources_text(p)));
    if p.differs != 0 {
        console::status_warn(&alloc::format!(
            "platform: acpi and dt differ on {}",
            platform::differs_text(p)
        ));
    }

    // Calibrate the monotonic clock first (x86: TSC vs PIT, port I/O only):
    // the x86 LAPIC tick period is derived from it.
    time::init();
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
    rng::init();
    console::status_ok("urandom");
    // Every driver and filesystem is a module, loaded from the initramfs in
    // the order of /lib/modules/boot.list (console, hello, pci_enum, acpi,
    // virtio_blk, nvme, xhci, usb_hub, usb_storage, virtio_net, netfs, fat,
    // ext2). More can follow at runtime with `insmod` from /lib/modules.
    modules::load_boot_modules();
    // /msg lives on rootfs; /ok mounts /dev/vda as fat at /tmp/fat.
    let _ = fs::register("rootfs", "msg", MSG_OK);
    console::status_ok("fat message");

    // Who may do what (docs/security.md): read before the first process.
    sec::init();
    user::init();
    input::init();
    if input::keyboard_present() {
        console::write_info(&alloc::format!(
            "\nstdin: {} + serial. Output is mirrored to the screen.\n\n",
            crate::arch::KEYBOARD_NAME
        ));
    } else {
        console::write_info(
            "\nstdin: serial (x86 COM1 38400 8N1, AArch64 PL011). \
             Output is mirrored to the screen when a framebuffer exists.\n\
             Connect USB-serial if no keyboard was detected.\n\
             UTM SE: add virtio-keyboard-device in QEMU settings.\n\n",
        );
    }
    user::spawn_init();
    // kernel_main is the BSP's idle task from here on: run whatever is Ready
    // for CPU 0, halt until the next interrupt otherwise.
    task::become_idle();
    while !user::both_exited() {
        task::idle_step();
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
