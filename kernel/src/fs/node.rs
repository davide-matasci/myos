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
//! reference may be dropped anywhere (under `TASKS` too). It is taken with
//! interrupts off ([`lock`]), as are [`REAP`] and [`ORPHANS`]: the page
//! cache takes it inside its own interrupts-off lock (`pagecache::locked`:
//! a reference cloned or dropped, `set_cached`), and a holder preempted on
//! that CPU could never run again while the cache spun for it.

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
    /// The page cache holds one of the `refs` (it has pages of the file).
    cached: bool,
}

impl Node {
    /// Unlinked, and the page cache is all that holds it: its pages are
    /// to go ([`take_orphans`]), and the file with them.
    fn orphaned(&self) -> bool {
        self.hidden && self.cached && self.refs == 1
    }
}

impl Node {
    /// Reachable by its name: not dead, not hidden.
    fn named(&self, mount: usize, rel: &str) -> bool {
        !self.dead && !self.hidden && self.mount as usize == mount && self.rel == rel
    }
}

static NODES: Mutex<Vec<Option<Node>>> = Mutex::new(Vec::new());

/// A lock held with interrupts off on this CPU, restored when it goes.
struct Held<T: 'static> {
    guard: Option<spin::MutexGuard<'static, T>>,
    flags: u64,
}

impl<T> core::ops::Deref for Held<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.guard.as_ref().unwrap()
    }
}

impl<T> core::ops::DerefMut for Held<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.guard.as_mut().unwrap()
    }
}

impl<T> Drop for Held<T> {
    fn drop(&mut self) {
        // The lock goes before interrupts come back: a tick in between
        // could switch to a task that spins for it.
        self.guard = None;
        crate::arch::irq_restore(self.flags);
    }
}

/// Take `lock` with interrupts off (see the module doc).
fn lock<T>(lock: &'static Mutex<T>) -> Held<T> {
    let flags = crate::arch::irq_save();
    crate::arch::irq_off();
    Held { guard: Some(lock.lock()), flags }
}

/// Kept files (mount, file id) whose last reference went, for the VFS to
/// have their filesystem forget outside every lock.
static REAP: Mutex<Vec<(u16, u64)>> = Mutex::new(Vec::new());

/// Unlinked files the page cache alone holds ([`Node::orphaned`]), for it
/// to let go of (`pagecache::reap_hidden`, from the VFS's `reap`).
static ORPHANS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

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
        if let Some(Some(n)) = lock(&NODES).get_mut(self.id as usize) {
            n.refs += 1;
        }
        Vnode { id: self.id }
    }
}

impl Drop for Vnode {
    fn drop(&mut self) {
        let mut nodes = lock(&NODES);
        let Some(slot) = nodes.get_mut(self.id as usize) else {
            return;
        };
        let Some(n) = slot.as_mut() else {
            return;
        };
        n.refs -= 1;
        if n.refs > 0 {
            let orphaned = n.orphaned();
            drop(nodes);
            if orphaned {
                lock(&ORPHANS).push(self.id);
            }
            return;
        }
        let n = slot.take().unwrap();
        drop(nodes);
        if let (true, Some(file)) = (n.hidden, n.file) {
            lock(&REAP).push((n.mount, file));
        }
    }
}

/// A reference to the file at `rel` of `mount`, whose filesystem gave it
/// the id `file` (if it gives ids): the entry it already has, by its id or
/// else by its name, or a new one.
pub(super) fn get(mount: usize, rel: &str, file: Option<u64>) -> Vnode {
    let mut nodes = lock(&NODES);
    let same = |n: &Node| match file {
        Some(f) => !n.dead && !n.hidden && n.mount as usize == mount && n.file == Some(f),
        None => n.named(mount, rel),
    };
    if let Some(id) = nodes.iter().position(|n| n.as_ref().is_some_and(same)) {
        nodes[id].as_mut().unwrap().refs += 1;
        return Vnode { id: id as u32 };
    }
    let node = Node { mount: mount as u16, rel: String::from(rel), file, hidden: false, dead: false, refs: 1, opens: 0, cached: false };
    Vnode { id: insert(&mut nodes, node) as u32 }
}

fn insert(nodes: &mut Vec<Option<Node>>, node: Node) -> usize {
    match nodes.iter().position(Option::is_none) {
        Some(id) => {
            nodes[id] = Some(node);
            id
        }
        None => {
            nodes.push(Some(node));
            nodes.len() - 1
        }
    }
}

/// A reference to a file of `mount` with the id `file` that no name
/// reaches (`vfs::anon_file`): kept, as an unlinked file is, while it is
/// referenced.
pub(super) fn kept(mount: usize, file: u64) -> Vnode {
    let node = Node { mount: mount as u16, rel: String::new(), file: Some(file), hidden: true, dead: false, refs: 1, opens: 0, cached: false };
    Vnode { id: insert(&mut lock(&NODES), node) as u32 }
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
    let nodes = lock(&NODES);
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
    let nodes = lock(&NODES);
    let n = nodes.get(node.id as usize)?.as_ref()?;
    if n.dead || n.hidden {
        return None;
    }
    Some((n.mount as usize, copy_rel(&n.rel)?))
}

