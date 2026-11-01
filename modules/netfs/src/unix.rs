//! `/net/unix`: local stream and record connections, Plan 9 style,
//! without netd.
//!
//! The same files as `/net/tcp` (`clone`, `N/ctl`, `N/data`, `N/status`) plus
//! `N/listen`. Opening `clone` makes a conversation (the fd is on its
//! `data`); its `ctl` takes:
//!
//! - `announce NAME`: listen under NAME (a name, not a file: nothing appears
//!   in the filesystem, and the name is free again when the listener closes);
//! - `connect NAME`: connect to the listener of that name. The server's end
//!   is a new conversation, queued on the listener: reading the listener's
//!   `listen` file hands out its number (`"N\n"`, nothing when none waits);
//! - `pair`: a second conversation connected to this one, handed out through
//!   this one's `listen` the same way (`socketpair`);
//! - `hangup`: close the connection both ways (`shutdown`);
//! - `seqpacket`: before it is announced, connected or paired, make the
//!   conversation keep records (`SOCK_SEQPACKET`): each write is one,
//!   taken whole or refused (a record larger than [`RECORD_MAX`] is
//!   refused: the kernel hands a file a larger write in pieces, which
//!   would be records of their own, so the libraries say `EMSGSIZE`
//!   first), and a read returns at most one, the rest of a record it has
//!   no room for dropped. `pair`'s other end and the server's end of a
//!   `connect` keep records too, and only a record client connects to a
//!   record listener. Reading `ctl` says `seqpacket` for such a
//!   conversation (nothing for a stream), `seqpacket cut` when the last
//!   read of `data` dropped the rest of a record (`MSG_TRUNC`), and
//!   `data`'s size is the length of its next record;
//! - `rights FD...`: send the writer's own fds with the next write of
//!   `data` (`SCM_RIGHTS`). They are held as the writer's fds hold them
//!   (the kernel's `fd_hold`) and come with the first byte of that write:
//!   a read of `data` never goes past the bytes before them, and the read
//!   that starts there takes them. Reading `N/rights` then makes them the
//!   reader's fds (`fd_install`) and gives their numbers (`"5 6\n"`); the
//!   next read of `data` closes the ones not taken, and so does the end of
//!   the conversation, for every fd it still carries. `norights` lets go
//!   of the fds `rights` took when the write they were for failed.
//!
//! `N/from` gives the pid of the process whose write the last read of
//! `data` returned bytes of (`"12\n"`, nothing before a read):
//! `SCM_CREDENTIALS`. A read never returns the bytes of two writers.
//!
//! Each end buffers what its peer wrote, up to [`BUF_CAP`] bytes (allocated
//! as it fills, let go of once it is drained). As for
//! tcp, reads and writes never wait: an empty `data` reads 0 bytes (`status`
//! says `hangup` once the peer is gone), and a write takes what fits, failing
//! when nothing does. Waiting is the socket library's job (libgloss
//! `socket.c`). The last close of `data` ends the conversation; a listener's
//! queued, never-accepted connections end with it. There is no fixed
//! number of conversations: each holds an fd, so the fd limits bound them,
//! and a full kernel heap refuses a new one (or a write) rather than
//! failing the kernel.
//!
//! A client and its server usually run on different CPUs at once, so every
//! entry point takes the lock on [`CONVS`] (module calls run with interrupts
//! off and no kernel lock).

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use myos_abi::{Lock, MYOS_POLLERR, MYOS_POLLHUP, MYOS_POLLIN, MYOS_POLLOUT};

use crate::{Node, put_bytes, put_dec, S_IFCHR, S_IFDIR, S_IFREG};

