//! Paths of the fds a Linux process opened, per task slot.
//!
//! Native fds carry no path, but `fstat`, `getdents64`, `fchdir` and the
//! `*at` calls need one, and a directory fd needs a read position. Paths are
//! the task's own (chroot-relative) absolute view. A socket's is its `/net`
//! conversation, with the socket's state.
//!
//! Lock order: the core calls `on_fork` and `on_spawn` with its task lock
//! held, so nothing here may enter the core (`task::current_pid` takes
//! that lock) while holding `PATHS` or `FLAGS`: the pid is read first.

use alloc::string::String;
use alloc::vec::Vec;
use myos_abi::Lock as Mutex;

use super::net::Sock;
use crate::k::task;
use crate::k::{MAX_FDS, MAX_TASKS};

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

/// A set of fds, a bit per fd.
#[derive(Clone, Copy)]
pub struct FdBits([u64; MAX_FDS / 64]);

impl FdBits {
    const EMPTY: Self = Self([0; MAX_FDS / 64]);

    pub fn contains(&self, fd: usize) -> bool {
        fd < MAX_FDS && self.0[fd / 64] & 1 << (fd % 64) != 0
    }

    fn set(&mut self, fd: usize, on: bool) {
        if fd < MAX_FDS {
            let (w, b) = (&mut self.0[fd / 64], 1 << (fd % 64));
            *w = if on { *w | b } else { *w & !b };
        }
    }
}

/// A process's fd flags: which fds close on exec (`FD_CLOEXEC`), which are
/// non-blocking (`O_NONBLOCK`, a socket's is in its `Sock`).
#[derive(Clone, Copy)]
struct Flags {
    cloexec: FdBits,
    nonblock: FdBits,
    /// The signal fds (`sys::signalfd4`): a read of one is translated into
    /// Linux's `struct signalfd_siginfo`.
    sigfd: FdBits,
}

const NO_FLAGS: Flags = Flags { cloexec: FdBits::EMPTY, nonblock: FdBits::EMPTY, sigfd: FdBits::EMPTY };

static FLAGS: Mutex<[Flags; MAX_TASKS]> = Mutex::new([NO_FLAGS; MAX_TASKS]);

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

/// The calling process's flags.
fn with_flags<T>(f: impl FnOnce(&mut Flags) -> T) -> T {
    let me = task::current_pid();
    f(&mut FLAGS.lock()[me])
}

/// Set fd's close-on-exec flag.
pub fn set_cloexec(fd: usize, on: bool) {
    with_flags(|e| e.cloexec.set(fd, on));
}

pub fn cloexec(fd: usize) -> bool {
    with_flags(|e| e.cloexec.contains(fd))
}

pub fn set_nonblock(fd: usize, on: bool) {
    with_flags(|e| e.nonblock.set(fd, on));
}

pub fn nonblock(fd: usize) -> bool {
    with_flags(|e| e.nonblock.contains(fd))
}

pub fn set_sigfd(fd: usize, on: bool) {
    with_flags(|e| e.sigfd.set(fd, on));
}

pub fn sigfd(fd: usize) -> bool {
    with_flags(|e| e.sigfd.contains(fd))
}

/// A successful exec in `slot`: the fds to close now (and forget).
pub fn take_cloexec(slot: usize) -> FdBits {
    let mut f = FLAGS.lock();
    core::mem::replace(&mut f[slot].cloexec, FdBits::EMPTY)
}

pub fn get(fd: usize) -> Option<FdPath> {
    let me = task::current_pid();
    PATHS.lock()[me].iter().find(|e| e.fd == fd).cloned()
}

pub fn set(fd: usize, path: String, dir: bool) {
    put(FdPath { fd, path, dir, pos: 0, sock: None });
}

pub fn set_sock(fd: usize, conv: String, sock: Sock) {
    put(FdPath { fd, path: conv, dir: false, pos: 0, sock: Some(sock) });
}

fn put(e: FdPath) {
    let me = task::current_pid();
    let mut t = PATHS.lock();
    let v = &mut t[me];
    v.retain(|x| x.fd != e.fd);
    v.push(e);
}

/// Change the state of socket `fd`; `None` if it is not a socket.
pub fn with_sock<T>(fd: usize, f: impl FnOnce(&mut Sock) -> T) -> Option<T> {
    let me = task::current_pid();
    let mut t = PATHS.lock();
    t[me].iter_mut().find(|e| e.fd == fd)?.sock.as_mut().map(f)
}

pub fn remove(fd: usize) {
    let me = task::current_pid();
    PATHS.lock()[me].retain(|e| e.fd != fd);
    set_cloexec(fd, false);
    set_nonblock(fd, false);
    set_sigfd(fd, false);
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
    set_sigfd(new, sigfd(old));
}

pub fn set_pos(fd: usize, pos: usize) {
    let me = task::current_pid();
    let mut t = PATHS.lock();
    if let Some(e) = t[me].iter_mut().find(|e| e.fd == fd) {
        e.pos = pos;
    }
}
