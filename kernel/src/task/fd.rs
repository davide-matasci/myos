//! Per-task file descriptor table: open/dup/close, pipes, FIFOs, ptys,
//! read/write/lseek. An fd on a file refers to
//! an open file description in [`OPEN_FILES`], shared with its `dup`s and
//! a fork's copies the POSIX way.

use super::*;
use spin::Mutex;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FdEntry {
    Empty,
    Stdin,
    Console,
    /// An open file: the index of its description in [`OPEN_FILES`].
    File(usize),
    PipeRead(usize),
    PipeWrite(usize),
    /// PTY master end (`/dev/pts/N/master`, from opening `/dev/pts/clone`):
    /// writes feed slave input, reads drain slave output. Refcounted like
    /// pipes.
    PtyMaster(usize),
    /// PTY slave end (`/dev/pts/N/data`): the session-side tty.
    PtySlave(usize),
}

/// One open file description (POSIX): the node, the file offset and the
/// open flags, shared by every fd that refers to it. A fork's and a dup's
/// copies of an fd point at the same description, so a child's writes
/// advance the offset the parent writes at next (`prog > file` keeps what
/// the children wrote) and `lseek` moves it for all of them. An `open`
/// makes a new one.
struct OpenFile {
    node: crate::fs::Vnode,
    pos: usize,
    writable: bool,
    append: bool,
    /// What it lets its holder do beneath it, for a directory: the rights
    /// its opener's namespace had there (docs/security.md). A path relative
    /// to a directory fd its holder's namespace cannot name is checked
    /// against these instead.
    rights: crate::sec::Rights,
    /// The fds referring to it, over every process.
    refs: u32,
}

/// Open file descriptions in the whole system (docs/linux-compat.md lists
/// the limits with the fds per process).
const MAX_OPEN_FILES: usize = 512;

/// Taken after `TASKS` when both are held (`fd_clone` runs under it), never
/// the other way round.
static OPEN_FILES: Mutex<[Option<OpenFile>; MAX_OPEN_FILES]> =
    Mutex::new([const { None }; MAX_OPEN_FILES]);

/// A new description with one reference; `None` when the table is full.
fn open_file_alloc(node: crate::fs::Vnode, writable: bool, append: bool, rights: crate::sec::Rights) -> Option<usize> {
    let mut files = OPEN_FILES.lock();
    let id = files.iter().position(Option::is_none)?;
    files[id] = Some(OpenFile { node, pos: 0, writable, append, rights, refs: 1 });
    Some(id)
}

fn open_file_ref(id: usize) {
    if let Some(Some(f)) = OPEN_FILES.lock().get_mut(id) {
        f.refs += 1;
    }
}

/// Drop one reference; the node of a description that just went away, for
/// the caller to `close_ref` outside the lock.
fn open_file_unref(id: usize) -> Option<crate::fs::Vnode> {
    let mut files = OPEN_FILES.lock();
    let slot = files.get_mut(id)?;
    let f = slot.as_mut()?;
    f.refs -= 1;
    if f.refs > 0 {
        return None;
    }
    slot.take().map(|f| f.node)
}

/// `(node, pos, writable, append)` of a description.
fn open_file_get(id: usize) -> Option<(crate::fs::Vnode, usize, bool, bool)> {
    let files = OPEN_FILES.lock();
    let f = files.get(id)?.as_ref()?;
    Some((f.node.clone(), f.pos, f.writable, f.append))
}

fn open_file_node(id: usize) -> Option<crate::fs::Vnode> {
    open_file_get(id).map(|(node, ..)| node)
}

fn open_file_set_pos(id: usize, pos: usize) {
    if let Some(Some(f)) = OPEN_FILES.lock().get_mut(id) {
        f.pos = pos;
    }
}

fn open_file_advance(id: usize, n: usize) {
    if let Some(Some(f)) = OPEN_FILES.lock().get_mut(id) {
        f.pos += n;
    }
}

pub(super) fn default_user_fds() -> [FdEntry; MAX_FDS] {
    let mut fds = [FdEntry::Empty; MAX_FDS];
    fds[0] = FdEntry::Stdin;
    fds[1] = FdEntry::Console;
    fds[2] = FdEntry::Console;
    fds
}

pub(super) fn fd_clone(entry: FdEntry) -> FdEntry {
    match entry {
        FdEntry::PipeRead(id) => {
            pipe::add_reader(id);
            FdEntry::PipeRead(id)
        }
        FdEntry::PipeWrite(id) => {
            pipe::add_writer(id);
            FdEntry::PipeWrite(id)
        }
        FdEntry::PtyMaster(id) => {
            crate::pty::master_ref(id);
            FdEntry::PtyMaster(id)
        }
        FdEntry::PtySlave(id) => {
            crate::pty::slave_ref(id);
            FdEntry::PtySlave(id)
        }
        FdEntry::File(id) => {
            open_file_ref(id);
            entry
        }
        other => other,
    }
}