/// Bytes one end holds for its reader: a whole large X reply or image.
const BUF_CAP: usize = 64 * 1024;
/// A drained buffer bigger than this is let go of.
const BUF_KEEP: usize = 4096;
/// `sizeof(sun_path)`.
const NAME_CAP: usize = 108;
/// Connections a listener queues before `connect` is refused.
const BACKLOG: usize = 8;
/// Fds one write carries at most (Linux's `SCM_MAX_FD`).
const RIGHTS_MAX: usize = 253;
/// The largest record: what the kernel hands a file in one write
/// (`FILE_IO_TMP`; issue #645).
const RECORD_MAX: usize = 8 * 1024;
/// The most records one end holds for its reader: each costs a `writers`
/// entry outside `BUF_CAP`, so small ones are bounded by count (as Linux
/// charges each its overhead).
const RECORDS_CAP: usize = 512;
/// What reading a record conversation's `ctl` says, and after a read that
/// dropped the rest of a record.
const SEQPACKET: &[u8] = b"seqpacket\n";
const SEQPACKET_CUT: &[u8] = b"seqpacket cut\n";

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Free,
    /// Cloned, not yet announced or connected.
    Open,
    Listening,
    Connected,
    /// The peer is gone, or `hangup` was written: no more writes; what is
    /// buffered can still be read.
    Hangup,
}

struct Conv {
    state: State,
    /// Records, not a byte stream (`seqpacket`).
    records: bool,
    /// The last read of `data` dropped the rest of a record.
    cut: bool,
    /// The length of the last record this end could not write for want of
    /// room in its peer's buffer: `poll` says `POLLOUT` once it fits.
    want: usize,
    /// How many times the end changed (its state, bytes in or out of its
    /// buffer, room in its peer's): the change count its `data`'s `poll`
    /// reports. Kept across the slot's reuse ([`reset`]).
    changes: u64,
    /// The uid of the user whose process opened `clone` (a connection made
    /// by `connect`: the listener's), the only one whose processes may
    /// open its files.
    owner: u32,
    peer: Option<u16>,
    name_len: usize,
    name: [u8; NAME_CAP],
    /// Conversations waiting to be handed out through `listen`.
    queue: [u16; BACKLOG],
    queued: usize,
    rx: VecDeque<u8>,
    /// The offset in this end's stream of `rx`'s first byte.
    rx_base: u64,
    /// Fds that came with the byte at an offset of the stream, in order
    /// (the kernel's tokens for them).
    marks: VecDeque<(u64, Vec<usize>)>,
    /// Fds `rights` took from this end's writer, for its next write.
    pending: Vec<usize>,
    /// Fds that came with the last read of `data`, for `rights`.
    delivered: Vec<usize>,
    /// Where in this end's stream each writer's bytes start (an entry when
    /// the writer changes, and for each record), with its pid.
    writers: VecDeque<(u64, u32)>,
    /// The pid of the writer of the last read's bytes, for `from` (0: none).
    from: u32,
}

impl Conv {
    const FREE: Self = Self {
        state: State::Free,
        records: false,
        cut: false,
        want: 0,
        changes: 0,
        owner: crate::NO_OWNER,
        peer: None,
        name_len: 0,
        name: [0; NAME_CAP],
        queue: [0; BACKLOG],
        queued: 0,
        rx: VecDeque::new(),
        rx_base: 0,
        marks: VecDeque::new(),
        pending: Vec::new(),
        delivered: Vec::new(),
        writers: VecDeque::new(),
        from: 0,
    };

    /// Every fd it carries, to be let go of.
    fn take_rights(&mut self, drops: &mut Vec<usize>) {
        drops.append(&mut self.pending);
        drops.append(&mut self.delivered);
        for (_, mut fds) in self.marks.drain(..) {
            drops.append(&mut fds);
        }
    }

    fn status(&self) -> &'static [u8] {
        match self.state {
            State::Free => b"",
            State::Open => b"open",
            State::Listening => b"announced",
            State::Connected => b"connected",
            State::Hangup => b"hangup",
        }
    }
}

/// The conversations by number; a free slot is used again first.
static CONVS: Lock<Vec<Conv>> = Lock::new(Vec::new());

/// Run `f` on the conversations with their lock held.
fn with<R>(f: impl FnOnce(&mut Vec<Conv>) -> R) -> R {
    f(&mut CONVS.lock())
}

/// End `id` changed: its readiness may have, and an edge-triggered poller
/// of its `data` is told again ([`Conv::changes`]).
fn changed(convs: &mut [Conv], id: u16) {
    if let Some(c) = convs.get_mut(id as usize) {
        c.changes = c.changes.wrapping_add(1);
    }
}

/// Slot `id` back to [`Conv::FREE`], its change count kept and moved.
fn reset(convs: &mut [Conv], id: u16) {
    let changes = convs[id as usize].changes;
    convs[id as usize] = Conv::FREE;
    convs[id as usize].changes = changes.wrapping_add(1);
}

