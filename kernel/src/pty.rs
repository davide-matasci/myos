//! PTY pairs: `/dev/pts/N/master` + `/dev/pts/N/data` slave (a pair comes
//! from opening `/dev/pts/clone`; `/dev/pts/N/ctl` is its control file,
//! `docs/tty.md`), Linux-faithful model.
//!
//! One pair = one shared termios (Linux ptys share termios across both ends)
//! with the slave-side input line discipline ([`crate::tty::TtyIn`]) and an
//! output ring the master reads. Data flow:
//!
//! - master write  → slave input discipline (ICRNL/ISIG/canonical editing,
//!   echo back into the output ring) → slave `read`
//! - slave write   → output processing (OPOST/ONLCR) → output ring → master
//!   `read`
//!
//! End-of-session semantics (what Dropbear and `forkpty` consumers rely on):
//! - slave `read` with no open master → `EIO`; master `read` after the last
//!   slave fd closes → `EIO` once drained.
//! - closing the last master fd sends `SIGHUP` to the pty's session group.
//! - `ISIG` ^C on the slave input goes to the pty's session group (the
//!   console's ^C path is unchanged and stays in [`crate::input`]).
//!
//! The kernel's own terminals are pairs too, whose master it holds itself
//! (`docs/tty.md`): `/dev/tty1`…`/dev/tty9`, each on a text screen of the
//! console, typed at with the keyboard while its screen is shown, and
//! `/dev/ttyS0`, the serial port. Their output goes to the screen or the
//! UART instead of a ring a master reads ([`Sink`]); they hold the first
//! [`TERMS`] ids, so pair `N` of `/dev/pts` is id `TERMS + N`.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::signal::{SIGHUP, SIGINT};
use crate::tty::{CtlAction, TtyIn, OPOST, ONLCR};

const OUT_CAP: usize = 4096;

/// The kernel's terminals: id 0 is `ttyS0`, id `N` is `ttyN`.
pub const TERMS: usize = 10;
/// `ttyS0`'s id.
pub const SERIAL: usize = 0;

/// Where a pair's output (and echo) goes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sink {
    /// The ring its master reads: a `/dev/pts` pair.
    Master,
    /// Text screen `N` of the console: `ttyN`.
    Screen(usize),
    /// The serial port: `ttyS0`.
    Serial,
}

impl Sink {
    fn emit(self, bytes: &[u8]) {
        match self {
            Sink::Master => {}
            Sink::Screen(n) => crate::console::write_screen(n, bytes),
            Sink::Serial => crate::console::write_serial(bytes),
        }
    }
}

pub struct Pty {
    sink: Sink,
    /// Shared input discipline + pair termios (TCGETS/TCSETS on either end).
    term: Mutex<TtyIn>,
    /// Slave output (and echo) the master reads. head/tlen relative to cap.
    out: Mutex<OutRing>,
    winsize: Mutex<(u16, u16)>,
    master_refs: AtomicUsize,
    slave_refs: AtomicUsize,
    /// The session that made the pair its controlling terminal (TIOCSCTTY,
    /// `ctty`), its leader's pid. `usize::MAX` when unset; a claim lasts
    /// while the leader lives ([`crate::task::session_alive`]).
    session: AtomicUsize,
    /// How many times the pair changed (bytes either way, an end closed):
    /// what an `et` wait set member on either end is reported against
    /// (`crate::evfd`).
    changes: AtomicU64,
}

struct OutRing {
    buf: [u8; OUT_CAP],
    head: usize,
    len: usize,
}

impl OutRing {
    const fn new() -> Self {
        Self {
            buf: [0; OUT_CAP],
            head: 0,
            len: 0,
        }
    }
    fn push(&mut self, b: u8) -> bool {
        if self.len >= OUT_CAP {
            return false;
        }
        self.buf[(self.head + self.len) % OUT_CAP] = b;
        self.len += 1;
        true
    }
    fn take(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.len);
        for (i, o) in out.iter_mut().take(n).enumerate() {
            *o = self.buf[(self.head + i) % OUT_CAP];
        }
        self.head = (self.head + n) % OUT_CAP;
        self.len -= n;
        n
    }
}

/// The pairs by id, as many as are open (no fixed limit: each holds an fd,
/// so the fd limits bound them). A freed id is reused first.
static PTYS: Mutex<Vec<Option<Box<Pty>>>> = Mutex::new(Vec::new());