pub(super) fn fd_drop(entry: FdEntry) {
    match entry {
        FdEntry::PipeRead(id) => pipe::drop_reader(id),
        FdEntry::PipeWrite(id) => pipe::drop_writer(id),
        FdEntry::PtyMaster(id) => crate::pty::drop_master(id),
        FdEntry::PtySlave(id) => crate::pty::drop_slave(id),
        FdEntry::File(id) => {
            if let Some(node) = open_file_unref(id) {
                crate::fs::vfs::close_ref(&node);
            }
        }
        _ => {}
    }
}

/// Whether `[buf, buf + len)` is user memory of process `t` (the caller
/// holds `TASKS`, in `with_process_mut`).
///
/// Must not take `TASKS` itself: routing through `user::buffer_ok` re-locks
/// (`current_user_map` / `mmap_contains`) and deadlocks the same CPU —
/// hang after `/ok` prints `user ok`, on the first `read` of `/msg`.
fn user_buf_ok(buf: usize, len: usize, t: &Process) -> bool {
    if len == 0 {
        return true;
    }
    let end = match buf.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    let user_base = t.user_base as usize;
    let image_span = t.image_span;
    let brk = t.brk_cur as usize;
    let stack = t.stack_off as usize;
    let stack_bytes = crate::user::USER_STACK_PAGES * crate::user::PAGE;
    let in_code = buf >= user_base && end <= user_base + image_span;
    let in_stack = buf >= user_base + stack && end <= user_base + stack + stack_bytes;
    let heap_base = user_base + stack + stack_bytes;
    let in_heap = brk > heap_base && buf >= heap_base && end <= brk;
    if in_code || in_stack || in_heap {
        return true;
    }
    mmap_range_in(&t.mmap, buf, len)
}

/// An fd on `node`, opened with `flags`, granting `rights` beneath it (see
/// [`OpenFile::rights`]).
pub fn fd_open(node: crate::fs::Vnode, flags: u32, rights: crate::sec::Rights) -> Option<usize> {
    let writable = crate::fs::open_writable(flags);
    let append = crate::fs::open_append(flags);
    // A file one program holds at a time (`/dev/console/kbd`) may say no.
    if !crate::fs::vfs::open_hook(&node) {
        return None;
    }
    let Some(id) = open_file_alloc(node.clone(), writable, append, rights) else {
        crate::fs::vfs::open_hook_undo(&node);
        return None;
    };
    let fd = with_process_mut(|t| {
        for i in 0..MAX_FDS {
            if t.fds[i] == FdEntry::Empty {
                t.fds[i] = FdEntry::File(id);
                return Some(i);
            }
        }
        None
    });
    if fd.is_some() {
        crate::fs::vfs::open_ref(&node);
    } else {
        open_file_unref(id);
        crate::fs::vfs::open_hook_undo(&node);
    }
    fd
}

/// A pipe: its read and write ends, both closing at exec if `cloexec`.
pub fn pipe_open(cloexec: bool) -> Option<(usize, usize)> {
    let id = pipe::alloc()?;
    let out = with_process_mut(|t| {
        let mut read_fd = None;
        let mut write_fd = None;
        for i in 0..MAX_FDS {
            if t.fds[i] == FdEntry::Empty {
                if read_fd.is_none() {
                    read_fd = Some(i);
                } else {
                    write_fd = Some(i);
                    break;
                }
            }
        }
        let (Some(r), Some(w)) = (read_fd, write_fd) else {
            return None;
        };
        pipe::add_reader(id);
        pipe::add_writer(id);
        t.fds[r] = FdEntry::PipeRead(id);
        t.fds[w] = FdEntry::PipeWrite(id);
        if cloexec {
            t.cloexec |= 1 << r | 1 << w;
        }
        Some((r, w))
    });
    if out.is_none() {
        pipe::free(id);
    }
    out
}

/// Why [`fd_open_fifo`] failed.
pub enum FifoOpenErr {
    /// `O_WRONLY|O_NONBLOCK` with no reader (POSIX `ENXIO`).
    NoReader,
    /// No free fd, `O_RDWR`, FIFO gone, or the wait was interrupted.
    Failed,
}

