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

pub fn on_fork(parent: usize, child: usize) {
    let mut t = PATHS.lock();
    t[child] = t[parent].clone();
}

pub fn on_spawn(slot: usize) {
    PATHS.lock()[slot] = Vec::new();
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
}

/// `new` now refers to what `old` does (dup/dup2/F_DUPFD).
pub fn dup(old: usize, new: usize) {
    match get(old) {
        Some(e) => put(FdPath { fd: new, pos: 0, ..e }),
        None => remove(new),
    }
}

pub fn set_pos(fd: usize, pos: usize) {
    let mut t = PATHS.lock();
    if let Some(e) = t[task::current_pid()].iter_mut().find(|e| e.fd == fd) {
        e.pos = pos;
    }
}