fn pty_at(id: usize) -> Option<&'static Pty> {
    let guard = PTYS.lock();
    // SAFETY: a pair is boxed (the table growing never moves it) and only
    // freed by replacing its slot with None, and callers hold a
    // reference-counted claim on the pair while using it.
    let ptr = &**guard.get(id)?.as_ref()? as *const Pty;
    drop(guard);
    Some(unsafe { &*ptr })
}

/// `pty` on the heap, or `None` when the heap is full (a failed `clone`,
/// not a kernel panic).
fn try_box(pty: Pty) -> Option<Box<Pty>> {
    let layout = core::alloc::Layout::new::<Pty>();
    // SAFETY: a fresh allocation of `Pty`'s layout, written before the Box
    // owns it.
    unsafe {
        let raw = alloc::alloc::alloc(layout) as *mut Pty;
        if raw.is_null() {
            return None;
        }
        raw.write(pty);
        Some(Box::from_raw(raw))
    }
}

/// Make the kernel's terminals ([`TERMS`]), before any pair: their master
/// is the kernel's for good.
pub fn init_terms() {
    let mut ptys = PTYS.lock();
    for id in 0..TERMS {
        let sink = if id == SERIAL { Sink::Serial } else { Sink::Screen(id) };
        let pty = Box::new(new_pty(sink));
        ptys.push(Some(pty));
    }
}

fn new_pty(sink: Sink) -> Pty {
    Pty {
        sink,
        term: Mutex::new(TtyIn::new()),
        out: Mutex::new(OutRing::new()),
        // A terminal's own size is the screen's (or the serial default):
        // (0, 0) until a `winsize` line sets it.
        winsize: Mutex::new(if sink == Sink::Master { (24, 80) } else { (0, 0) }),
        master_refs: AtomicUsize::new(1),
        slave_refs: AtomicUsize::new(0),
        session: AtomicUsize::new(usize::MAX),
        changes: AtomicU64::new(0),
    }
}

/// The directory a pair's files are in: `/dev/pts/N`, `/dev/ttyN` or
/// `/dev/ttyS0`.
pub fn dir(id: usize) -> alloc::string::String {
    match id {
        SERIAL => alloc::string::String::from("/dev/ttyS0"),
        1..TERMS => alloc::format!("/dev/tty{id}"),
        _ => alloc::format!("/dev/pts/{}", id - TERMS),
    }
}

/// The id of `/dev/pts/N`, if that pair exists.
pub fn pts_id(n: usize) -> Option<usize> {
    let id = n.checked_add(TERMS)?;
    slave_exists(id).then_some(id)
}

/// `/dev/ttyS0` has been opened (by its getty): from then on the serial
/// port is that terminal's, the console's output stays on its screen and
/// the UART's input goes to `ttyS0`.
static SERIAL_TAKEN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn serial_taken() -> bool {
    SERIAL_TAKEN.load(Ordering::Relaxed)
}

/// Allocate a pair. The caller owns the initial master reference.
pub fn alloc() -> Option<usize> {
    let pty = try_box(Pty {
        sink: Sink::Master,
        term: Mutex::new(TtyIn::new()),
        out: Mutex::new(OutRing::new()),
        winsize: Mutex::new((24, 80)),
        master_refs: AtomicUsize::new(1),
        slave_refs: AtomicUsize::new(0),
        session: AtomicUsize::new(usize::MAX),
        changes: AtomicU64::new(0),
    })?;
    let mut ptys = PTYS.lock();
    if let Some(id) = ptys.iter().position(Option::is_none) {
        ptys[id] = Some(pty);
        return Some(id);
    }
    ptys.try_reserve(1).ok()?;
    ptys.push(Some(pty));
    Some(ptys.len() - 1)
}

/// One past the highest pair id in use: the ids to look at.
pub fn id_bound() -> usize {
    PTYS.lock().len()
}

/// The live session that claimed pair `id`.
fn session_of(id: usize) -> Option<usize> {
    let sess = pty_at(id)?.session.load(Ordering::SeqCst);
    (sess != usize::MAX && crate::task::session_alive(sess)).then_some(sess)
}

fn session_pgid(id: usize) -> Option<usize> {
    crate::task::task_pgid(session_of(id)?)
}