const FIFO_O_ACCMODE: u32 = 3;
const FIFO_O_WRONLY: u32 = 1;
const FIFO_O_RDWR: u32 = 2;
/// Linux-shaped `O_NONBLOCK` (libgloss maps newlib's bit to this).
pub const O_NONBLOCK_K: u32 = 0o4000;

/// open(2) of a named FIFO backed by pipe slot `id`.
///
/// POSIX semantics: a read-only open blocks until a writer opens and a
/// write-only open blocks until a reader opens. With `O_NONBLOCK` the read
/// side returns at once and the write side fails with ENXIO when nobody reads.
/// `O_RDWR` is refused: an fd here is either a read or a write end.
pub fn fd_open_fifo(id: usize, flags: u32) -> Result<usize, FifoOpenErr> {
    let acc = flags & FIFO_O_ACCMODE;
    if acc == FIFO_O_RDWR {
        return Err(FifoOpenErr::Failed);
    }
    let write = acc == FIFO_O_WRONLY;
    let nonblock = flags & O_NONBLOCK_K != 0;
    let Some((readers, _, r_opens, w_opens)) = pipe::fifo_ends(id) else {
        return Err(FifoOpenErr::Failed);
    };
    if write && nonblock && readers == 0 {
        return Err(FifoOpenErr::NoReader);
    }
    if !pipe::fifo_attach(id, !write, write) {
        return Err(FifoOpenErr::Failed);
    }
    let entry = if write { FdEntry::PipeWrite(id) } else { FdEntry::PipeRead(id) };
    let fd = with_process_mut(|t| {
        let i = (0..MAX_FDS).find(|&i| t.fds[i] == FdEntry::Empty)?;
        t.fds[i] = entry;
        Some(i)
    });
    let Some(fd) = fd else {
        fd_drop(entry);
        return Err(FifoOpenErr::Failed);
    };
    if nonblock {
        return Ok(fd);
    }
    // Wait for the peer: either one is attached now, or one attached (and
    // perhaps already left) since we looked.
    loop {
        let seq = wait_seq();
        let Some((r, w, ro, wo)) = pipe::fifo_ends(id) else {
            break;
        };
        let peer_came = if write { r > 0 || ro != r_opens } else { w > 0 || wo != w_opens };
        if peer_came {
            return Ok(fd);
        }
        if crate::signal::interrupt_wait() {
            break;
        }
        block_until(key_pipe(id), seq, 0);
    }
    fd_close(fd);
    Err(FifoOpenErr::Failed)
}

/// Readiness bits for a userspace fd, for select()/poll() on pipes.
/// bit0 = readable (data or EOF), bit1 = writable, bit2 = hangup.
/// Non-pipe fds return 0; sockets are handled by netfs from userspace.
/// Poll readiness for pipes. `None` if `fd` is not a pipe end (so libgloss can
/// tell "empty pipe" apart from "not a pipe" when honouring O_NONBLOCK).
pub fn fd_poll_bits(fd: usize) -> Option<u32> {
    if fd >= MAX_FDS {
        return None;
    }
    with_process_mut(|t| match t.fds[fd] {
        FdEntry::PipeRead(id) => {
            let mut bits = 0u32;
            // Readable when data is buffered, or the writer closed (EOF).
            if !pipe::read_would_block(id) {
                bits |= 1;
            }
            if pipe::read_closed(id) {
                bits |= 1 | 4;
            }
            Some(bits)
        }
        FdEntry::PipeWrite(id) => {
            Some(if !pipe::write_would_block(id) { 2 } else { 0 })
        }
        _ => None,
    })
}

/// `poll(2)` bits (Linux values).
pub const POLLIN: u16 = 0x1;
pub const POLLOUT: u16 = 0x4;
pub const POLLERR: u16 = 0x8;
pub const POLLHUP: u16 = 0x10;
pub const POLLNVAL: u16 = 0x20;

