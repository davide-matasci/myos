//! `/net/unix`: local stream connections, Plan 9 style, without netd.
//!
//! The same files as `/net/tcp` (`clone`, `N/ctl`, `N/data`, `N/status`) plus
//! `N/listen`. Reading `clone` makes a conversation; its `ctl` takes:
//!
//! - `announce NAME`: listen under NAME (a name, not a file: nothing appears
//!   in the filesystem, and the name is free again when the listener closes);
//! - `connect NAME`: connect to the listener of that name. The server's end
//!   is a new conversation, queued on the listener: reading the listener's
//!   `listen` file hands out its number (`"N\n"`, nothing when none waits);
//! - `pair`: a second conversation connected to this one, handed out through
//!   this one's `listen` the same way (`socketpair`);
//! - `hangup`: close the connection both ways (`shutdown`).
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
//! entry point takes [`LOCK`] (module calls run with interrupts off and no
//! kernel lock).

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use myos_abi::{MYOS_POLLERR, MYOS_POLLHUP, MYOS_POLLIN, MYOS_POLLOUT};

use crate::{Node, put_bytes, put_dec, S_IFDIR, S_IFREG};

/// Bytes one end holds for its reader: a whole large X reply or image.
const BUF_CAP: usize = 64 * 1024;
/// A drained buffer bigger than this is let go of.
const BUF_KEEP: usize = 4096;
/// `sizeof(sun_path)`.
const NAME_CAP: usize = 108;
/// Connections a listener queues before `connect` is refused.
const BACKLOG: usize = 8;

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
    peer: Option<u16>,
    name_len: usize,
    name: [u8; NAME_CAP],
    /// Conversations waiting to be handed out through `listen`.
    queue: [u16; BACKLOG],
    queued: usize,
    rx: VecDeque<u8>,
}