/// [`with`] for a call that may let go of passed fds (into `drops`): they
/// are released once the lock is dropped, as the last reference to a unix
/// socket closes it, back into this module.
fn with_drops<R>(f: impl FnOnce(&mut Vec<Conv>, &mut Vec<usize>) -> R) -> R {
    let mut drops = Vec::new();
    let r = f(&mut CONVS.lock(), &mut drops);
    for token in drops {
        crate::api().fd_release(token);
    }
    r
}

fn get(convs: &mut Vec<Conv>, id: u16) -> Option<&mut Conv> {
    convs.get_mut(id as usize).filter(|c| c.state != State::Free)
}

/// [`get`] for a file call: the caller's user's conversations only.
fn get_own(convs: &mut Vec<Conv>, id: u16) -> Option<&mut Conv> {
    let uid = crate::uid();
    get(convs, id).filter(|c| c.owner == uid)
}

fn alloc(convs: &mut Vec<Conv>) -> Option<u16> {
    let i = match convs.iter().position(|c| c.state == State::Free) {
        Some(i) => i,
        None if convs.len() < u16::MAX as usize && convs.try_reserve(1).is_ok() => {
            convs.push(Conv::FREE);
            convs.len() - 1
        }
        None => return None,
    };
    reset(convs, i as u16);
    convs[i].state = State::Open;
    convs[i].owner = crate::uid();
    Some(i as u16)
}

/// A new conversation (`clone`'s open), the opener's user's: its number.
pub fn clone() -> Option<u16> {
    with_drops(|convs, _| alloc(convs))
}

/// Connect `a` and `b` (both just made or `Open`).
fn link(convs: &mut Vec<Conv>, a: u16, b: u16) {
    convs[a as usize].state = State::Connected;
    convs[a as usize].peer = Some(b);
    convs[b as usize].state = State::Connected;
    convs[b as usize].peer = Some(a);
    changed(convs, a);
    changed(convs, b);
}

/// End `id`'s connection: both ends see `hangup`.
fn disconnect(convs: &mut Vec<Conv>, id: u16) {
    if let Some(peer) = convs[id as usize].peer.take() {
        convs[peer as usize].peer = None;
        convs[peer as usize].state = State::Hangup;
        changed(convs, peer);
    }
    if convs[id as usize].state == State::Connected {
        convs[id as usize].state = State::Hangup;
    }
    changed(convs, id);
}

/// The listener whose name is `name`.
fn listener(convs: &[Conv], name: &[u8]) -> Option<u16> {
    let i = convs
        .iter()
        .position(|c| c.state == State::Listening && &c.name[..c.name_len] == name)?;
    Some(i as u16)
}

fn enqueue(convs: &mut Vec<Conv>, on: u16, id: u16) -> bool {
    let c = &mut convs[on as usize];
    if c.queued == BACKLOG {
        return false;
    }
    c.queue[c.queued] = id;
    c.queued += 1;
    changed(convs, on);
    true
}

fn dec(id: u16, out: &mut [u8]) -> i32 {
    let mut tmp = [0u8; 8];
    let mut n = 0;
    let _ = put_dec(&mut tmp, &mut n, id);
    let n = n.min(out.len());
    out[..n].copy_from_slice(&tmp[..n]);
    n as i32
}

/// `stat`: (mode, size, ino); size is the bytes waiting in `data`, the
/// connections waiting in `listen`.
pub fn stat(node: Node) -> Option<(u32, u32, u32)> {
    with(|convs| {
        let (id, tag) = match node {
            Node::Proto(_) => return Some((S_IFDIR | 0o755, 0, 14)),
            Node::Clone(_) => return Some((S_IFCHR | 0o666, 0, 24)),
            Node::ConvDir(_, id) => (id, 0),
            Node::Ctl(_, id) => (id, 1),
            Node::Data(_, id) => (id, 2),
            Node::Status(_, id) => (id, 3),
            Node::Listen(_, id) => (id, 4),
            Node::Rights(_, id) => (id, 5),
            Node::From(_, id) => (id, 6),
            Node::Root | Node::Ndb | Node::Ifaddrs | Node::Convs | Node::Slots | Node::Hdata(_) | Node::Peek(_, _) => {
                return None
            }
        };
        let c = get_own(convs, id)?;
        let ino = 1000 + id as u32 * 8 + tag;
        Some(match tag {
            0 => (S_IFDIR | 0o755, 0, ino),
            2 => (S_IFREG | 0o666, waiting(c) as u32, ino),
            3 => (S_IFREG | 0o666, c.status().len() as u32, ino),
            4 => (S_IFREG | 0o666, c.queued as u32, ino),
            _ => (S_IFREG | 0o666, 0, ino),
        })
    })
}