/// Readiness of `fd` for `poll(2)`: the bits that hold now, whatever the
/// caller asked for, and whether `fd` is the console tty (whose keyboard
/// input is polled, not interrupt-driven: a waiter re-checks it).
pub fn fd_poll(fd: usize) -> (u16, bool) {
    let Some(entry) = with_process_mut(|t| t.fds.get(fd).copied()) else {
        return (POLLNVAL, false);
    };
    let tty = || {
        let readable = if crate::input::readable() { POLLIN } else { 0 };
        (readable | POLLOUT, true)
    };
    match entry {
        FdEntry::Empty => (POLLNVAL, false),
        FdEntry::Stdin | FdEntry::Console => tty(),
        FdEntry::File(_) if fd_is_console_tty(entry) => tty(),
        FdEntry::PipeRead(id) => {
            let mut bits = if pipe::read_would_block(id) { 0 } else { POLLIN };
            if pipe::read_closed(id) {
                // Writers gone: end of file is readable.
                bits |= POLLIN | POLLHUP;
            }
            (bits, false)
        }
        FdEntry::PipeWrite(id) => {
            if pipe::readers_gone(id) {
                (POLLERR, false)
            } else if pipe::write_would_block(id) {
                (0, false)
            } else {
                (POLLOUT, false)
            }
        }
        FdEntry::PtyMaster(id) | FdEntry::PtySlave(id) => {
            let master = matches!(entry, FdEntry::PtyMaster(_));
            let (readable, writable, hup) = crate::pty::poll_state(id, master);
            let mut bits = 0;
            if readable {
                bits |= POLLIN;
            }
            if writable {
                bits |= POLLOUT;
            }
            if hup {
                // The peer is gone: reads report it (EIO / end of file).
                bits |= POLLIN | POLLHUP;
            }
            (bits, false)
        }
        FdEntry::File(id) => {
            // A module file (a socket's /net data) knows its readiness; any
            // other file is always readable and writable.
            let bits = open_file_node(id)
                .and_then(|node| crate::fs::poll(&node))
                .unwrap_or((POLLIN | POLLOUT) as u32);
            // A polled device (the raw keyboard) is re-checked like the tty.
            let recheck = bits & myos_abi::MYOS_POLL_RECHECK != 0;
            (bits as u16, recheck)
        }
    }
}

/// Peer fd of a pipe end in the current task (read<->write), or None.
/// Legacy `PIPE_PEER` syscall (the old libgloss SIGCHLD self-pipe wake).
pub fn fd_pipe_peer(fd: usize) -> Option<usize> {
    if fd >= MAX_FDS {
        return None;
    }
    with_process_mut(|t| match t.fds[fd] {
        FdEntry::PipeRead(id) => (0..MAX_FDS).find(|&i| t.fds[i] == FdEntry::PipeWrite(id)),
        FdEntry::PipeWrite(id) => (0..MAX_FDS).find(|&i| t.fds[i] == FdEntry::PipeRead(id)),
        _ => None,
    })
}

/// Open `/dev/pts/clone`: allocate a pty pair, take the master fd.
pub fn fd_open_pty_master() -> Option<usize> {
    let id = crate::pty::alloc()?;
    let out = with_process_mut(|t| {
        for i in 0..MAX_FDS {
            if t.fds[i] == FdEntry::Empty {
                t.fds[i] = FdEntry::PtyMaster(id);
                return Some(i);
            }
        }
        None
    });
    if out.is_none() {
        crate::pty::drop_master(id);
    }
    out
}

/// Open `/dev/pts/N/data`: take a slave fd on an existing pair. Opening does NOT
/// claim the controlling-terminal session (Linux only binds a ctty via
/// `TIOCSCTTY`); `fd_open_pty_slave` deliberately skips `claim_session` so a
/// plain `openpty` can never scope SIGHUP/^C to a foreign process group.
pub fn fd_open_pty_slave(id: usize) -> Option<usize> {
    if !crate::pty::slave_exists(id) {
        return None;
    }
    let out = with_process_mut(|t| {
        for i in 0..MAX_FDS {
            if t.fds[i] == FdEntry::Empty {
                t.fds[i] = FdEntry::PtySlave(id);
                return Some(i);
            }
        }
        None
    });
    if out.is_some() {
        crate::pty::slave_ref(id);
    }
    out
}

pub fn fd_dup2(oldfd: usize, newfd: usize) -> bool {
    if oldfd >= MAX_FDS || newfd >= MAX_FDS {
        return false;
    }
    // Release the old entry outside TASKS: dropping a pipe/pty end wakes
    // its peers (and a pty hangup signals the session), which take TASKS.
    let (ok, dropped) = with_process_mut(|t| {
        let old = t.fds[oldfd];
        if old == FdEntry::Empty {
            return (false, FdEntry::Empty);
        }
        if oldfd == newfd {
            return (true, FdEntry::Empty);
        }
        let prev = t.fds[newfd];
        t.fds[newfd] = fd_clone(old);
        // The copy stays open across exec, whatever `oldfd` does.
        t.cloexec &= !(1 << newfd);
        (true, prev)
    });
    fd_drop(dropped);
    ok
}