/// `node`'s mount, the path it has (or had) there, and whether it is gone
/// from it (unlinked or dead): what `/proc/self/fd/N` shows.
pub(super) fn name(node: &Vnode) -> Option<(usize, String, bool)> {
    let nodes = lock(&NODES);
    let n = nodes.get(node.id as usize)?.as_ref()?;
    Some((n.mount as usize, n.rel.clone(), n.dead || n.hidden))
}

/// `rel` lies at or below `dir` in a mount.
fn at_or_below(rel: &str, dir: &str) -> bool {
    rel == dir || (rel.starts_with(dir) && rel.as_bytes().get(dir.len()) == Some(&b'/'))
}

/// `old` of `mount` is now `new`: so are the files below it.
pub(super) fn moved(mount: usize, old: &str, new: &str) {
    for n in lock(&NODES).iter_mut().flatten() {
        if n.mount as usize == mount && !n.dead && !n.hidden && at_or_below(&n.rel, old) {
            n.rel = alloc::format!("{new}{}", &n.rel[old.len()..]);
        }
    }
}

/// Something holds the file at `rel` of `mount`.
pub(super) fn referenced(mount: usize, rel: &str) -> bool {
    lock(&NODES).iter().flatten().any(|n| n.named(mount, rel))
}

/// Something other than the page cache holds the file at `rel` of `mount`
/// (an fd, a mapping, a cwd).
pub(super) fn held(mount: usize, rel: &str) -> bool {
    lock(&NODES).iter().flatten().any(|n| n.named(mount, rel) && (n.opens > 0 || n.refs > n.cached as u32))
}

/// The page cache has pages of `node`'s file, or has let go of them. A
/// file it takes that is unlinked and otherwise unheld already is an
/// orphan from the start.
pub(super) fn set_cached(node: &Vnode, cached: bool) {
    let orphaned = {
        let mut nodes = lock(&NODES);
        let Some(Some(n)) = nodes.get_mut(node.id as usize) else {
            return;
        };
        n.cached = cached;
        n.orphaned()
    };
    if orphaned {
        lock(&ORPHANS).push(node.id);
    }
}

/// The file at `rel` of `mount` was unlinked but is kept by its id.
pub(super) fn hide(mount: usize, rel: &str) {
    let mut orphans = Vec::new();
    for (id, n) in lock(&NODES).iter_mut().enumerate() {
        if let Some(n) = n {
            if n.named(mount, rel) && n.file.is_some() {
                n.hidden = true;
                if n.orphaned() {
                    orphans.push(id as u32);
                }
            }
        }
    }
    lock(&ORPHANS).append(&mut orphans);
}

/// The unlinked files the page cache alone holds, noted since the last
/// call ([`orphaned`] says whether each still is).
pub(super) fn take_orphans() -> Vec<u32> {
    core::mem::take(&mut *lock(&ORPHANS))
}

/// Node `id` is an unlinked file the page cache alone holds.
pub(super) fn orphaned(id: u32) -> bool {
    matches!(lock(&NODES).get(id as usize), Some(Some(n)) if n.orphaned())
}

/// The files at and below `rel` of `mount` are gone.
pub(super) fn kill(mount: usize, rel: &str) {
    for n in lock(&NODES).iter_mut().flatten() {
        if n.mount as usize == mount && !n.hidden && at_or_below(&n.rel, rel) {
            n.dead = true;
        }
    }
}

/// One more open file description on `node`.
pub(super) fn opened(node: &Vnode) {
    if let Some(Some(n)) = lock(&NODES).get_mut(node.id as usize) {
        n.opens += 1;
    }
}

/// One open file description on `node` fewer: true for the last.
pub(super) fn closed(node: &Vnode) -> bool {
    match lock(&NODES).get_mut(node.id as usize) {
        Some(Some(n)) if n.opens > 0 => {
            n.opens -= 1;
            n.opens == 0
        }
        _ => false,
    }
}

/// Something holds a file of `mount` (it cannot be unmounted).
pub(super) fn on_mount(mount: usize) -> bool {
    lock(&NODES).iter().flatten().any(|n| n.mount as usize == mount)
}

/// The open file descriptions on `rel` of `mount`.
pub(super) fn opens_at(mount: usize, rel: &str) -> u32 {
    lock(&NODES).iter().flatten().filter(|n| n.named(mount, rel)).map(|n| n.opens).sum()
}

/// The kept files nothing references any more: (mount, file id).
pub(super) fn take_reaped() -> Vec<(u16, u64)> {
    core::mem::take(&mut *lock(&REAP))
}