/// The records `c` holds for its reader: its `writers` entries but the
/// one a read already took (kept until the next read).
fn records_held(c: &Conv) -> usize {
    c.writers.len() - usize::from(c.writers.front().is_some_and(|&(at, _)| at < c.rx_base))
}

/// The bytes a read of `data` could take: all that is buffered, or a
/// record conversation's next record.
fn waiting(c: &Conv) -> usize {
    let next = c.writers.iter().map(|&(at, _)| at).find(|&at| at > c.rx_base);
    match next {
        Some(at) if c.records => (at - c.rx_base) as usize,
        _ => c.rx.len(),
    }
}

pub fn listdir(node: Node, dst: &mut [u8], n: &mut usize) -> bool {
    with(|convs| match node {
        Node::Proto(_) => {
            let _ = put_bytes(dst, n, b"clone");
            for (i, c) in convs.iter().enumerate() {
                if c.state != State::Free {
                    let _ = put_dec(dst, n, i as u16);
                }
            }
            true
        }
        Node::ConvDir(_, id) if get_own(convs, id).is_some() => {
            for leaf in [&b"ctl"[..], b"data", b"status", b"listen", b"rights", b"from"] {
                let _ = put_bytes(dst, n, leaf);
            }
            true
        }
        _ => false,
    })
}

pub fn read(node: Node, pos: usize, out: &mut [u8]) -> i32 {
    let n = read_locked(node, pos, out);
    if n > 0 && matches!(node, Node::Data(..)) {
        // Room in this end's buffer: a writer may be polling for it.
        crate::wake_any();
    }
    n
}

/// `poll` readiness: bytes (or a queued connection) to read, room in the
/// peer's buffer to write, the peer gone; the end's change count into
/// `changes`.
pub fn poll(node: Node, changes: *mut u64) -> u32 {
    with(|convs| {
        let (Node::Data(_, id) | Node::Listen(_, id)) = node else {
            return MYOS_POLLIN | MYOS_POLLOUT;
        };
        // SAFETY: the kernel passes a live out-parameter.
        unsafe { *changes = convs.get(id as usize).map_or(0, |c| c.changes) };
        let Some(c) = get_own(convs, id) else {
            return MYOS_POLLERR | MYOS_POLLHUP;
        };
        let (readable, state, peer, want, records) =
            (!c.rx.is_empty() || c.queued != 0, c.state, c.peer, c.want.max(1), c.records);
        let mut bits = if readable { MYOS_POLLIN } else { 0 };
        if state == State::Hangup {
            bits |= MYOS_POLLIN | MYOS_POLLHUP;
        }
        let room = |p: &Conv| BUF_CAP - p.rx.len() >= want && (!records || records_held(p) < RECORDS_CAP);
        if state == State::Connected && peer.is_some_and(|p| room(&convs[p as usize])) {
            bits |= MYOS_POLLOUT;
        }
        bits
    })
}