/// First free fd >= `minfd` that clones `oldfd` (fcntl `F_DUPFD`; closing
/// at exec if `cloexec`, `F_DUPFD_CLOEXEC`).
pub fn fd_dup_min(oldfd: usize, minfd: usize, cloexec: bool) -> Option<usize> {
    if oldfd >= MAX_FDS || minfd >= MAX_FDS {
        return None;
    }
    with_process_mut(|t| {
        let old = t.fds[oldfd];
        if old == FdEntry::Empty {
            return None;
        }
        for i in minfd..MAX_FDS {
            if t.fds[i] == FdEntry::Empty {
                t.fds[i] = fd_clone(old);
                if cloexec {
                    t.cloexec |= 1 << i;
                }
                return Some(i);
            }
        }
        None
    })
}

/// File/chr write chunk (and pipe copy) size. DHCP ~300B was truncated at
/// 128, so TX chunks became separate Ethernet frames. Cap at netfs
/// MSG_CAP-REQ_HDR (2042): a 2048 chunk was rejected by net_write (payload >
/// MSG_CAP-6) → EIO on large SSH/TLS writes.
const FILE_IO_TMP: usize = 2042;

/// File/chr read size. A device that hands out messages (netfs's requests to
/// netd on `/dev/netd`, up to 2048 bytes) must get each whole into one read:
/// it was copied through `FILE_IO_TMP` and lost its tail, and netd dropped
/// the request.
const FILE_READ_TMP: usize = 4096;

/// Read/write on a pty end whose peer has hung up. `usize::MAX` stays the
/// generic SYSERR (EBADF in libgloss); this distinct value lets libgloss map
/// the hangup to `EIO` (Linux: read on a hung-up pty returns EIO).
pub const SYSERR_EIO: usize = usize::MAX - 1;

/// A read or write at an offset on an fd that has none (a pipe, a
/// terminal): `ESPIPE`.
pub const SYSERR_ESPIPE: usize = usize::MAX - 5;

/// Distinct syscall error for pty peer-gone (`EIO`): `usize::MAX` is the
/// generic SYSERR (libgloss maps it to EBADF), so read/write return `MAX-1`
/// on pty hangup and libgloss translates that to `errno = EIO` — the Linux
/// semantic `forkpty`/session consumers rely on.

/// Read from fd 0 (keyboard + serial stdin). `buf` must lie in the user map.
pub fn fd_read_stdin(buf: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    if !user::buffer_ok(buf, len) {
        return usize::MAX;
    }
    let mut tmp = [0u8; 128];
    let want = len.min(tmp.len());
    // Do not call input::read (may yield) while TASKS is locked — deadlock.
    let n = crate::input::read(&mut tmp[..want]);
    let aspace = current_aspace();
    if !user::copy_to_user(aspace, buf, &tmp[..n]) {
        return usize::MAX;
    }
    n
}

