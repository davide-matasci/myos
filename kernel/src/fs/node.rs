//! Nodes: the files the system holds on to (an open file, a mapping, a
//! running program's pages) by identity rather than by name.
//!
//! The filesystems below the VFS find a file by its path in the mount. A
//! [`Vnode`] is a counted reference to an entry here that knows where its
//! file is now: a rename moves the entries at and below the old name
//! ([`moved`]); a file unlinked while it is referenced is kept until the
//! last reference goes ([`hide`], then the VFS reaps it): tmpfs keeps it
//! under a name no path can reach, a module filesystem (ext2) by its inode
//! number; one its filesystem cannot keep is dead ([`kill`]): its reads
//! fail rather than reach a new file that took the name. Two opens of one
//! file share its entry.
//!
//! [`NODES`] is a leaf lock: nothing else is taken while it is held, so a
//! reference may be dropped anywhere (under `TASKS` too).

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use super::vfs::PATH_MAX;

/// How a file unlinked while held is kept ([`hide`]).
#[derive(Clone)]
pub(super) enum Kept {
    /// Under a name no path reaches ([`hidden_name`]).
    Name(String),
    /// By its inode number (`ModuleVfsOps::unlink_keep`).
    Ino(u64),
}

struct Node {
    mount: u16,
    /// Where the file is in its mount (for a hidden one: where it was).
    rel: String,
    /// How it is kept after an unlink.
    hidden: Option<Kept>,
    /// Its filesystem no longer has it.
    dead: bool,
    /// The [`Vnode`]s on it.
    refs: u32,
    /// The open file descriptions on it (`open_ref`): its module's
    /// `release` hook runs when the last one closes.
    opens: u32,
}

impl Node {
    /// Reachable by its name: not dead, not hidden.
    fn named(&self, mount: usize, rel: &str) -> bool {
        !self.dead && self.hidden.is_none() && self.mount as usize == mount && self.rel == rel
    }
}

static NODES: Mutex<Vec<Option<Node>>> = Mutex::new(Vec::new());

/// Kept files whose last reference went, for the VFS to remove outside
/// every lock.
static REAP: Mutex<Vec<(u16, Kept)>> = Mutex::new(Vec::new());

/// Numbers the hidden names: unique until reboot.
static HIDDEN_SEQ: AtomicU64 = AtomicU64::new(0);

/// A reference to a file (see the module doc). Cloning one takes another
/// reference; dropping the last frees the entry.
pub struct Vnode {
    id: u32,
}

impl PartialEq for Vnode {
    fn eq(&self, other: &Vnode) -> bool {
        self.id == other.id
    }
}

impl Eq for Vnode {}

impl core::fmt::Debug for Vnode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Vnode({})", self.id)
    }
}

impl Clone for Vnode {
    fn clone(&self) -> Vnode {
        if let Some(Some(n)) = NODES.lock().get_mut(self.id as usize) {
            n.refs += 1;
        }
        Vnode { id: self.id }
    }
}

impl Drop for Vnode {
    fn drop(&mut self) {
        let mut nodes = NODES.lock();
        let Some(slot) = nodes.get_mut(self.id as usize) else {
            return;
        };
        let Some(n) = slot.as_mut() else {
            return;
        };
        n.refs -= 1;
        if n.refs > 0 {
            return;
        }
        let n = slot.take().unwrap();
        drop(nodes);
        if let Some(hidden) = n.hidden {
            REAP.lock().push((n.mount, hidden));
        }
    }
}

/// A reference to the file at `rel` of `mount`: the entry it already has,
/// else a new one.
pub(super) fn get(mount: usize, rel: &str) -> Vnode {
    let mut nodes = NODES.lock();
    if let Some(id) = nodes.iter().position(|n| n.as_ref().is_some_and(|n| n.named(mount, rel))) {
        nodes[id].as_mut().unwrap().refs += 1;
        return Vnode { id: id as u32 };
    }
    let node = Node { mount: mount as u16, rel: String::from(rel), hidden: None, dead: false, refs: 1, opens: 0 };
    let id = match nodes.iter().position(Option::is_none) {
        Some(id) => {
            nodes[id] = Some(node);
            id
        }
        None => {
            nodes.push(Some(node));
            nodes.len() - 1
        }
    };
    Vnode { id: id as u32 }
}

/// A path in a mount, copied out of the table (no allocation per read).
pub struct Rel {
    len: usize,
    buf: [u8; PATH_MAX],
}

