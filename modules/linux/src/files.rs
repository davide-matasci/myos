//! Paths of the fds a Linux process opened, per task slot.
//!
//! Native fds carry no path, but `fstat`, `getdents64`, `fchdir` and the
//! `*at` calls need one, and a directory fd needs a read position. Paths are
//! the task's own (chroot-relative) absolute view. A socket's is its `/net`
//! conversation, with the socket's state.

use alloc::string::String;
use alloc::vec::Vec;
use crate::lock::Lock as Mutex;

use super::net::Sock;
use crate::k::task;
use crate::k::MAX_TASKS;

#[derive(Clone)]
pub struct FdPath {
    pub fd: usize,
    pub path: String,
    pub dir: bool,
    /// Directory read position (entries already returned).
    pub pos: usize,
    /// A socket: `path` is its `/net` conversation (see `net`).
    pub sock: Option<Sock>,
}

static PATHS: Mutex<[Vec<FdPath>; MAX_TASKS]> = Mutex::new([const { Vec::new() }; MAX_TASKS]);

/// A process's fd flags, a bit per fd (native fds are below 64): which
/// close on exec (`FD_CLOEXEC`), which are non-blocking (`O_NONBLOCK`,
/// a socket's is in its `Sock`).
#[derive(Clone, Copy)]
struct Flags {
    cloexec: u64,
    nonblock: u64,
    /// Which fds are eventfds (`sys::eventfd2`).
    event: u64,
    /// For a pipe read end that stands for an eventfd or a socketpair end:
    /// the hidden pipe write end its writes go to (fd + 1).
    writer: [u8; 64],
}

const NO_FLAGS: Flags = Flags { cloexec: 0, nonblock: 0, event: 0, writer: [0; 64] };

static FLAGS: Mutex<[Flags; MAX_TASKS]> = Mutex::new([NO_FLAGS; MAX_TASKS]);

fn bit(fd: usize) -> u64 {
    if fd < 64 { 1 << fd } else { 0 }
}

pub fn on_fork(parent: usize, child: usize) {
    let mut t = PATHS.lock();
    t[child] = t[parent].clone();
    let mut f = FLAGS.lock();
    f[child] = f[parent];
}

pub fn on_spawn(slot: usize) {
    PATHS.lock()[slot] = Vec::new();
    FLAGS.lock()[slot] = NO_FLAGS;
}

/// `fd` and the hidden write end behind it, if any: they share their flags.
fn with_writer(e: &Flags, fd: usize) -> u64 {
    match e.writer.get(fd) {
        Some(&w) if w != 0 => bit(fd) | bit(w as usize - 1),
        _ => bit(fd),
    }
}

/// Set fd's close-on-exec flag.
pub fn set_cloexec(fd: usize, on: bool) {
    let mut f = FLAGS.lock();
    let e = &mut f[task::current_pid()];
    let b = with_writer(e, fd);
    e.cloexec = if on { e.cloexec | b } else { e.cloexec & !b };
}

pub fn cloexec(fd: usize) -> bool {
    FLAGS.lock()[task::current_pid()].cloexec & bit(fd) != 0
}

pub fn set_nonblock(fd: usize, on: bool) {
    let mut f = FLAGS.lock();
    let e = &mut f[task::current_pid()];
    let b = with_writer(e, fd);
    e.nonblock = if on { e.nonblock | b } else { e.nonblock & !b };
}

pub fn nonblock(fd: usize) -> bool {
    FLAGS.lock()[task::current_pid()].nonblock & bit(fd) != 0
}

/// The hidden write end behind `fd`, if it has one.
pub fn writer(fd: usize) -> Option<usize> {
    let w = *FLAGS.lock()[task::current_pid()].writer.get(fd)?;
    (w != 0).then(|| w as usize - 1)
}

pub fn set_writer(fd: usize, writer: Option<usize>) {
    if let Some(slot) = FLAGS.lock()[task::current_pid()].writer.get_mut(fd) {
        *slot = writer.map_or(0, |w| w as u8 + 1);
    }
}

pub fn set_event(fd: usize, on: bool) {
    let mut f = FLAGS.lock();
    let e = &mut f[task::current_pid()];
    e.event = if on { e.event | bit(fd) } else { e.event & !bit(fd) };
}

pub fn event(fd: usize) -> bool {
    FLAGS.lock()[task::current_pid()].event & bit(fd) != 0
}

/// A socketpair end (`net::socketpair`): a hidden writer, not an eventfd.
pub fn pair(fd: usize) -> bool {
    writer(fd).is_some() && !event(fd)
}

/// A successful exec in `slot`: the fds to close now (and forget).
pub fn take_cloexec(slot: usize) -> u64 {
    let mut f = FLAGS.lock();
    core::mem::replace(&mut f[slot].cloexec, 0)
}

pub fn get(fd: usize) -> Option<FdPath> {
    PATHS.lock()[task::current_pid()].iter().find(|e| e.fd == fd).cloned()
}

pub fn set(fd: usize, path: String, dir: bool) {
    put(FdPath { fd, path, dir, pos: 0, sock: None });
}

pub fn set_sock(fd: usize, conv: String, sock: Sock) {
    put(FdPath { fd, path: conv, dir: false, pos: 0, sock: Some(sock) });
}

fn put(e: FdPath) {
    let mut t = PATHS.lock();
    let v = &mut t[task::current_pid()];
    v.retain(|x| x.fd != e.fd);
    v.push(e);
}

/// Change the state of socket `fd`; `None` if it is not a socket.
pub fn with_sock<T>(fd: usize, f: impl FnOnce(&mut Sock) -> T) -> Option<T> {
    let mut t = PATHS.lock();
    t[task::current_pid()].iter_mut().find(|e| e.fd == fd)?.sock.as_mut().map(f)
}

pub fn remove(fd: usize) {
    PATHS.lock()[task::current_pid()].retain(|e| e.fd != fd);
    set_cloexec(fd, false);
    set_nonblock(fd, false);
    set_writer(fd, None);
    set_event(fd, false);
}

/// `new` now refers to what `old` does (dup/dup2/F_DUPFD), without
/// close-on-exec; `O_NONBLOCK` belongs to what both refer to.
pub fn dup(old: usize, new: usize) {
    let nb = nonblock(old);
    match get(old) {
        Some(e) => put(FdPath { fd: new, pos: 0, ..e }),
        None => remove(new),
    }
    set_cloexec(new, false);
    set_nonblock(new, nb);
}

pub fn set_pos(fd: usize, pos: usize) {
    let mut t = PATHS.lock();
    if let Some(e) = t[task::current_pid()].iter_mut().find(|e| e.fd == fd) {
        e.pos = pos;
    }
}
