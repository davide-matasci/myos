//! Nodes: the files the system holds on to (an open file, a mapping, a
//! running program's pages) by identity rather than by name.
//!
//! A [`Vnode`] is a counted reference to an entry here. A filesystem with
//! file ids (tmpfs, ext2: `vfs::FileOps`) gave the entry its file's id at
//! open: every read, write and stat goes by it, and two opens of the file
//! share the entry whatever name they came by. On the others (devices,
//! procfs, netfs, fat) the entry finds its file by its path in the mount.
//! Either way the entry knows its file's name now, for `/proc/self/fd`, the
//! cwd and a directory fd: a rename moves the entries at and below the old
//! name ([`moved`]). A file unlinked while it is referenced is kept by its
//! id until the last reference goes ([`hide`], then the VFS has its
//! filesystem forget it); one without an id is dead ([`kill`]): its reads
//! fail rather than reach a new file that took the name.
//!
//! [`NODES`] is a leaf lock: nothing else is taken while it is held, so a
//! reference may be dropped anywhere (under `TASKS` too).

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use super::vfs::PATH_MAX;

struct Node {
    mount: u16,
    /// Where the file is in its mount (for a hidden one: where it was).
    rel: String,
    /// The id its filesystem gave the file (`vfs::FileOps::id`), if it
    /// gives them.
    file: Option<u64>,
    /// Unlinked, kept by its id for its holders.
    hidden: bool,
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
        !self.dead && !self.hidden && self.mount as usize == mount && self.rel == rel
    }
}

static NODES: Mutex<Vec<Option<Node>>> = Mutex::new(Vec::new());

/// Kept files (mount, file id) whose last reference went, for the VFS to
/// have their filesystem forget outside every lock.
static REAP: Mutex<Vec<(u16, u64)>> = Mutex::new(Vec::new());

/// A reference to a file (see the module doc). Cloning one takes another
/// reference; dropping the last frees the entry.
pub struct Vnode {
    id: u32,
}

impl Vnode {
    /// A number for the file while the node lives (file locks key on it).
    pub fn key(&self) -> usize {
        self.id as usize
    }
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
        if let (true, Some(file)) = (n.hidden, n.file) {
            REAP.lock().push((n.mount, file));
        }
    }
}

/// A reference to the file at `rel` of `mount`, whose filesystem gave it
/// the id `file` (if it gives ids): the entry it already has, by its id or
/// else by its name, or a new one.
pub(super) fn get(mount: usize, rel: &str, file: Option<u64>) -> Vnode {
    let mut nodes = NODES.lock();
    let same = |n: &Node| match file {
        Some(f) => !n.dead && !n.hidden && n.mount as usize == mount && n.file == Some(f),
        None => n.named(mount, rel),
    };
    if let Some(id) = nodes.iter().position(|n| n.as_ref().is_some_and(same)) {
        nodes[id].as_mut().unwrap().refs += 1;
        return Vnode { id: id as u32 };
    }
    let node = Node { mount: mount as u16, rel: String::from(rel), file, hidden: false, dead: false, refs: 1, opens: 0 };
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

/// How to reach a node's file: by the id its filesystem gave it, or by its
/// path in the mount.
pub enum Loc {
    File(u64),
    Path(Rel),
}

/// How to reach `node`'s file now: its mount and its id, or its path there.
/// `None` once it is dead.
pub(super) fn locate(node: &Vnode) -> Option<(usize, Loc)> {
    let nodes = NODES.lock();
    let n = nodes.get(node.id as usize)?.as_ref()?;
    if n.dead {
        return None;
    }
    if let Some(file) = n.file {
        return Some((n.mount as usize, Loc::File(file)));
    }
    Some((n.mount as usize, Loc::Path(copy_rel(&n.rel)?)))
}

fn copy_rel(name: &str) -> Option<Rel> {
    let mut rel = Rel { len: name.len(), buf: [0; PATH_MAX] };
    rel.buf.get_mut(..name.len())?.copy_from_slice(name.as_bytes());
    Some(rel)
}

/// Where `node`'s file is named now: its mount and path there. `None` once
/// it is dead or unlinked.
pub(super) fn location(node: &Vnode) -> Option<(usize, Rel)> {
    let nodes = NODES.lock();
    let n = nodes.get(node.id as usize)?.as_ref()?;
    if n.dead || n.hidden {
        return None;
    }
    Some((n.mount as usize, copy_rel(&n.rel)?))
}

/// `node`'s mount, the path it has (or had) there, and whether it is gone
/// from it (unlinked or dead): what `/proc/self/fd/N` shows.
pub(super) fn name(node: &Vnode) -> Option<(usize, String, bool)> {
    let nodes = NODES.lock();
    let n = nodes.get(node.id as usize)?.as_ref()?;
    Some((n.mount as usize, n.rel.clone(), n.dead || n.hidden))
}

/// `rel` lies at or below `dir` in a mount.
fn at_or_below(rel: &str, dir: &str) -> bool {
    rel == dir || (rel.starts_with(dir) && rel.as_bytes().get(dir.len()) == Some(&b'/'))
}

/// `old` of `mount` is now `new`: so are the files below it.
pub(super) fn moved(mount: usize, old: &str, new: &str) {
    for n in NODES.lock().iter_mut().flatten() {
        if n.mount as usize == mount && !n.dead && !n.hidden && at_or_below(&n.rel, old) {
            n.rel = alloc::format!("{new}{}", &n.rel[old.len()..]);
        }
    }
}

/// Something holds the file at `rel` of `mount`.
pub(super) fn referenced(mount: usize, rel: &str) -> bool {
    NODES.lock().iter().flatten().any(|n| n.named(mount, rel))
}

/// The file at `rel` of `mount` was unlinked but is kept by its id.
pub(super) fn hide(mount: usize, rel: &str) {
    for n in NODES.lock().iter_mut().flatten() {
        if n.named(mount, rel) && n.file.is_some() {
            n.hidden = true;
        }
    }
}

/// The files at and below `rel` of `mount` are gone.
pub(super) fn kill(mount: usize, rel: &str) {
    for n in NODES.lock().iter_mut().flatten() {
        if n.mount as usize == mount && !n.hidden && at_or_below(&n.rel, rel) {
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

/// The kept files nothing references any more: (mount, file id).
pub(super) fn take_reaped() -> Vec<(u16, u64)> {
    core::mem::take(&mut *REAP.lock())
}
