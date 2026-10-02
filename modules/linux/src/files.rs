//! Paths of the fds a Linux process opened, per task slot.
//!
//! Native fds carry no path, but `fstat`, `getdents64`, `fchdir` and the
//! `*at` calls need one, and a directory fd needs a read position. Paths are
//! the task's own (chroot-relative) absolute view.

use alloc::string::String;
use alloc::vec::Vec;
use crate::lock::Lock as Mutex;

use crate::k::task;
use crate::k::MAX_TASKS;

#[derive(Clone)]
pub struct FdPath {
    pub fd: usize,
    pub path: String,
    pub dir: bool,
    /// Directory read position (entries already returned).
    pub pos: usize,
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
    let mut t = PATHS.lock();
    let v = &mut t[task::current_pid()];
    v.retain(|e| e.fd != fd);
    v.push(FdPath { fd, path, dir, pos: 0 });
}

pub fn remove(fd: usize) {
    PATHS.lock()[task::current_pid()].retain(|e| e.fd != fd);
}

/// `new` now refers to what `old` does (dup/dup2/F_DUPFD).
pub fn dup(old: usize, new: usize) {
    match get(old) {
        Some(e) => set(new, e.path, e.dir),
        None => remove(new),
    }
}

pub fn set_pos(fd: usize, pos: usize) {
    let mut t = PATHS.lock();
    if let Some(e) = t[task::current_pid()].iter_mut().find(|e| e.fd == fd) {
        e.pos = pos;
    }
}
