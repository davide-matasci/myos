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

use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::signal::{SIGHUP, SIGINT};
use crate::tty::{CtlAction, TtyIn, OPOST, ONLCR};

const OUT_CAP: usize = 4096;

pub struct Pty {
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

/// Allocate a pair. The caller owns the initial master reference.
pub fn alloc() -> Option<usize> {
    let pty = try_box(Pty {
        term: Mutex::new(TtyIn::new()),
        out: Mutex::new(OutRing::new()),
        winsize: Mutex::new((24, 80)),
        master_refs: AtomicUsize::new(1),
        slave_refs: AtomicUsize::new(0),
        session: AtomicUsize::new(usize::MAX),
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
    }
}

/// Last fd referencing the master end closed.
pub fn drop_master(id: usize) {
    let Some(p) = pty_at(id) else { return };
    if p.master_refs.fetch_sub(1, Ordering::SeqCst) == 1 {
        // Session end: the login shell's controlling pty is gone.
        if let Some(pgid) = session_pgid(id) {
            let _ = crate::signal::kill_pg(pgid, SIGHUP);
        }
        // Blocked slave readers/writers report EIO now.
        notify(id);
        free_if_dead(id);
    }
}

/// One fd referencing the slave end closed.
pub fn drop_slave(id: usize) {
    let Some(p) = pty_at(id) else { return };
    if p.slave_refs.fetch_sub(1, Ordering::SeqCst) == 1 {
        // A blocked master read drains and then reports EIO.
        notify(id);
    }
    free_if_dead(id);
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

/// The text of `/dev/pts/N/ctl` (`crate::tty::ctl_text`).
pub fn ctl_text(id: usize) -> Option<Vec<u8>> {
    let p = pty_at(id)?;
    let termios = p.term.lock().termios;
    let (rows, cols) = *p.winsize.lock();
    Some(crate::tty::ctl_text(&termios, rows, cols))
}

/// A write to `/dev/pts/N/ctl`: the parsed lines applied to the pair, or
/// `None` (and nothing applied) when the text is not valid.
pub fn ctl_write(id: usize, text: &[u8]) -> Option<usize> {
    let p = pty_at(id)?;
    let current = p.term.lock().termios;
    let actions = crate::tty::ctl_parse(&current, text)?;
    // The keyboard map and the screen are the console's: refused before
    // anything is applied.
    if actions.iter().any(|a| matches!(a, CtlAction::Keymap(_) | CtlAction::Mirror(_))) {
        return None;
    }
    for action in actions {
        match action {
            CtlAction::Termios(t) => p.term.lock().set_termios(t),
            CtlAction::Winsize(rows, cols) => *p.winsize.lock() = (rows, cols),
            CtlAction::Ctty => claim_session(id),
            CtlAction::Keymap(_) | CtlAction::Mirror(_) => {}
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
        let mut echo = |b: u8| {
            if post && onlcr && b == b'\n' {
                out.push(b'\r');
            }
            out.push(b);
        };
        for &raw in data {
            if term.push_raw(raw, &mut echo) {
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
    crate::task::wake(crate::task::key_pty(id));
    crate::task::wake_any();
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
    let mut n = 0usize;
    for &b in data {
        // OPOST/ONLCR: LF expands to CRLF in the output stream.
        let expand = post && onlcr && b == b'\n';
        loop {
            if p.master_refs.load(Ordering::SeqCst) == 0 {
                notify(id);
                return if n == 0 { usize::MAX } else { n };
            }
            // A signal must break the wait: otherwise a slave writing to a
            // full ring whose master never reads spins unkillably in the
            // kernel (not even SIGKILL reaches it).
            if crate::signal::interrupt_wait() {
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
            // Ring full: wake the master to drain it, THEN read the wait
            // sequence and sleep. Reading the sequence after `notify` (which
            // bumps it) is what makes `block_until` actually sleep until the
            // master's drain wakes us; reading it before (the old bug) made
            // `block_until` return at once, busy-spinning the CPU. Re-check
            // under the lock so a drain racing between the notify and the
            // sleep is not missed.
            notify(id);
            let seq = crate::task::wait_seq();
            if p.out.lock().len == OUT_CAP {
                crate::task::block_until(crate::task::key_pty(id), seq, 0);
            }
        }
    }
    notify(id);
    n
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
        let writable = p.out.lock().len < OUT_CAP;
        (readable, writable, p.master_refs.load(Ordering::SeqCst) == 0)
    }
}

/// Slave count (used by libgloss-facing stat to hide dead pairs).
pub fn slave_exists(id: usize) -> bool {
    pty_at(id).is_some()
}