fn read_locked(node: Node, pos: usize, out: &mut [u8]) -> i32 {
    with_drops(|convs, drops| match node {
        Node::Data(_, id) => {
            let Some(c) = get_own(convs, id) else {
                return -1;
            };
            // The fds an earlier read brought and nobody took are closed,
            // as a Linux `read` of a socket closes them.
            drops.append(&mut c.delivered);
            let start = c.rx_base;
            let mut n = out.len().min(c.rx.len());
            // Fds come with the read that starts at their byte, never with
            // one that started before it.
            if let Some(&(at, _)) = c.marks.iter().find(|(at, _)| *at > start) {
                n = n.min((at - start) as usize);
            }
            if n > 0 && c.marks.front().is_some_and(|(at, _)| *at == start) {
                c.delivered = c.marks.pop_front().map(|(_, fds)| fds).unwrap_or_default();
            }
            // Nor bytes of the next writer.
            while c.writers.get(1).is_some_and(|&(at, _)| at <= start) {
                c.writers.pop_front();
            }
            if let Some(&(at, _)) = c.writers.get(1) {
                n = n.min((at - start) as usize);
            }
            if n > 0 {
                c.from = c.writers.front().map_or(0, |&(_, pid)| pid);
            }
            for (o, b) in out.iter_mut().zip(c.rx.drain(..n)) {
                *o = b;
            }
            c.rx_base += n as u64;
            // A record is read whole: what did not fit is dropped (with the
            // fds of a record that started in it, none: they come with a
            // record's first byte).
            c.cut = false;
            if c.records && n > 0 {
                let end = c.writers.get(1).map_or(c.rx.len(), |&(at, _)| (at - c.rx_base) as usize);
                c.rx.drain(..end);
                c.rx_base += end as u64;
                c.cut = end > 0;
            }
            if c.rx.is_empty() && c.rx.capacity() > BUF_KEEP {
                c.rx = VecDeque::new();
            }
            if n > 0 {
                // Room for the peer's writes too.
                let peer = c.peer;
                changed(convs, id);
                if let Some(peer) = peer {
                    changed(convs, peer);
                }
            }
            n as i32
        }
        Node::Status(_, id) => {
            let Some(c) = get_own(convs, id) else {
                return -1;
            };
            crate::copy_at(c.status(), pos, out)
        }
        Node::Listen(_, id) => {
            let Some(c) = get_own(convs, id) else {
                return -1;
            };
            if c.queued == 0 {
                return 0;
            }
            let next = c.queue[0];
            c.queue.copy_within(1..c.queued, 0);
            c.queued -= 1;
            changed(convs, id);
            dec(next, out)
        }
        Node::Ctl(_, id) => {
            let Some(c) = get_own(convs, id) else {
                return -1;
            };
            let mode: &[u8] = match (c.records, c.cut) {
                (false, _) => b"",
                (true, false) => SEQPACKET,
                (true, true) => SEQPACKET_CUT,
            };
            crate::copy_at(mode, pos, out)
        }
        Node::From(_, id) => {
            let Some(c) = get_own(convs, id) else {
                return -1;
            };
            if pos > 0 || c.from == 0 {
                return 0;
            }
            crate::copy_at(alloc::format!("{}\n", c.from).as_bytes(), 0, out)
        }
        Node::Rights(_, id) => {
            let Some(c) = get_own(convs, id) else {
                return -1;
            };
            // Each fd's number and a space: installed only when all fit.
            if out.len() < 4 * c.delivered.len() + 1 {
                return -1;
            }
            let mut n = 0;
            for token in c.delivered.drain(..) {
                // No free fd: closed, as Linux closes what does not fit
                // (`MSG_CTRUNC`).
                let Some(fd) = crate::api().fd_install(token, false) else {
                    drops.push(token);
                    continue;
                };
                if n > 0 {
                    out[n] = b' ';
                    n += 1;
                }
                n += dec(fd as u16, &mut out[n..]) as usize;
            }
            if n > 0 {
                out[n] = b'\n';
                n += 1;
            }
            n as i32
        }
        _ => -1,
    })
}

pub fn write(node: Node, src: &[u8]) -> i32 {
    with_drops(|convs, drops| match node {
        Node::Ctl(_, id) => {
            if get_own(convs, id).is_none() {
                return -1;
            }
            if ctl(convs, id, crate::trim_ctl(src), drops) {
                src.len() as i32
            } else {
                -1
            }
        }
        Node::Data(_, id) => {
            let Some(peer) = get_own(convs, id).and_then(|c| c.peer) else {
                return -1;
            };
            let records = convs[id as usize].records;
            let p = &mut convs[peer as usize];
            let room = BUF_CAP - p.rx.len();
            // A record is taken whole or not at all; an empty one is none.
            // One that never fits (`EMSGSIZE`, the library's) waits for
            // nothing.
            if records && src.len() > RECORD_MAX {
                return -1;
            }
            if records && (src.len() > room || records_held(p) >= RECORDS_CAP) {
                convs[id as usize].want = src.len();
                return -1;
            }
            if records && src.is_empty() {
                return 0;
            }
            let n = src.len().min(room);
            // Full (or no heap for it): the library waits for the reader and
            // retries.
            if n == 0 || p.rx.try_reserve(n).is_err() || p.marks.try_reserve(1).is_err() || p.writers.try_reserve(1).is_err() {
                return -1;
            }
            let at = p.rx_base + p.rx.len() as u64;
            p.rx.extend(&src[..n]);
            let pid = crate::api().current_pid() as u32;
            if records || p.writers.back().is_none_or(|&(_, last)| last != pid) {
                p.writers.push_back((at, pid));
            }
            convs[id as usize].want = 0;
            // The fds `rights` took come with this write's first byte.
            let fds = core::mem::take(&mut convs[id as usize].pending);
            if !fds.is_empty() {
                convs[peer as usize].marks.push_back((at, fds));
            }
            // Bytes for the peer; this end may have filled its buffer.
            changed(convs, peer);
            changed(convs, id);
            n as i32
        }
        _ => -1,
    })
}