fn free_if_dead(id: usize) {
    let mut ptys = PTYS.lock();
    let Some(slot) = ptys.get_mut(id) else { return };
    if slot
        .as_ref()
        .is_some_and(|p| p.master_refs.load(Ordering::SeqCst) == 0 && p.slave_refs.load(Ordering::SeqCst) == 0)
    {
        // Dropped after the lock: the pair is a few KiB.
        let pty = slot.take();
        drop(ptys);
        drop(pty);
    }
}

/// Reference count bump for `dup`/`fork` of a pty fd.
pub fn master_ref(id: usize) {
    if let Some(p) = pty_at(id) {
        p.master_refs.fetch_add(1, Ordering::SeqCst);
    }
}

pub fn slave_ref(id: usize) {
    if let Some(p) = pty_at(id) {
        p.slave_refs.fetch_add(1, Ordering::SeqCst);
        if p.sink == Sink::Serial {
            SERIAL_TAKEN.store(true, Ordering::Relaxed);
        }
    }
}

/// One fd referencing the master end closed; true when it was the last.
pub fn drop_master(id: usize) -> bool {
    let Some(p) = pty_at(id) else { return false };
    let last = p.master_refs.fetch_sub(1, Ordering::SeqCst) == 1;
    if last {
        // Session end: the login shell's controlling pty is gone.
        if let Some(pgid) = session_pgid(id) {
            let _ = crate::signal::kill_pg(pgid, SIGHUP);
        }
        // Blocked slave readers/writers report EIO now.
        notify(id);
        free_if_dead(id);
    }
    last
}

/// One fd referencing the slave end closed; true when it was the last.
pub fn drop_slave(id: usize) -> bool {
    let Some(p) = pty_at(id) else { return false };
    let last = p.slave_refs.fetch_sub(1, Ordering::SeqCst) == 1;
    if last {
        // A blocked master read drains and then reports EIO.
        notify(id);
    }
    free_if_dead(id);
    last
}

/// Make the pair the controlling terminal of the caller's session
/// (TIOCSCTTY, `ctty` on the control file). Only a session leader can, and
/// only while no live session holds the pair; anything else is ignored.
pub fn claim_session(id: usize) {
    let Some(p) = pty_at(id) else { return };
    let me = crate::task::current_pid();
    if crate::task::getsid(0) != Some(me) {
        return;
    }
    let held = p.session.load(Ordering::SeqCst);
    if held != usize::MAX && crate::task::session_alive(held) {
        return;
    }
    let _ = p
        .session
        .compare_exchange(held, me, Ordering::SeqCst, Ordering::SeqCst);
}

/// The pty that `pid`'s session claimed, if any: its controlling terminal,
/// what `/dev/tty` and `/proc/self/tty` mean to it.
pub fn for_session(pid: usize) -> Option<usize> {
    let sid = crate::task::getsid(pid)?;
    let ptys = PTYS.lock();
    let id = ptys
        .iter()
        .position(|slot| slot.as_ref().is_some_and(|p| p.session.load(Ordering::SeqCst) == sid))?;
    drop(ptys);
    crate::task::session_alive(sid).then_some(id)
}

/// The pair's window size: its own, or a terminal's screen's.
fn winsize(p: &Pty) -> (u16, u16) {
    match (p.sink, *p.winsize.lock()) {
        (Sink::Screen(_), _) | (Sink::Serial, (0, 0)) => crate::console::winsize(),
        (_, size) => size,
    }
}

/// The text of a pair's `ctl` (`crate::tty::ctl_text`).
pub fn ctl_text(id: usize) -> Option<Vec<u8>> {
    let p = pty_at(id)?;
    let termios = p.term.lock().termios;
    let (rows, cols) = winsize(p);
    Some(crate::tty::ctl_text(&termios, rows, cols))
}