/// Read up to `len` bytes from `fd` into user `buf`: at the file position,
/// which advances, or (`at`) at that offset of a file, the position left as
/// it is.
pub fn fd_read(fd: usize, buf: usize, len: usize, at: Option<usize>) -> usize {
    if len == 0 {
        return 0;
    }
    loop {
        let (entry, ok) = with_process_mut(|t| {
            let entry = t.fds.get(fd).copied().unwrap_or(FdEntry::Empty);
            (entry, user_buf_ok(buf, len.min(FILE_READ_TMP), t))
        });
        if !ok {
            return usize::MAX;
        }
        if at.is_some() && !matches!(entry, FdEntry::File(_) | FdEntry::Empty) {
            return SYSERR_ESPIPE;
        }
        match entry {
            FdEntry::Stdin => return fd_read_stdin(buf, len),
            FdEntry::File(id) => {
                // Snapshot then read without holding a lock (devfs tty may yield).
                let Some((node, pos, ..)) = open_file_get(id) else {
                    return usize::MAX;
                };
                let pos = at.unwrap_or(pos);
                let mut tmp = [0u8; FILE_READ_TMP];
                let want = len.min(tmp.len());
                let seq = wait_seq();
                let Some(n) = crate::fs::vfs::read_or_wait(&node, pos, &mut tmp[..want]) else {
                    // A device with nothing yet (the raw keyboard): wait for
                    // it, re-checking at the keyboard's polling rate.
                    if crate::signal::interrupt_wait() {
                        return 0;
                    }
                    block_until(WAIT_ANY, seq, deadline_ms(10));
                    continue;
                };
                // Not under TASKS: the copy may page in the buffer.
                if n != 0 && !user::copy_to_user(current_aspace(), buf, &tmp[..n]) {
                    return usize::MAX;
                }
                if with_process_mut(|t| t.fds.get(fd).copied()) != Some(FdEntry::File(id)) {
                    return usize::MAX;
                }
                if at.is_none() {
                    open_file_advance(id, n);
                }
                return n;
            }
            FdEntry::PipeRead(id) => {
                let mut tmp = [0u8; FILE_IO_TMP];
                let want = len.min(tmp.len());
                let seq = wait_seq();
                let Some(n) = pipe::read(id, &mut tmp[..want]) else {
                    // A signal that terminates or is caught breaks the wait
                    // (Ctrl+C while a `cat`/`yes` pipe read is blocked).
                    if crate::signal::interrupt_wait() {
                        return 0;
                    }
                    block_until(key_pipe(id), seq, 0);
                    continue;
                };
                if n == usize::MAX {
                    return usize::MAX;
                }
                let aspace = current_aspace();
                if !user::copy_to_user(aspace, buf, &tmp[..n]) {
                    return usize::MAX;
                }
                return n;
            }
            FdEntry::PtyMaster(id) => {
                // pty::master_read blocks (yield loop) and returns EIO once
                // the last slave fd closed and output drained.
                let mut tmp = [0u8; 128];
                let want = len.min(tmp.len());
                let n = crate::pty::master_read(id, &mut tmp[..want]);
                if n == usize::MAX {
                    return SYSERR_EIO;
                }
                let aspace = current_aspace();
                if !user::copy_to_user(aspace, buf, &tmp[..n]) {
                    return usize::MAX;
                }
                return n;
            }
            FdEntry::PtySlave(id) => {
                let mut tmp = [0u8; 128];
                let want = len.min(tmp.len());
                let n = crate::pty::slave_read(id, &mut tmp[..want]);
                if n == usize::MAX {
                    return SYSERR_EIO;
                }
                let aspace = current_aspace();
                if !user::copy_to_user(aspace, buf, &tmp[..n]) {
                    return usize::MAX;
                }
                return n;
            }
            FdEntry::Empty | FdEntry::Console | FdEntry::PipeWrite(_) => return usize::MAX,
        }
    }
}

/// Write `len` bytes of user `buf` to `fd`: at the file position (the end
/// with `O_APPEND`), which advances, or (`at`) at that offset of a file,
/// the position left as it is.
pub fn fd_write(fd: usize, buf: usize, len: usize, at: Option<usize>) -> usize {
    if len == 0 {
        return 0;
    }
    let mut total = 0usize;
    while total < len {
        let chunk = (len - total).min(FILE_IO_TMP);
        let (entry, ok) = with_process_mut(|t| {
            let entry = t.fds.get(fd).copied().unwrap_or(FdEntry::Empty);
            (entry, user_buf_ok(buf + total, chunk, t))
        });
        if !ok {
            return if total == 0 { usize::MAX } else { total };
        }
        if at.is_some() && !matches!(entry, FdEntry::File(_) | FdEntry::Empty) {
            return SYSERR_ESPIPE;
        }
        let mut tmp = [0u8; FILE_IO_TMP];
        if !user::copy_from_user(current_aspace(), buf + total, &mut tmp[..chunk]) {
            return if total == 0 { usize::MAX } else { total };
        }
        // Bytes of this chunk a pipe took so far (a full ring takes part of it).
        let mut done = 0usize;
        loop {
            match entry {
                FdEntry::Console => {
                    print_bytes(&tmp[..chunk]);
                    total += chunk;
                    break;
                }
                FdEntry::File(id) => {
                    let Some((node, pos, writable, append)) = open_file_get(id) else {
                        return if total == 0 { usize::MAX } else { total };
                    };
                    if !writable {
                        return if total == 0 { usize::MAX } else { total };
                    }
                    let write_pos = match at {
                        Some(at) => at + total,
                        None if append => crate::fs::size_of(&node).unwrap_or(pos),
                        None => pos,
                    };
                    let Some(n) = crate::fs::write(&node, write_pos, &tmp[..chunk]) else {
                        return if total == 0 { usize::MAX } else { total };
                    };
                    // A poller (select/poll sleeping on "any event") may be
                    // waiting for this device/channel traffic.
                    wake_any();
                    if n == 0 {
                        return if total == 0 { usize::MAX } else { total };
                    }
                    if at.is_none() {
                        open_file_set_pos(id, write_pos + n);
                    }
                    total += n;
                    break;
                }
                FdEntry::PipeWrite(id) => {
                    let seq = wait_seq();
                    let n = pipe::write(id, &tmp[done..chunk]);
                    if n == usize::MAX {
                        return if total == 0 { usize::MAX } else { total };
                    }
                    if n == 0 && pipe::write_would_block(id) {
                        // See PipeRead wait. With nothing written yet this is
                        // EINTR; otherwise report the partial write.
                        if total == 0 && crate::signal::interrupt_wait() {
                            return usize::MAX;
                        }
                        if total != 0 && crate::signal::current_should_wake() {
                            return total;
                        }
                        block_until(key_pipe(id), seq, 0);
                        continue;
                    }
                    done += n;
                    total += n;
                    if done < chunk {
                        // Partial: the rest goes in the next loop round once
                        // a reader drained the ring.
                        continue;
                    }
                    break;
                }
                FdEntry::PtyMaster(id) => {
                    // Master write → slave input discipline; processes every
                    // byte (echo back into the output ring).
                    let n = crate::pty::master_write(id, &tmp[..chunk]);
                    if n == usize::MAX {
                        return if total == 0 { SYSERR_EIO } else { total };
                    }
                    total += n;
                    break;
                }
                FdEntry::PtySlave(id) => {
                    // Slave write → output processing → master-readable ring.
                    let n = crate::pty::slave_write(id, &tmp[..chunk]);
                    if n == usize::MAX {
                        return if total == 0 { SYSERR_EIO } else { total };
                    }
                    total += n;
                    break;
                }
                _ => return if total == 0 { usize::MAX } else { total },
            }
        }
    }
    total
}

