//! Stdin: serial + keyboard (when detected), shared ring buffer.
//!
//! Keyboard bytes come from the `console` module (keycode→character via its
//! loadable keymap, empty until userspace loads a map). Serial is unaffected.
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

/// Lock-free UART RX staging, filled from interrupt context: the UART's
/// receive interrupt, or CPU 0's tick where the UART has none. Without
/// this, a starved shell under `-smp 4` TCG can miss COM1 FIFO bytes
/// (`which ls` → `which s` on CI #34824642315).
const IRQ_RING: usize = 256;
static IRQ_HEAD: AtomicUsize = AtomicUsize::new(0);
static IRQ_TAIL: AtomicUsize = AtomicUsize::new(0);
static IRQ_BUF: [AtomicU8; IRQ_RING] = {
    const ZERO: AtomicU8 = AtomicU8::new(0);
    [ZERO; IRQ_RING]
};

/// The console tty: one input line-discipline instance (termios + rings).
static TTY: Mutex<crate::tty::TtyIn> = Mutex::new(crate::tty::TtyIn::new());

pub fn init() {
    IRQ_HEAD.store(0, Ordering::SeqCst);
    IRQ_TAIL.store(0, Ordering::SeqCst);
    *TTY.lock() = crate::tty::TtyIn::new();
    arch::serial_flush_rx();
    DRAIN_ENABLED.store(true, Ordering::Relaxed);
    let routed = arch::serial_irq().is_some_and(|irq| crate::irq::enable(irq, "uart", uart_interrupt, 0));
    if routed {
        arch::serial_rx_irq_on();
        UART_IRQ.store(true, Ordering::SeqCst);
    }
}

/// The UART interrupts on received data; without, CPU 0's tick drains it
/// ([`tick`]) and CPU 0 keeps ticking while idle (`task::sched`).
static UART_IRQ: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn uart_irq() -> bool {
    UART_IRQ.load(Ordering::Relaxed)
}

/// The UART's receive interrupt (CPU 0): stage its bytes and wake the
/// console reader. The wake comes even when another CPU holds the drain
/// lock: that CPU's reader or poller then goes round once more and takes
/// what arrived since its drain.
unsafe extern "C" fn uart_interrupt(_ctx: *mut core::ffi::c_void) {
    drain_uart_irq();
    task::wake(task::KEY_CONSOLE);
}

/// CPU 0's timer tick, while the UART has no interrupt: stage its bytes so
/// a starved shell CPU cannot overrun the FIFO (bios `which ls`→`which s`
/// under -smp 4 TCG). When bytes land, wake the console reader (it blocks
/// on KEY_CONSOLE, possibly on another CPU): ECHO only runs from that
/// reader's poll — without a wake, host echo-sync waits then resends,
/// sticky-keying `login: rroooo…` / `roootttt…`.
pub fn tick() {
    if !uart_irq() && drain_uart_irq() {
        task::wake(task::KEY_CONSOLE);
    }
}

pub fn termios() -> crate::tty::Termios {
    TTY.lock().termios
}

pub fn set_termios(t: crate::tty::Termios) {
    TTY.lock().set_termios(t);
}

/// `flush in` on `/dev/console/ctl`: drop the pending input. The console
/// has no output buffer to flush.
pub fn flush_input() {
    TTY.lock().flush_input();
}

/// Serializes hardware UART RX across timer IRQs and `poll` (multi-CPU TCG
/// otherwise races two `inb(COM1)` and drops chars — e.g. `root` → `oot`).
static UART_RX_LOCK: Mutex<()> = Mutex::new(());
/// Set true from `init` so early timer ticks (before stdin setup) no-op.
static DRAIN_ENABLED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Drain UART hardware into the lock-free IRQ staging ring.
///
/// Safe from interrupts on any CPU (`try_lock` — never spins in IRQ). `poll` /
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
    // `inb` vs the interrupt). Interrupts off meanwhile: the UART's
    // interrupt on this CPU would find the lock held and leave a
    // level-triggered line asserted, re-entering until this drain is done,
    // which it then never gets to finish.
    fold_irq_rx();
    let flags = arch::irq_save();
    arch::irq_off();
    drain_uart_irq();
    arch::irq_restore(flags);
    fold_irq_rx();
    while let Some(b) = console::keyboard_poll_byte() {
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
    // Readers (possibly another task than the one polling) and pollers.
    task::wake(task::KEY_CONSOLE);
    task::wake_any();
    if vintr {
        // Console foreground group (signal.rs resolves reader/last-input pgid).
        crate::signal::handle_ctrl_c();
    }
}

pub fn read(buf: &mut [u8]) -> usize {
    crate::signal::enter_input_read();
    let mut n = 0;
    // A keyboard without an interrupt is polled: a reader re-polls it at a
    // modest rate. Serial bytes and an interrupting keyboard wake
    // KEY_CONSOLE.
    let keyboard = console::keyboard_present() && !console::keyboard_irq();
    while n == 0 {
        // A signal that terminates or is caught ends the wait (EINTR).
        if crate::signal::interrupt_wait() {
            break;
        }
        let seq = task::wait_seq();
        poll();
        while n < buf.len() {
            let Some(b) = TTY.lock().pop() else {
                break;
            };
            buf[n] = b;
            n += 1;
        }
        if n == 0 {
            if crate::signal::interrupt_wait() {
                break;
            }
            let deadline = if keyboard { task::deadline_ms(10) } else { 0 };
            task::block_until(task::KEY_CONSOLE, seq, deadline);
        }
    }
    crate::signal::leave_input_read();
    // Woken by a signal with no bytes: 0 here, turned into EINTR (or a
    // restart, or the task's termination) on the way out of the syscall.
    n
}

/// A read would not block: committed input, or a pending end-of-file (`^D`).
/// Drains the UART and keyboard first, as a read does.
pub fn readable() -> bool {
    poll();
    let t = TTY.lock();
    t.available() > 0 || t.eof
}

pub fn keyboard_present() -> bool {
    console::keyboard_present()
}