/// A write to `/dev/pts/N/ctl`: the parsed lines applied to the pair, or
/// `None` (and nothing applied) when the text is not valid.
pub fn ctl_write(id: usize, text: &[u8]) -> Option<usize> {
    let p = pty_at(id)?;
    let current = p.term.lock().termios;
    let actions = crate::tty::ctl_parse(&current, text)?;
    // The keyboard map and the screen shown are the console's: refused
    // before anything is applied.
    if actions.iter().any(|a| matches!(a, CtlAction::Keymap(_) | CtlAction::Screen(_))) {
        return None;
    }
    for action in actions {
        match action {
            CtlAction::Termios(t) => p.term.lock().set_termios(t),
            // A screen is the size it is.
            CtlAction::Winsize(rows, cols) => {
                if !matches!(p.sink, Sink::Screen(_)) {
                    *p.winsize.lock() = (rows, cols);
                }
            }
            CtlAction::Ctty => claim_session(id),
            CtlAction::Keymap(_) | CtlAction::Screen(_) => {}
            CtlAction::Flush { input, output } => {
                if input {
                    p.term.lock().flush_input();
                }
                if output {
                    let mut out = p.out.lock();
                    out.head = 0;
                    out.len = 0;
                }
                notify(id);
            }
        }
    }
    Some(text.len())
}

/// Write to the master: bytes become slave input through the discipline.
/// Echo and `ISIG` behave exactly like the console tty, scoped to this pair.
pub fn master_write(id: usize, data: &[u8]) -> usize {
    let Some(p) = pty_at(id) else {
        return usize::MAX;
    };
    let mut n = 0usize;
    let mut vintr = false;
    {
        let mut term = p.term.lock();
        let mut out = p.out.lock();
        // Echo goes through the same OPOST/ONLCR output processing as slave
        // writes (Linux: echoed input is subject to output post-processing),
        // so a typed LF echoes as CRLF on the master.
        let oflag = term.termios.c_oflag;
        let post = oflag & OPOST != 0;
        let onlcr = oflag & ONLCR != 0;
        let sink = p.sink;
        let mut echo = |b: u8| {
            if post && onlcr && b == b'\n' {
                out.push(b'\r');
            }
            out.push(b);
        };
        let mut echo_terminal = |b: u8| {
            if post && onlcr && b == b'\n' {
                sink.emit(b"\r");
            }
            sink.emit(&[b]);
        };
        let echo: &mut dyn FnMut(u8) = if sink == Sink::Master { &mut echo } else { &mut echo_terminal };
        for &raw in data {
            if term.push_raw(raw, echo) {
                vintr = true;
            }
            n += 1;
        }
    }
    if vintr {
        if let Some(pgid) = session_pgid(id) {
            let _ = crate::signal::kill_pg(pgid, SIGINT);
        }
    }
    notify(id);
    n
}

/// Wake tasks blocked on this pair (either end) and any poller.
fn notify(id: usize) {
    if let Some(p) = pty_at(id) {
        p.changes.fetch_add(1, Ordering::SeqCst);
    }
    crate::task::wake(crate::task::key_pty(id));
    crate::task::wake_any();
}

/// How many times pair `id` changed so far ([`notify`]).
pub fn changes(id: usize) -> u64 {
    pty_at(id).map_or(0, |p| p.changes.load(Ordering::SeqCst))
}

/// Read from the slave: committed discipline output. Blocks until at least
/// one byte is available; `EIO` (`usize::MAX`) once the master is closed.
pub fn slave_read(id: usize, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    let Some(p) = pty_at(id) else {
        return usize::MAX;
    };
    crate::signal::enter_input_read();
    let n = loop {
        let seq = crate::task::wait_seq();
        if p.master_refs.load(Ordering::SeqCst) == 0 {
            break usize::MAX; // EIO: peer gone
        }
        if crate::signal::interrupt_wait() {
            break 0;
        }
        let got = {
            let mut term = p.term.lock();
            if term.available() == 0 && term.take_eof() {
                // Canonical ^D: read reports end-of-file.
                break 0;
            }
            let mut k = 0usize;
            while k < out.len() {
                match term.pop() {
                    Some(b) => {
                        out[k] = b;
                        k += 1;
                    }
                    None => break,
                }
            }
            k
        };
        if got > 0 {
            break got;
        }
        crate::task::block_until(crate::task::key_pty(id), seq, 0);
    };
    crate::signal::leave_input_read();
    notify(id);
    n
}

