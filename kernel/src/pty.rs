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

use alloc::vec::Vec;

use crate::signal::{SIGHUP, SIGINT};
use crate::tty::{CtlAction, TtyIn, Termios, OPOST, ONLCR};

pub const MAX_PTYS: usize = 4;
const OUT_CAP: usize = 4096;

pub struct Pty {
    /// Shared input discipline + pair termios (TCGETS/TCSETS on either end).
    term: Mutex<TtyIn>,
    /// Slave output (and echo) the master reads. head/tlen relative to cap.
    out: Mutex<OutRing>,
    winsize: Mutex<(u16, u16)>,
    master_refs: AtomicUsize,
    slave_refs: AtomicUsize,
    /// Session leader recorded at first slave open / TIOCSCTTY. `usize::MAX`
    /// when unset.
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

static PTYS: Mutex<[Option<Pty>; MAX_PTYS]> = Mutex::new([const { None }; MAX_PTYS]);
/// Ids freed by full-pair teardown (both ends' fds closed).
static FREE_IDS: Mutex<[bool; MAX_PTYS]> = Mutex::new([false; MAX_PTYS]);

fn pty_at(id: usize) -> Option<&'static Pty> {
    let guard = PTYS.lock();
    // SAFETY: entries are only ever *replaced* with None (never moved), and
    // callers hold a reference-counted claim on the pair while using it.
    let ptr = guard.get(id).and_then(|s| s.as_ref())? as *const Pty;
    drop(guard);
    Some(unsafe { &*ptr })
}

/// Allocate a pair. The caller owns the initial master reference.
pub fn alloc() -> Option<usize> {
    let mut ptys = PTYS.lock();
    for (id, slot) in ptys.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(Pty {
                term: Mutex::new(TtyIn::new()),
                out: Mutex::new(OutRing::new()),
                winsize: Mutex::new((24, 80)),
                master_refs: AtomicUsize::new(1),
                slave_refs: AtomicUsize::new(0),
                session: AtomicUsize::new(usize::MAX),
            });
            return Some(id);
        }
    }
    None
}

fn session_pgid(id: usize) -> Option<usize> {
    let p = pty_at(id)?;
    let sess = p.session.load(Ordering::SeqCst);
    if sess == usize::MAX {
        return None;
    }
    crate::task::task_pgid(sess)
}

fn free_if_dead(id: usize) {
    let mut ptys = PTYS.lock();
    if let Some(p) = ptys[id].as_ref() {
        if p.master_refs.load(Ordering::SeqCst) == 0 && p.slave_refs.load(Ordering::SeqCst) == 0 {
            ptys[id] = None;
            FREE_IDS.lock()[id] = true;
        }
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

/// Record the caller as the pty's session leader (first slave open,
/// TIOCSCTTY). Only the first claim wins, matching "session leader" rules.
pub fn claim_session(id: usize) {
    let Some(p) = pty_at(id) else { return };
    let me = crate::task::current_pid();
    let _ = p
        .session
        .compare_exchange(usize::MAX, me, Ordering::SeqCst, Ordering::SeqCst);
}

/// The pty whose session `pid` claimed (TIOCSCTTY), if any: what `/dev/tty`
/// means to that process and its descendants.
pub fn claimed_by(pid: usize) -> Option<usize> {
    let ptys = PTYS.lock();
    (0..MAX_PTYS).find(|&id| ptys[id].as_ref().is_some_and(|p| p.session.load(Ordering::SeqCst) == pid))
}

/// The pty of `pid`'s session: the one it or an ancestor claimed (`ctty`
/// on the control file, TIOCSCTTY). Sessions are not real yet (setsid is
/// a no-op in libgloss), so the claim is looked up along the parent chain:
/// a forkpty child and what it started (an SSH login, the tty smoke).
pub fn for_session(pid: usize) -> Option<usize> {
    let mut pid = Some(pid);
    for _ in 0..16 {
        let p = pid?;
        if let Some(id) = claimed_by(p) {
            return Some(id);
        }
        pid = crate::task::parent_pid(p);
    }
    None
}

/// TIOCGPTN: slave index for the pair.
pub fn index(id: usize) -> Option<u32> {
    Some(id as u32)
}

pub fn winsize(id: usize) -> Option<(u16, u16)> {
    Some(*pty_at(id)?.winsize.lock())
}

pub fn set_winsize(id: usize, row: u16, col: u16) {
    if let Some(p) = pty_at(id) {
        *p.winsize.lock() = (row, col);
    }
}

pub fn termios_get_bytes(id: usize) -> Option<[u8; crate::tty::TERMIOS_LEN]> {
    Some(pty_at(id)?.term.lock().termios.as_bytes())
}

pub fn termios_set_bytes(id: usize, buf: &[u8; crate::tty::TERMIOS_LEN]) {
    if let Some(p) = pty_at(id) {
        p.term.lock().set_termios(Termios::from_bytes(buf));
    }
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
    for action in crate::tty::ctl_parse(&current, text)? {
        match action {
            CtlAction::Termios(t) => p.term.lock().set_termios(t),
            CtlAction::Winsize(rows, cols) => *p.winsize.lock() = (rows, cols),
            CtlAction::Ctty => claim_session(id),
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
            let seq = crate::task::wait_seq();
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
            // Ring full: let the master drain it.
            notify(id);
            crate::task::block_until(crate::task::key_pty(id), seq, 0);
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