/// Make the file `fd` is open on (for writing) `size` bytes long
/// (`ftruncate`): cut, or grown with zeros. The position stays.
pub fn fd_set_size(fd: usize, size: usize) -> bool {
    let Some(FdEntry::File(id)) = with_process_mut(|t| t.fds.get(fd).copied()) else {
        return false;
    };
    match open_file_get(id) {
        Some((node, _, true, _)) => crate::fs::set_size(&node, size),
        _ => false,
    }
}

pub fn fd_lseek(fd: usize, offset: i64, whence: usize) -> usize {
    const SEEK_SET: usize = 0;
    const SEEK_CUR: usize = 1;
    const SEEK_END: usize = 2;
    if fd >= MAX_FDS {
        return usize::MAX;
    }
    let FdEntry::File(id) = with_process_mut(|t| t.fds[fd]) else {
        return usize::MAX;
    };
    let Some((node, pos, ..)) = open_file_get(id) else {
        return usize::MAX;
    };
    let size = crate::fs::size_of(&node).unwrap_or(pos) as i64;
    let cur = pos as i64;
    let next = match whence {
        SEEK_SET => offset,
        SEEK_CUR => cur.saturating_add(offset),
        SEEK_END => size.saturating_add(offset),
        _ => return usize::MAX,
    };
    if next < 0 {
        return usize::MAX;
    }
    open_file_set_pos(id, next as usize);
    next as usize
}

/// What an open fd refers to (a personality module's `fstat`).
pub enum FdKind {
    Tty,
    Pipe,
    File { size: usize },
}

pub fn fd_kind(fd: usize) -> Option<FdKind> {
    let entry = with_process_mut(|t| t.fds.get(fd).copied())?;
    Some(match entry {
        FdEntry::Empty => return None,
        FdEntry::Stdin | FdEntry::Console | FdEntry::PtyMaster(_) | FdEntry::PtySlave(_) => {
            FdKind::Tty
        }
        FdEntry::PipeRead(_) | FdEntry::PipeWrite(_) => FdKind::Pipe,
        FdEntry::File(_) if fd_is_console_tty(entry) => FdKind::Tty,
        FdEntry::File(id) => FdKind::File {
            size: open_file_node(id).and_then(|node| crate::fs::size_of(&node)).unwrap_or(0),
        },
    })
}

/// What `fd` is open on, as `/proc/self/fd/N` names it: a path for a file
/// or a terminal (`/dev/console/data`, `/dev/pts/N/data`, the master end as
/// `/dev/pts/N/master`), `pipe:[N]` for a pipe end.
pub fn fd_path(fd: usize) -> Option<alloc::string::String> {
    use alloc::format;
    use alloc::string::String;
    let entry = with_process_mut(|t| t.fds.get(fd).copied())?;
    Some(match entry {
        FdEntry::Empty => return None,
        FdEntry::Stdin | FdEntry::Console => String::from("/dev/console/data"),
        FdEntry::File(id) => crate::fs::vfs::vnode_path(&open_file_node(id)?),
        FdEntry::PipeRead(id) | FdEntry::PipeWrite(id) => format!("pipe:[{id}]"),
        FdEntry::PtyMaster(id) => format!("/dev/pts/{id}/master"),
        FdEntry::PtySlave(id) => format!("/dev/pts/{id}/data"),
    })
}