impl Rel {
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

/// Where a node's file is: a path in its mount, or the inode number a
/// module filesystem keeps an unlinked one by.
pub enum Loc {
    Path(Rel),
    Ino(u64),
}

/// Where `node`'s file is now: its mount and where in it. `None` once it is
/// dead.
pub(super) fn locate(node: &Vnode) -> Option<(usize, Loc)> {
    let nodes = NODES.lock();
    let n = nodes.get(node.id as usize)?.as_ref()?;
    if n.dead {
        return None;
    }
    let name = match &n.hidden {
        Some(Kept::Ino(ino)) => return Some((n.mount as usize, Loc::Ino(*ino))),
        Some(Kept::Name(name)) => name,
        None => &n.rel,
    };
    let mut rel = Rel { len: name.len(), buf: [0; PATH_MAX] };
    rel.buf.get_mut(..name.len())?.copy_from_slice(name.as_bytes());
    Some((n.mount as usize, Loc::Path(rel)))
}

/// [`locate`] for a file reached by a path (not one kept by its inode).
pub(super) fn location(node: &Vnode) -> Option<(usize, Rel)> {
    match locate(node)? {
        (idx, Loc::Path(rel)) => Some((idx, rel)),
        (_, Loc::Ino(_)) => None,
    }
}

/// `node`'s mount, the path it has (or had) there, and whether it is gone
/// from it (unlinked or dead): what `/proc/self/fd/N` shows.
pub(super) fn name(node: &Vnode) -> Option<(usize, String, bool)> {
    let nodes = NODES.lock();
    let n = nodes.get(node.id as usize)?.as_ref()?;
    Some((n.mount as usize, n.rel.clone(), n.dead || n.hidden.is_some()))
}

/// `rel` lies at or below `dir` in a mount.
fn at_or_below(rel: &str, dir: &str) -> bool {
    rel == dir || (rel.starts_with(dir) && rel.as_bytes().get(dir.len()) == Some(&b'/'))
}

/// `old` of `mount` is now `new`: so are the files below it.
pub(super) fn moved(mount: usize, old: &str, new: &str) {
    for n in NODES.lock().iter_mut().flatten() {
        if n.mount as usize == mount && !n.dead && n.hidden.is_none() && at_or_below(&n.rel, old) {
            n.rel = alloc::format!("{new}{}", &n.rel[old.len()..]);
        }
    }
}

/// Something holds the file at `rel` of `mount`.
pub(super) fn referenced(mount: usize, rel: &str) -> bool {
    NODES.lock().iter().flatten().any(|n| n.named(mount, rel))
}

/// A fresh name for [`hide`]: it holds a NUL, which no path can (the VFS
/// refuses one), at the mount's root.
pub(super) fn hidden_name() -> String {
    alloc::format!("\0unlinked{}", HIDDEN_SEQ.fetch_add(1, Ordering::Relaxed))
}

/// The file at `rel` of `mount` was unlinked but is kept.
pub(super) fn hide(mount: usize, rel: &str, kept: &Kept) {
    for n in NODES.lock().iter_mut().flatten() {
        if n.named(mount, rel) {
            n.hidden = Some(kept.clone());
        }
    }
}

/// The file kept as `hidden` of `mount` is back under its name.
pub(super) fn unhide(mount: usize, hidden: &str) {
    for n in NODES.lock().iter_mut().flatten() {
        if n.mount as usize == mount && matches!(&n.hidden, Some(Kept::Name(h)) if h == hidden) {
            n.hidden = None;
        }
    }
}

/// The files at and below `rel` of `mount` are gone.
pub(super) fn kill(mount: usize, rel: &str) {
    for n in NODES.lock().iter_mut().flatten() {
        if n.mount as usize == mount && n.hidden.is_none() && at_or_below(&n.rel, rel) {
            n.dead = true;
        }
    }
}

/// One more open file description on `node`.
pub(super) fn opened(node: &Vnode) {
    if let Some(Some(n)) = NODES.lock().get_mut(node.id as usize) {
        n.opens += 1;
    }
}

/// One open file description on `node` fewer: true for the last.
pub(super) fn closed(node: &Vnode) -> bool {
    match NODES.lock().get_mut(node.id as usize) {
        Some(Some(n)) if n.opens > 0 => {
            n.opens -= 1;
            n.opens == 0
        }
        _ => false,
    }
}

/// Something holds a file of `mount` (it cannot be unmounted).
pub(super) fn on_mount(mount: usize) -> bool {
    NODES.lock().iter().flatten().any(|n| n.mount as usize == mount)
}

/// The open file descriptions on `rel` of `mount`.
pub(super) fn opens_at(mount: usize, rel: &str) -> u32 {
    NODES.lock().iter().flatten().filter(|n| n.named(mount, rel)).map(|n| n.opens).sum()
}

/// The kept files nothing references any more.
pub(super) fn take_reaped() -> Vec<(u16, Kept)> {
    core::mem::take(&mut *REAP.lock())
}