impl Conv {
    const FREE: Self = Self {
        state: State::Free,
        peer: None,
        name_len: 0,
        name: [0; NAME_CAP],
        queue: [0; BACKLOG],
        queued: 0,
        rx: VecDeque::new(),
    };

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

static LOCK: AtomicBool = AtomicBool::new(false);
/// The conversations by number; a free slot is used again first.
static mut CONVS: Vec<Conv> = Vec::new();

/// Run `f` on the conversations with [`LOCK`] held.
fn with<R>(f: impl FnOnce(&mut Vec<Conv>) -> R) -> R {
    while LOCK
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    let out = f(unsafe { &mut *core::ptr::addr_of_mut!(CONVS) });
    LOCK.store(false, Ordering::Release);
    out
}

fn get(convs: &mut Vec<Conv>, id: u16) -> Option<&mut Conv> {
    convs.get_mut(id as usize).filter(|c| c.state != State::Free)
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
    convs[i] = Conv::FREE;
    convs[i].state = State::Open;
    Some(i as u16)
}

/// Connect `a` and `b` (both just made or `Open`).
fn link(convs: &mut Vec<Conv>, a: u16, b: u16) {
    convs[a as usize].state = State::Connected;
    convs[a as usize].peer = Some(b);
    convs[b as usize].state = State::Connected;
    convs[b as usize].peer = Some(a);
}

/// End `id`'s connection: both ends see `hangup`.
fn disconnect(convs: &mut Vec<Conv>, id: u16) {
    if let Some(peer) = convs[id as usize].peer.take() {
        convs[peer as usize].peer = None;
        convs[peer as usize].state = State::Hangup;
    }
    if convs[id as usize].state == State::Connected {
        convs[id as usize].state = State::Hangup;
    }
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
            Node::Clone(_) => return Some((S_IFREG | 0o666, 0, 24)),
            Node::ConvDir(_, id) => (id, 0),
            Node::Ctl(_, id) => (id, 1),
            Node::Data(_, id) => (id, 2),
            Node::Status(_, id) => (id, 3),
            Node::Listen(_, id) => (id, 4),
            Node::Root => return None,
        };
        let c = get(convs, id)?;
        let ino = 1000 + id as u32 * 8 + tag;
        Some(match tag {
            0 => (S_IFDIR | 0o755, 0, ino),
            2 => (S_IFREG | 0o666, c.rx.len() as u32, ino),
            3 => (S_IFREG | 0o666, c.status().len() as u32, ino),
            4 => (S_IFREG | 0o666, c.queued as u32, ino),
            _ => (S_IFREG | 0o666, 0, ino),
        })
    })
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
        Node::ConvDir(_, id) if get(convs, id).is_some() => {
            for leaf in [&b"ctl"[..], b"data", b"status", b"listen"] {
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
/// peer's buffer to write, the peer gone.
pub fn poll(node: Node) -> u32 {
    with(|convs| {
        let (Node::Data(_, id) | Node::Listen(_, id)) = node else {
            return MYOS_POLLIN | MYOS_POLLOUT;
        };
        let Some(c) = get(convs, id) else {
            return MYOS_POLLERR | MYOS_POLLHUP;
        };
        let (readable, state, peer) = (!c.rx.is_empty() || c.queued != 0, c.state, c.peer);
        let mut bits = if readable { MYOS_POLLIN } else { 0 };
        if state == State::Hangup {
            bits |= MYOS_POLLIN | MYOS_POLLHUP;
        }
        if state == State::Connected && peer.is_some_and(|p| convs[p as usize].rx.len() < BUF_CAP) {
            bits |= MYOS_POLLOUT;
        }
        bits
    })
}

fn read_locked(node: Node, pos: usize, out: &mut [u8]) -> i32 {
    with(|convs| match node {
        Node::Clone(_) => {
            if pos > 0 {
                return 0;
            }
            match alloc(convs) {
                Some(id) => dec(id, out),
                None => 0,
            }
        }
        Node::Data(_, id) => {
            let Some(c) = get(convs, id) else {
                return -1;
            };
            let n = out.len().min(c.rx.len());
            for (o, b) in out.iter_mut().zip(c.rx.drain(..n)) {
                *o = b;
            }
            if c.rx.is_empty() && c.rx.capacity() > BUF_KEEP {
                c.rx = VecDeque::new();
            }
            n as i32
        }
        Node::Status(_, id) => {
            let Some(c) = get(convs, id) else {
                return -1;
            };
            crate::copy_at(c.status(), pos, out)
        }
        Node::Listen(_, id) => {
            let Some(c) = get(convs, id) else {
                return -1;
            };
            if c.queued == 0 {
                return 0;
            }
            let next = c.queue[0];
            c.queue.copy_within(1..c.queued, 0);
            c.queued -= 1;
            dec(next, out)
        }
        Node::Ctl(_, id) => {
            if get(convs, id).is_none() { -1 } else { 0 }
        }
        _ => -1,
    })
}

pub fn write(node: Node, src: &[u8]) -> i32 {
    with(|convs| match node {
        Node::Ctl(_, id) => {
            if get(convs, id).is_none() {
                return -1;
            }
            if ctl(convs, id, crate::trim_ctl(src)) {
                src.len() as i32
            } else {
                -1
            }
        }
        Node::Data(_, id) => {
            let Some(peer) = get(convs, id).and_then(|c| c.peer) else {
                return -1;
            };
            let p = &mut convs[peer as usize];
            let n = src.len().min(BUF_CAP - p.rx.len());
            // Full (or no heap for it): the library waits for the reader and
            // retries.
            if n == 0 || p.rx.try_reserve(n).is_err() {
                return -1;
            }
            p.rx.extend(&src[..n]);
            n as i32
        }
        _ => -1,
    })
}

fn ctl(convs: &mut Vec<Conv>, id: u16, cmd: &[u8]) -> bool {
    let open = convs[id as usize].state == State::Open;
    if cmd == b"hangup" {
        disconnect(convs, id);
        return true;
    }
    if cmd == b"pair" {
        let Some(other) = open.then(|| alloc(convs)).flatten() else {
            return false;
        };
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
        return true;
    }
    if let Some(name) = cmd.strip_prefix(b"connect ") {
        let Some(server) = listener(convs, name) else {
            return false;
        };
        if !open || convs[server as usize].queued == BACKLOG {
            return false;
        }
        let Some(end) = alloc(convs) else {
            return false;
        };
        link(convs, id, end);
        return enqueue(convs, server, end);
    }
    false
}

/// Last close of `data`: end the conversation, and the connections still
/// queued on it (nobody can open those any more).
pub fn release(node: Node) {
    let Node::Data(_, id) = node else {
        return;
    };
    with(|convs| {
        if get(convs, id).is_none() {
            return;
        }
        let c = &mut convs[id as usize];
        let (queue, queued) = (c.queue, c.queued);
        for &q in &queue[..queued] {
            disconnect(convs, q);
            convs[q as usize] = Conv::FREE;
        }
        disconnect(convs, id);
        convs[id as usize] = Conv::FREE;
    });
}
