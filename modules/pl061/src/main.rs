//! The ARM PL061 GPIO controller (QEMU `virt`'s, `arm,pl061`) as an
//! interrupt domain (`docs/pci-acpi-smp.md`, "Interrupt domains"): its 8
//! lines' interrupts, behind its one interrupt to the root controller,
//! are interrupt numbers of their own, which a driver of a device on a
//! line enables with `KernelApi::irq_enable` like any other. Register
//! layout: ARM's "PrimeCell GPIO (PL061) Technical Reference Manual".
//!
//! Nothing drives a line yet, so the probe checks the domain's path once:
//! it sets line [`CHECK_LINE`], an input nothing drives, to interrupt
//! while it is low, enables that interrupt through the kernel, and the
//! handler turns it off again. `/proc/interrupts` shows both counts
//! (`pl061`, `pl061 check`: `user/tests/kernel.sh`).

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use core::fmt::Write;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use myos_abi::{status_ok, status_warn, ApiCell, KernelApi, MmioDevice, ABI_VERSION};

static API: ApiCell = ApiCell::new();
/// The registers (kernel virtual).
static REGS: AtomicUsize = AtomicUsize::new(0);
/// The domain's first interrupt number: line `n` is `FIRST + n`.
static FIRST: AtomicU32 = AtomicU32::new(0);

const LINES: u32 = 8;
/// The line the probe interrupts on: QEMU `virt` wires line 3 (the power
/// key), nothing to line 7.
const CHECK_LINE: u32 = 7;

/// Direction (1: output), interrupt sense (1: level), both edges,
/// event (1: rising edge or high level), mask (1: enabled), masked status,
/// clear.
const GPIODIR: usize = 0x400;
const GPIOIS: usize = 0x404;
const GPIOIBE: usize = 0x408;
const GPIOIEV: usize = 0x40c;
const GPIOIE: usize = 0x410;
const GPIOMIS: usize = 0x418;
const GPIOIC: usize = 0x41c;

fn read(off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((REGS.load(Ordering::Relaxed) + off) as *const u32) }
}

fn write(off: usize, value: u32) {
    unsafe { core::ptr::write_volatile((REGS.load(Ordering::Relaxed) + off) as *mut u32, value) }
}

fn update(off: usize, set: u32, clear: u32) {
    write(off, (read(off) & !clear) | set);
}

/// The domain's `mask`: line `line`'s interrupt off.
unsafe extern "C" fn mask(_ctx: *mut core::ffi::c_void, line: u32) {
    if line < LINES {
        update(GPIOIE, 0, 1 << line);
    }
}

/// The domain's `unmask`: line `line`'s interrupt on.
unsafe extern "C" fn unmask(_ctx: *mut core::ffi::c_void, line: u32) {
    if line < LINES {
        update(GPIOIE, 1 << line, 0);
    }
}

/// The controller's own interrupt: each pending line's handlers. An edge
/// is cleared before them (another edge then interrupts again), a level
/// after (the handler has made its device let go of the line).
unsafe extern "C" fn demux(_ctx: *mut core::ffi::c_void) {
    let Some(api) = API.try_get() else {
        return;
    };
    let pending = read(GPIOMIS);
    let level = read(GPIOIS);
    write(GPIOIC, pending & !level);
    let first = FIRST.load(Ordering::Relaxed);
    for line in 0..LINES {
        if pending & (1 << line) != 0 {
            api.irq_dispatch(first + line);
        }
    }
    write(GPIOIC, pending & level);
}

/// The check's handler: [`CHECK_LINE`] interrupts no more (high now, and
/// masked).
unsafe extern "C" fn check(_ctx: *mut core::ffi::c_void) {
    update(GPIOIEV, 1 << CHECK_LINE, 0);
    update(GPIOIE, 0, 1 << CHECK_LINE);
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api: *const KernelApi) -> i32 {
    let Some(api) = (unsafe { api.as_ref() }) else {
        return -1;
    };
    if api.abi_version != ABI_VERSION {
        return -2;
    }
    unsafe { API.set(api) };
    let mut dev = MmioDevice::default();
    if api.dt_mmio_find("arm,pl061", 0, &mut dev) != 0 {
        api.write_str("pl061: none\n");
        return 0;
    }
    if dev.irq == 0 {
        status_warn(api, "pl061: its interrupt cannot be routed");
        return 0;
    }
    REGS.store(dev.base, Ordering::SeqCst);
    // Every line masked, nothing pending.
    write(GPIOIE, 0);
    write(GPIOIC, 0xff);
    let first = unsafe { api.irq_domain_register(dev.phandle, LINES, unmask, mask, core::ptr::null_mut()) };
    if first == 0 {
        status_warn(api, "pl061: no interrupt domain");
        return 0;
    }
    FIRST.store(first, Ordering::SeqCst);
    if api.irq_enable(dev.irq, "pl061", demux, core::ptr::null_mut()) != 0 {
        status_warn(api, "pl061: its interrupt cannot be routed");
        return 0;
    }
    // The check: an input interrupting while low, which it is (nothing
    // drives it), enabled through the kernel as a driver's would be.
    let bit = 1 << CHECK_LINE;
    update(GPIODIR, 0, bit);
    update(GPIOIBE, 0, bit);
    update(GPIOIEV, 0, bit);
    update(GPIOIS, bit, 0);
    if api.irq_enable(first + CHECK_LINE, "pl061 check", check, core::ptr::null_mut()) != 0 {
        status_warn(api, "pl061: its line's interrupt cannot be enabled");
    }
    let mut line = Line::new();
    let _ = write!(line, "pl061: {LINES} lines, interrupts {first}..{}", first + LINES - 1);
    status_ok(api, line.as_str());
    0
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

/// A status line, formatted without an allocator.
struct Line {
    buf: [u8; 64],
    len: usize,
}

impl Line {
    fn new() -> Line {
        Line { buf: [0; 64], len: 0 }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("pl061")
    }
}

impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

// So `cargo rustc --bin pl061` links. The kernel never jumps here.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