fn ctl(convs: &mut Vec<Conv>, id: u16, cmd: &[u8], drops: &mut Vec<usize>) -> bool {
    let open = convs[id as usize].state == State::Open;
    if let Some(list) = cmd.strip_prefix(b"rights ") {
        return rights(convs, id, list, drops);
    }
    if cmd == b"norights" {
        // The write they were for failed: they are not sent.
        drops.append(&mut convs[id as usize].pending);
        return true;
    }
    if cmd == b"hangup" {
        disconnect(convs, id);
        return true;
    }
    if cmd == b"seqpacket" {
        convs[id as usize].records |= open;
        return open;
    }
    let records = convs[id as usize].records;
    if cmd == b"pair" {
        let Some(other) = open.then(|| alloc(convs)).flatten() else {
            return false;
        };
        convs[other as usize].records = records;
        link(convs, id, other);
        return enqueue(convs, id, other);
    }
    if let Some(name) = cmd.strip_prefix(b"announce ") {
        if !open || name.is_empty() || name.len() > NAME_CAP || listener(convs, name).is_some() {
            return false;
        }
        let c = &mut convs[id as usize];
        c.name[..name.len()].copy_from_slice(name);
        c.name_len = name.len();
        c.state = State::Listening;
        changed(convs, id);
        return true;
    }
    if let Some(name) = cmd.strip_prefix(b"connect ") {
        let Some(server) = listener(convs, name) else {
            return false;
        };
        if !open || convs[server as usize].queued == BACKLOG || convs[server as usize].records != records {
            return false;
        }
        let Some(end) = alloc(convs) else {
            return false;
        };
        // The server's end is the listener's user's.
        convs[end as usize].owner = convs[server as usize].owner;
        convs[end as usize].records = records;
        link(convs, id, end);
        return enqueue(convs, server, end);
    }
    false
}

/// `rights FD...`: hold the writer's fds `list` (decimal, space
/// separated) for its next write; none held when one is not an open fd.
fn rights(convs: &mut Vec<Conv>, id: u16, list: &[u8], drops: &mut Vec<usize>) -> bool {
    if convs[id as usize].state != State::Connected {
        return false;
    }
    let mut held = Vec::new();
    for word in list.split(|&b| b == b' ').filter(|w| !w.is_empty()) {
        let fd = core::str::from_utf8(word).ok().and_then(|w| w.parse::<usize>().ok());
        match fd.and_then(|fd| crate::api().fd_hold(fd)) {
            Some(token) if convs[id as usize].pending.len() + held.len() < RIGHTS_MAX => held.push(token),
            other => {
                drops.extend(other);
                drops.append(&mut held);
                return false;
            }
        }
    }
    convs[id as usize].pending.append(&mut held);
    true
}

/// Last close of `data`: end the conversation, and the connections still
/// queued on it (nobody can open those any more), with the fds they carry.
pub fn release(node: Node) {
    let Node::Data(_, id) = node else {
        return;
    };
    with_drops(|convs, drops| {
        if get_own(convs, id).is_none() {
            return;
        }
        let c = &mut convs[id as usize];
        let (queue, queued) = (c.queue, c.queued);
        for &q in &queue[..queued] {
            disconnect(convs, q);
            convs[q as usize].take_rights(drops);
            reset(convs, q);
        }
        disconnect(convs, id);
        convs[id as usize].take_rights(drops);
        reset(convs, id);
    });
}