/// What `fd` grants beneath it (a file description's `rights`).
pub fn fd_rights(fd: usize) -> Option<crate::sec::Rights> {
    match with_process_mut(|t| t.fds.get(fd).copied())? {
        FdEntry::File(id) => OPEN_FILES.lock().get(id)?.as_ref().map(|f| f.rights),
        _ => None,
    }
}

/// The file behind `fd` (for file-backed `mmap`), if it is a regular file.
pub fn fd_file_node(fd: usize) -> Option<crate::fs::Vnode> {
    match with_process_mut(|t| t.fds.get(fd).copied())? {
        FdEntry::File(id) => open_file_node(id),
        _ => None,
    }
}

pub fn fd_close(fd: usize) -> bool {
    if fd >= MAX_FDS {
        return false;
    }
    let entry = with_process_mut(|t| {
        let entry = t.fds[fd];
        // POSIX close semantics: the slot must become free so the next
        // open/pipe/socket reuses the lowest fd (os-test stdio/puts does
        // close(0); close(1); pipe() and expects the pipe on 0,1).
        t.fds[fd] = FdEntry::Empty;
        t.cloexec &= !(1 << fd);
        entry
    });
    if entry == FdEntry::Empty {
        return false;
    }
    // Outside TASKS: dropping a pipe/pty end wakes peers / signals a session.
    fd_drop(entry);
    true
}

/// Whether `fd` closes at exec (`F_GETFD`); `None` for a free fd.
pub fn fd_cloexec(fd: usize) -> Option<bool> {
    if fd >= MAX_FDS {
        return None;
    }
    with_process_mut(|t| (t.fds[fd] != FdEntry::Empty).then_some(t.cloexec & 1 << fd != 0))
}

/// Make `fd` close at exec or stay open (`F_SETFD`): false for a free fd.
pub fn fd_set_cloexec(fd: usize, on: bool) -> bool {
    if fd >= MAX_FDS {
        return false;
    }
    with_process_mut(|t| {
        if t.fds[fd] == FdEntry::Empty {
            return false;
        }
        if on {
            t.cloexec |= 1 << fd;
        } else {
            t.cloexec &= !(1 << fd);
        }
        true
    })
}

/// Close the fds marked close-on-exec: the new image of an exec never sees
/// them.
pub fn fd_close_on_exec() {
    let marked = with_process_mut(|t| t.cloexec);
    for fd in (0..MAX_FDS).filter(|fd| marked & 1 << fd != 0) {
        fd_close(fd);
    }
}

/// Whether the current task has a controlling terminal.

fn fd_is_console_tty(entry: FdEntry) -> bool {
    match entry {
        FdEntry::Stdin | FdEntry::Console => true,
        FdEntry::File(id) => {
            let Some(node) = open_file_node(id) else {
                return false;
            };
            crate::fs::vfs::node_at(&node, "/dev/tty") || crate::fs::vfs::node_at(&node, "/dev/console/data")
        }
        _ => false,
    }
}

/// The terminal `fd` is open on, for its control file: the console
/// (fds 0-2 and `/dev/console/data`) or a pty pair.
enum Terminal {
    Console,
    Pty(usize),
}

fn fd_terminal(fd: usize) -> Option<Terminal> {
    let entry = with_process_mut(|t| t.fds.get(fd).copied())?;
    match entry {
        FdEntry::Stdin | FdEntry::Console => Some(Terminal::Console),
        FdEntry::File(_) if fd_is_console_tty(entry) => Some(Terminal::Console),
        FdEntry::PtyMaster(id) | FdEntry::PtySlave(id) => Some(Terminal::Pty(id)),
        _ => None,
    }
}

/// The `ctl` text of the terminal `fd` is open on (docs/tty.md), for the
/// Linux layer's tty ioctls; `None` when the fd is not a terminal.
pub fn fd_tty_ctl_read(fd: usize) -> Option<alloc::vec::Vec<u8>> {
    match fd_terminal(fd)? {
        Terminal::Console => Some(crate::fs::console_ctl_text()),
        Terminal::Pty(id) => crate::pty::ctl_text(id),
    }
}

/// A write to the `ctl` of the terminal `fd` is open on: `None` when the fd
/// is not a terminal or the text is refused (nothing applied).
pub fn fd_tty_ctl_write(fd: usize, text: &[u8]) -> Option<usize> {
    match fd_terminal(fd)? {
        Terminal::Console => crate::fs::console_ctl_write(text),
        Terminal::Pty(id) => crate::pty::ctl_write(id, text),
    }
}