/// Write to the slave: output processing (OPOST/ONLCR) into the master ring.
/// Blocks while the ring is full; `EIO` once the master is closed.
pub fn slave_write(id: usize, data: &[u8]) -> usize {
    let Some(p) = pty_at(id) else {
        return usize::MAX;
    };
    if p.master_refs.load(Ordering::SeqCst) == 0 {
        return usize::MAX; // EIO
    }
    let oflag = p.term.lock().termios.c_oflag;
    let post = oflag & OPOST != 0;
    let onlcr = oflag & ONLCR != 0;
    if p.sink != Sink::Master {
        terminal_write(p.sink, data, post && onlcr);
        notify(id);
        return data.len();
    }
    let mut n = 0usize;
    for &b in data {
        // OPOST/ONLCR: LF expands to CRLF in the output stream.
        let expand = post && onlcr && b == b'\n';
        loop {
            if p.master_refs.load(Ordering::SeqCst) == 0 {
                notify(id);
                return if n == 0 { usize::MAX } else { n };
            }
            let mut out = p.out.lock();
            if expand {
                // Push CRLF as a unit; if only one slot is free, push the CR
                // first and finish the LF next round (progress guaranteed).
                if out.push(b'\r') {
                    if out.push(b'\n') {
                        n += 1;
                        break;
                    }
                    continue;
                }
            } else if out.push(b) {
                n += 1;
                break;
            }
            drop(out);
            // Ring full: wake the master to drain it, then sleep until it
            // has. The sequence is read after `notify` (which bumps it), or
            // `block_until` returns at once and the write spins; the ring is
            // looked at again before sleeping, so a drain between the two
            // is not missed. A signal breaks the wait: a slave writing to a
            // ring whose master never reads must stay killable.
            notify(id);
            if crate::signal::interrupt_wait() {
                return if n == 0 { usize::MAX } else { n };
            }
            let seq = crate::task::wait_seq();
            if p.out.lock().len == OUT_CAP {
                crate::task::block_until(crate::task::key_pty(id), seq, 0);
            }
        }
    }
    notify(id);
    n
}

/// A kernel terminal's output: to its screen or the UART, LF as CRLF with
/// `crlf` (`OPOST`, `ONLCR`), in pieces between the newlines.
fn terminal_write(sink: Sink, data: &[u8], crlf: bool) {
    if !crlf {
        sink.emit(data);
        return;
    }
    for (i, line) in data.split(|&b| b == b'\n').enumerate() {
        if i > 0 {
            sink.emit(b"\r\n");
        }
        sink.emit(line);
    }
}

/// Input for a kernel terminal: the keyboard's bytes for the shown screen's
/// tty, the UART's for `ttyS0`.
pub fn terminal_input(id: usize, data: &[u8]) {
    master_write(id, data);
}

/// Read from the master: slave output (and echo). Blocks until at least one
/// byte is available; `EIO` after the last slave fd has closed (and the
/// buffer is drained).
pub fn master_read(id: usize, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    let Some(p) = pty_at(id) else {
        return usize::MAX;
    };
    let n = loop {
        let seq = crate::task::wait_seq();
        let got = p.out.lock().take(out);
        if got > 0 {
            break got;
        }
        if p.slave_refs.load(Ordering::SeqCst) == 0 {
            break usize::MAX; // EIO: session ended
        }
        if crate::signal::interrupt_wait() {
            break 0;
        }
        crate::task::block_until(crate::task::key_pty(id), seq, 0);
    };
    // Space freed for slave writers blocked on a full ring.
    notify(id);
    n
}

/// `poll` readiness of one end of pair `id`, as (readable, writable, hung
/// up). The slave reads committed discipline input (or a pending `^D`) and
/// is hung up once the master is gone; the master reads slave output and is
/// hung up once every slave fd closed.
pub fn poll_state(id: usize, master: bool) -> (bool, bool, bool) {
    let Some(p) = pty_at(id) else {
        return (true, false, true);
    };
    if master {
        let readable = p.out.lock().len > 0;
        (readable, true, p.slave_refs.load(Ordering::SeqCst) == 0)
    } else {
        let readable = {
            let term = p.term.lock();
            term.available() > 0 || term.eof
        };
        let writable = p.sink != Sink::Master || p.out.lock().len < OUT_CAP;
        (readable, writable, p.master_refs.load(Ordering::SeqCst) == 0)
    }
}

/// Slave count (used by libgloss-facing stat to hide dead pairs).
pub fn slave_exists(id: usize) -> bool {
    pty_at(id).is_some()
}


