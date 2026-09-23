//! PTY pairs: `/dev/ptmx` master + `/dev/pts/N` slave, Linux-faithful model.
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

use crate::tty::{TtyIn, Termios, ICANON, OPOST, ONLCR};

pub const MAX_PTYS: usize = 4;
const OUT_CAP: usize = 4096;

const SIGINT: u32 = 2;
const SIGHUP: u32 = 1;

pub struct Pty {
    /// Shared input discipline + pair termios (TCGETS/TCSETS on either end).
    term: Mutex<TtyIn>,
    /// Slave output (and echo) the master reads. head/tlen relative to cap.
    out: Mutex<OutRing>,
    winsize: Mutex<(u16, u16)>,
    master_refs: AtomicUsize,
    slave_refs: AtomicUsize,
    /// Session id (slot of the session leader) bound to this pair: recorded at
    /// the first slave open without `O_NOCTTY` or via `TIOCSCTTY`. `usize::MAX`
    /// when the pair belongs to no session yet.
    session: AtomicUsize,
    /// Foreground process group of the pair (TIOCSPGRP/TIOCGPGRP). `usize::MAX`
    /// when unset: reads then report the session leader's group (the initial
    /// foreground group), per POSIX "initial foreground process group".
    fg_pgid: AtomicUsize,
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
                fg_pgid: AtomicUsize::new(usize::MAX),
            });
            return Some(id);
        }
    }
    None
}

/// Session id bound to the pair, or `usize::MAX` when unclaimed.
fn session_of_raw(id: usize) -> usize {
    pty_at(id)
        .map(|p| p.session.load(Ordering::SeqCst))
        .unwrap_or(usize::MAX)
}

fn session_pgid(id: usize) -> Option<usize> {
    let sess = session_of_raw(id);
    if sess == usize::MAX {
        return None;
    }
    crate::task::task_pgid(sess)
}

/// Raw stored foreground pgid (`usize::MAX` when never set via TIOCSPGRP and
/// never defaulted at claim time).
fn fg_raw(id: usize) -> usize {
    pty_at(id)
        .map(|p| p.fg_pgid.load(Ordering::SeqCst))
        .unwrap_or(usize::MAX)
}

/// Effective foreground process group (TIOCGPGRP). Unset falls back to the
/// session leader's slot (the initial foreground group), and a pair with no
/// session reports 0 — sortix/os-test pty/tcgetpgrp-uncontrolled requires
/// tcgetpgrp(3) to succeed (>= 0) on a session-less pty, and pty/
/// tiocsctty-is-needed accepts only ENOTTY or the session from tcgetsid(3).
/// The tests are the spec, so "no foreground group" is 0, not an error.
pub fn fg_pgid(id: usize) -> usize {
    let fg = fg_raw(id);
    if fg != usize::MAX {
        return fg;
    }
    let sess = session_of_raw(id);
    if sess != usize::MAX {
        return sess;
    }
    0
}

/// TIOCSPGRP: store the foreground pgid verbatim. The os-test pty suite
/// demands permissive storage semantics (tcsetpgrp-wrong-pid stores a
/// nonexistent pid, -wrong-session stores a foreign pgid, -zombie/-limbo
/// store awaited leaders, -wrong-orphan must succeed without SIGTTOU), so no
/// liveness or same-session validation happens here.
pub fn set_fg_pgid(id: usize, pgid: usize) {
    if let Some(p) = pty_at(id) {
        p.fg_pgid.store(pgid, Ordering::SeqCst);
    }
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
        // Session end: the login shell's controlling pty is gone. POSIX:
        // hangup notifies the foreground process group; fall back to the
        // session leader's group when no foreground group was ever set.
        let fg = fg_raw(id);
        let pgid = if fg != usize::MAX {
            Some(fg)
        } else {
            session_pgid(id)
        };
        if let Some(pgid) = pgid {
            let _ = crate::signal::kill_pg(pgid, SIGHUP);
        }
        free_if_dead(id);
    }
}

