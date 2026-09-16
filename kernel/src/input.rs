//! Stdin: serial + keyboard (when detected), shared ring buffer.
//!
//! Keyboard bytes are keycode→character via the loadable [`crate::keymap`]
//! (empty until userspace loads a map). Serial is unaffected.
//!
//! Line discipline follows the console termios (`ICANON` / `ECHO` / `ISIG` /
//! `ICRNL`). The discipline core (termios + edit/ring processing) lives in
//! [`crate::tty`] and is shared with pty slave inputs; this module owns the
//! console instance and its hardware drain paths. Default is **canonical**
//! (cooked): printable bytes accumulate in
//! a private edit buffer and are not visible to `read` until newline. Backspace
//! / DEL erase the last edit column (and the console glyph) without ever
//! delivering `0x08` to userspace. That matches oksh's non-`x_init` path, which
//! expects the kernel line discipline to resolve erase before `shf_getse`.
//!
//! When userspace clears `ICANON` via `TCSETS` / `tcsetattr` (vim raw/cbreak),
//! bytes are pushed straight into the readable ring — including ESC — so TUIs
//! get non-canonical input.

use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use spin::Mutex;

use crate::arch;
use crate::console;
use crate::task;

/// Lock-free UART RX staging drained from timer IRQs on every CPU.
/// Without this, a starved shell under `-smp 4` TCG can miss COM1 FIFO bytes
/// (`which ls` → `which s` on CI #34824642315).
const IRQ_RING: usize = 256;
static IRQ_HEAD: AtomicUsize = AtomicUsize::new(0);
static IRQ_TAIL: AtomicUsize = AtomicUsize::new(0);
static IRQ_BUF: [AtomicU8; IRQ_RING] = {
    const ZERO: AtomicU8 = AtomicU8::new(0);
    [ZERO; IRQ_RING]
};

/// The console tty: one input line-discipline instance (termios + rings).
pub const TERMIOS_LEN: usize = crate::tty::TERMIOS_LEN;

static TTY: Mutex<crate::tty::TtyIn> = Mutex::new(crate::tty::TtyIn::new());

pub fn init() {
    IRQ_HEAD.store(0, Ordering::SeqCst);
    IRQ_TAIL.store(0, Ordering::SeqCst);
    *TTY.lock() = crate::tty::TtyIn::new();
    arch::serial_flush_rx();
    arch::keyboard_init();
    DRAIN_ENABLED.store(true, Ordering::Relaxed);
}

pub fn termios_get_bytes() -> [u8; crate::tty::TERMIOS_LEN] {
    TTY.lock().termios.as_bytes()
}

pub fn termios_set_bytes(buf: &[u8; crate::tty::TERMIOS_LEN]) {
    TTY.lock().set_termios(crate::tty::Termios::from_bytes(buf));
}

/// Serializes hardware UART RX across timer IRQs and `poll` (multi-CPU TCG
/// otherwise races two `inb(COM1)` and drops chars — e.g. `root` → `oot`).
static UART_RX_LOCK: Mutex<()> = Mutex::new(());
/// Set true from `init` so early timer ticks (before stdin setup) no-op.
static DRAIN_ENABLED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Drain UART hardware into the lock-free IRQ staging ring.
///
/// Safe from timer IRQs on any CPU (`try_lock` — never spins in IRQ). `poll` /
/// `read` fold these bytes into the cooked/raw discipline. Prevents COM1 FIFO
/// overrun when the interactive shell's CPU is starved under `-smp 4` TCG.
/// Returns true if at least one UART byte was staged (caller may kick the
/// console reader's CPU so ECHO is not delayed under `-smp` TCG).
pub fn drain_uart_irq() -> bool {
    if !DRAIN_ENABLED.load(Ordering::Relaxed) {
        return false;
    }
    let Some(_guard) = UART_RX_LOCK.try_lock() else {
        return false;
    };
    let mut got = false;
    while let Some(b) = arch::serial_read_byte() {
        got = true;
        let h = IRQ_HEAD.load(Ordering::Relaxed);
        let next = (h + 1) % IRQ_RING;
        if next == IRQ_TAIL.load(Ordering::Acquire) {
            // Staging full — drop oldest by advancing tail so fresh input wins.
            let old_t = IRQ_TAIL.load(Ordering::Relaxed);
            IRQ_TAIL.store((old_t + 1) % IRQ_RING, Ordering::Release);
        }
        IRQ_BUF[h].store(b, Ordering::Relaxed);
        IRQ_HEAD.store(next, Ordering::Release);
    }
    got
}

fn fold_irq_rx() {
    loop {
        let t = IRQ_TAIL.load(Ordering::Acquire);
        if t == IRQ_HEAD.load(Ordering::Acquire) {
            break;
        }
        let b = IRQ_BUF[t].load(Ordering::Relaxed);
        IRQ_TAIL.store((t + 1) % IRQ_RING, Ordering::Release);
        push_byte(b);
    }
}

/// Drain UART and keyboard into the ring (call with interrupts enabled).
pub fn poll() {
    // Fold IRQ-staged bytes, then drain UART under the same lock (no parallel
    // `inb` vs timer). Keyboard stays polled here only.
    fold_irq_rx();
    drain_uart_irq();
    fold_irq_rx();
    while let Some(b) = arch::keyboard_poll_byte() {
        push_byte(b);
    }
}

/// Console echo sink: every discipline-shown byte lands on the console.
/// Called with the TTY lock held; the console lock never re-enters the tty.
fn console_echo(b: u8) {
    console::write_byte(b);
}

fn push_byte(raw: u8) {
    let vintr = {
        let mut t = TTY.lock();
        t.push_raw(raw, &mut console_echo)
    };
    if vintr {
        // Console foreground group (signal.rs resolves reader/last-input pgid).
        crate::signal::handle_ctrl_c();
    }
}

pub fn read(buf: &mut [u8]) -> usize {
    crate::signal::enter_input_read();
    let mut n = 0;
    while n == 0 {
        // Pending fatal/actionable signal: break so deliver_due can exit.
        if crate::signal::current_should_wake() {
            break;
        }
        poll();
        while n < buf.len() {
            let Some(b) = TTY.lock().pop() else {
                break;
            };
            buf[n] = b;
            n += 1;
        }
        if n == 0 {
            if crate::signal::current_should_wake() {
                break;
            }
            task::yield_now();
        }
    }
    crate::signal::leave_input_read();
    // If we woke for a signal with no bytes, still return 0 so the syscall
    // path can run `deliver_due` and terminate with 128+sig.
    n
}

pub fn keyboard_present() -> bool {
    arch::keyboard_present()
}