/// One fd referencing the slave end closed.
pub fn drop_slave(id: usize) {
    let Some(p) = pty_at(id) else { return };
    p.slave_refs.fetch_sub(1, Ordering::SeqCst);
    free_if_dead(id);
}

/// Bind the caller's session to this pair (first slave open without
/// `O_NOCTTY`, or TIOCSCTTY). The pair's session id is the caller's session
/// leader slot; the first claim also becomes the initial foreground process
/// group (the opener's process group). Only the first claim wins, matching
/// "session leader" rules — TIOCSCTTY uses `force_claim_session` instead.
pub fn claim_session(id: usize) {
    let Some(p) = pty_at(id) else { return };
    let sid = crate::task::current_sid();
    let newly = p
        .session
        .compare_exchange(usize::MAX, sid, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok();
    if newly {
        let pgid = crate::task::current_pgid().unwrap_or(sid);
        let _ = p
            .fg_pgid
            .compare_exchange(usize::MAX, pgid, Ordering::SeqCst, Ordering::SeqCst);
    }
}

/// TIOCSCTTY: (re-)bind the pair to the caller's session unconditionally.
/// sortix/os-test pty/tiocsctty-steal requires that a second session can
/// steal a live pty this way, so this never refuses; the new session's group
/// becomes the foreground group.
pub fn force_claim_session(id: usize) {
    let Some(p) = pty_at(id) else { return };
    let sid = crate::task::current_sid();
    p.session.store(sid, Ordering::SeqCst);
    let pgid = crate::task::current_pgid().unwrap_or(sid);
    p.fg_pgid.store(pgid, Ordering::SeqCst);
}

/// TIOCNOTTY: relinquish the pair when the caller's session owns it.
/// Returns true when the pair was released. The old foreground group gets the
/// hangup signal, matching the last-master-close semantics. A caller from a
/// foreign session leaves the pair untouched (os-test pty/
/// tiocsctty-steal-tiocnotty accepts TIOCNOTTY succeeding while only
/// removing the caller's own controlling terminal).
pub fn release_session_if_owner(id: usize, sid: usize) -> bool {
    let Some(p) = pty_at(id) else { return false };
    let owned = p
        .session
        .compare_exchange(sid, usize::MAX, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok();
    if owned {
        let fg = p.fg_pgid.swap(usize::MAX, Ordering::SeqCst);
        if fg != usize::MAX {
            let _ = crate::signal::kill_pg(fg, SIGHUP);
        }
    }
    owned
}

/// Session id of the pair, or `usize::MAX` when the pair has no session.
/// TIOCGSID (tcgetsid(3)) copies this as-is; `task::fd_ioctl` maps MAX to 0.
pub fn session_of(id: usize) -> usize {
    pty_at(id)
        .map(|p| p.session.load(Ordering::SeqCst))
        .unwrap_or(usize::MAX)
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
    n
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
        if p.master_refs.load(Ordering::SeqCst) == 0 {
            break usize::MAX; // EIO: peer gone
        }
        if crate::signal::current_should_wake() {
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
        crate::task::yield_now();
    };
    crate::signal::leave_input_read();
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
            crate::task::yield_now();
        }
    }
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
        let got = p.out.lock().take(out);
        if got > 0 {
            break got;
        }
        if p.slave_refs.load(Ordering::SeqCst) == 0 {
            break usize::MAX; // EIO: session ended
        }
        if crate::signal::current_should_wake() {
            break 0;
        }
        crate::task::yield_now();
    };
    n
}

/// Slave count (used by libgloss-facing stat to hide dead pairs).
pub fn slave_exists(id: usize) -> bool {
    pty_at(id).is_some()
}

/// True when the pair's canonical input processing is enabled (debug helper).
pub fn canonical(id: usize) -> bool {
    pty_at(id)
        .map(|p| p.term.lock().termios.c_lflag & ICANON != 0)
        .unwrap_or(false)
}

