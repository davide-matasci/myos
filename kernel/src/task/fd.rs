//! Per-task file descriptor table: open/dup/close, pipes, FIFOs, ptys,
//! read/write/lseek and the tty/termios ioctls.

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FdEntry {
    Empty,
    Stdin,
    Console,
    File {
        node: crate::fs::Vnode,
        pos: usize,
        writable: bool,
        append: bool,
    },
    PipeRead(usize),
    PipeWrite(usize),
    /// PTY master end (`/dev/ptmx`): writes feed slave input, reads drain
    /// slave output. Refcounted like pipes.
    PtyMaster(usize),
    /// PTY slave end (`/dev/pts/N`): the session-side tty.
    PtySlave(usize),
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
        FdEntry::File { node, .. } => {
            crate::fs::vfs::open_ref(&node);
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
        FdEntry::File { node, .. } => crate::fs::vfs::close_ref(&node),
        _ => {}
    }
}

/// Lock-free user-buffer check from a TASKS snapshot.
///
/// Must not take `TASKS`: `fd_read` of a file re-checks the dest buffer
/// inside `with_process_mut`. Routing through `user::buffer_ok` re-locks
/// (`current_user_map` / `mmap_contains`) and deadlocks the same CPU —
/// hang after `/ok` prints `user ok`, on the first `read` of `/msg`.
fn user_buf_ok(
    buf: usize,
    len: usize,
    user_base: usize,
    image_span: usize,
    stack_off: usize,
    brk: usize,
    mmap: &[MmapRegion],
) -> bool {
    if len == 0 {
        return true;
    }
    let end = match buf.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    let stack = stack_off;
    let stack_bytes = crate::user::USER_STACK_PAGES * crate::user::PAGE;
    let in_code = buf >= user_base && end <= user_base + image_span;
    let in_stack = buf >= user_base + stack && end <= user_base + stack + stack_bytes;
    let heap_base = user_base + stack + stack_bytes;
    let in_heap = brk > heap_base && buf >= heap_base && end <= brk;
    if in_code || in_stack || in_heap {
        return true;
    }
    mmap_range_in(mmap, buf, len)
}

pub fn fd_open(node: crate::fs::Vnode, flags: u32) -> Option<usize> {
    let writable = crate::fs::open_writable(flags);
    let append = crate::fs::open_append(flags);
    with_process_mut(|t| {
        for i in 0..MAX_FDS {
            if t.fds[i] == FdEntry::Empty {
                t.fds[i] = FdEntry::File {
                    node,
                    pos: 0,
                    writable,
                    append,
                };
                crate::fs::vfs::open_ref(&node);
                return Some(i);
            }
        }
        None
    })
}

pub fn pipe_open() -> Option<(usize, usize)> {
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

/// Open `/dev/ptmx`: allocate a pty pair, take the master fd.
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

/// Open `/dev/pts/N`: take a slave fd on an existing pair. Opening does NOT
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
        (true, prev)
    });
    fd_drop(dropped);
    ok
}

/// First free fd >= `minfd` that clones `oldfd` (fcntl F_DUPFD).
pub fn fd_dup_min(oldfd: usize, minfd: usize) -> Option<usize> {
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
                return Some(i);
            }
        }
        None
    })
}

/// File/chr copy size. DHCP ~300B was truncated at 128, so TX chunks became
/// separate Ethernet frames. Cap at netfs MSG_CAP-REQ_HDR (2042): a 2048
/// chunk was rejected by net_write (payload > MSG_CAP-6) → EIO on large
/// SSH/TLS writes.
const FILE_IO_TMP: usize = 2042;

/// Read/write on a pty end whose peer has hung up. `usize::MAX` stays the
/// generic SYSERR (EBADF in libgloss); this distinct value lets libgloss map
/// the hangup to `EIO` (Linux: read on a hung-up pty returns EIO).
pub const SYSERR_EIO: usize = usize::MAX - 1;

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

pub fn fd_read(fd: usize, buf: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    loop {
        // Copy out only what the check needs: a whole `Task` is ~10 KiB
        // (fd table + mmap table) and used to be copied twice per read.
        let (entry, map, mmap) = with_process_mut(|t| {
            (
                t.fds.get(fd).copied().unwrap_or(FdEntry::Empty),
                (
                    t.user_base,
                    t.image_span,
                    t.stack_off,
                    t.brk_cur as usize,
                ),
                t.mmap,
            )
        });
        let (user_base, image_span, stack_off, brk) = map;
        let user_base = user_base as usize;
        let stack_off = stack_off as usize;
        if !user_buf_ok(buf, len.min(FILE_IO_TMP), user_base, image_span, stack_off, brk, &mmap) {
            return usize::MAX;
        }
        match entry {
            FdEntry::Stdin => return fd_read_stdin(buf, len),
            FdEntry::File { node, pos, .. } => {
                // Snapshot then read without holding TASKS (devfs tty may yield).
                let mut tmp = [0u8; FILE_IO_TMP];
                let want = len.min(tmp.len());
                let n = crate::fs::read(&node, pos, &mut tmp[..want]);
                return with_process_mut(|t| {
                    let FdEntry::File {
                        pos: p,
                        ..
                    } = &mut t.fds[fd]
                    else {
                        return usize::MAX;
                    };
                    if n != 0 {
                        if !user_buf_ok(
                            buf,
                            n,
                            t.user_base as usize,
                            t.image_span,
                            t.stack_off as usize,
                            t.brk_cur as usize,
                            &t.mmap,
                        ) {
                            return usize::MAX;
                        }
                        if !user::copy_to_user(t.aspace, buf, &tmp[..n]) {
                            return usize::MAX;
                        }
                    }
                    *p += n;
                    n
                });
            }
            FdEntry::PipeRead(id) => {
                let mut tmp = [0u8; FILE_IO_TMP];
                let want = len.min(tmp.len());
                let seq = wait_seq();
                let n = pipe::read(id, &mut tmp[..want]);
                if n == usize::MAX {
                    return usize::MAX;
                }
                if n == 0 && pipe::read_would_block(id) {
                    // A signal that terminates or is caught breaks the wait
                    // (Ctrl+C while a `cat`/`yes` pipe read is blocked).
                    if crate::signal::interrupt_wait() {
                        return 0;
                    }
                    block_until(key_pipe(id), seq, 0);
                    continue;
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

pub fn fd_write(fd: usize, buf: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let mut total = 0usize;
    while total < len {
        let chunk = (len - total).min(FILE_IO_TMP);
        let (entry, map, mmap) = with_process_mut(|t| {
            (
                t.fds.get(fd).copied().unwrap_or(FdEntry::Empty),
                (
                    t.user_base,
                    t.image_span,
                    t.stack_off,
                    t.brk_cur as usize,
                ),
                t.mmap,
            )
        });
        let (user_base, image_span, stack_off, brk) = map;
        if !user_buf_ok(
            buf + total,
            chunk,
            user_base as usize,
            image_span,
            stack_off as usize,
            brk,
            &mmap,
        ) {
            return if total == 0 { usize::MAX } else { total };
        }
        let mut tmp = [0u8; FILE_IO_TMP];
        unsafe {
            core::ptr::copy_nonoverlapping((buf + total) as *const u8, tmp.as_mut_ptr(), chunk);
        }
        loop {
            match entry {
                FdEntry::Console => {
                    print_bytes(&tmp[..chunk]);
                    total += chunk;
                    break;
                }
                FdEntry::File {
                    node,
                    pos,
                    writable,
                    append,
                } => {
                    if !writable {
                        return if total == 0 { usize::MAX } else { total };
                    }
                    let write_pos = if append {
                        crate::fs::size_of(&node).unwrap_or(pos)
                    } else {
                        pos
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
                    with_process_mut(|t| {
                        if let FdEntry::File { pos: p, append: ap, .. } = &mut t.fds[fd] {
                            if *ap {
                                *p = write_pos + n;
                            } else {
                                *p = write_pos + n;
                            }
                        }
                    });
                    total += n;
                    break;
                }
                FdEntry::PipeWrite(id) => {
                    let seq = wait_seq();
                    let n = pipe::write(id, &tmp[..chunk]);
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
                    if n < chunk {
                        // Partial: the rest goes in the next loop round once
                        // a reader drained the ring.
                        total += n;
                        continue;
                    }
                    total += n;
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

pub fn fd_lseek(fd: usize, offset: i64, whence: usize) -> usize {
    const SEEK_SET: usize = 0;
    const SEEK_CUR: usize = 1;
    const SEEK_END: usize = 2;
    with_process_mut(|t| {
        if fd >= MAX_FDS {
            return usize::MAX;
        }
        match t.fds[fd] {
            FdEntry::File { node, pos, .. } => {
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
                if let FdEntry::File { pos: p, .. } = &mut t.fds[fd] {
                    *p = next as usize;
                }
                next as usize
            }
            _ => usize::MAX,
        }
    })
}

/// What an open fd refers to (for the Linux layer's `fstat`).
#[cfg(feature = "linux-compat")]
pub enum FdKind {
    Tty,
    Pipe,
    File { size: usize },
}

#[cfg(feature = "linux-compat")]
pub fn fd_kind(fd: usize) -> Option<FdKind> {
    let entry = with_process_mut(|t| t.fds.get(fd).copied())?;
    Some(match entry {
        FdEntry::Empty => return None,
        FdEntry::Stdin | FdEntry::Console | FdEntry::PtyMaster(_) | FdEntry::PtySlave(_) => {
            FdKind::Tty
        }
        FdEntry::PipeRead(_) | FdEntry::PipeWrite(_) => FdKind::Pipe,
        FdEntry::File { .. } if fd_is_console_tty(entry) => FdKind::Tty,
        FdEntry::File { node, .. } => FdKind::File {
            size: crate::fs::size_of(&node).unwrap_or(0),
        },
    })
}

/// The file behind `fd` (for file-backed `mmap`), if it is a regular file.
pub fn fd_file_node(fd: usize) -> Option<crate::fs::Vnode> {
    match with_process_mut(|t| t.fds.get(fd).copied())? {
        FdEntry::File { node, .. } => Some(node),
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
        entry
    });
    if entry == FdEntry::Empty {
        return false;
    }
    // Outside TASKS: dropping a pipe/pty end wakes peers / signals a session.
    fd_drop(entry);
    true
}

/// Whether the current task has a controlling terminal.

fn fd_is_console_tty(entry: FdEntry) -> bool {
    match entry {
        FdEntry::Stdin | FdEntry::Console => true,
        FdEntry::File { node, .. } => {
            let p = node.path_str();
            p == "tty" || p == "console"
        }
        _ => false,
    }
}

/// Generic ioctl dispatch. Tty/console keep Linux getty semantics; other
/// open File vnodes go through [`crate::fs::ioctl`] (devfs → chrdevs like net0).
pub fn fd_ioctl(fd: usize, request: usize, arg: usize) -> usize {
    use crate::fs::IoctlResult;

    const TIOCSCTTY: usize = 0x540E;

    let entry = with_process_mut(|t| t.fds.get(fd).copied().unwrap_or(FdEntry::Empty));

    // PTY fd ioctls: per-pair termios (shared across both ends, Linux model),
    // winsize propagation, TIOCGPTN/TIOCSPTLCK on the master, TIOCSCTTY on the
    // slave (session-leader claim). Handled here because they copy user data.
    const TIOCGPTN: usize = 0x8004_5430;
    const TIOCSPTLCK: usize = 0x4004_5431;
    const TIOCGWINSZ: usize = 0x5413;
    const TIOCSWINSZ: usize = 0x5414;
    const TCGETS: usize = 0x5401;
    const TCSETS: usize = 0x5402;
    match (request, entry) {
        (TIOCGPTN, FdEntry::PtyMaster(id)) => {
            if arg == 0 {
                return usize::MAX;
            }
            let Some(id) = crate::pty::index(id) else {
                return usize::MAX;
            };
            if !user::copy_to_user(current_aspace(), arg, &id.to_ne_bytes()) {
                return usize::MAX;
            }
            return 0;
        }
        (TIOCSPTLCK, FdEntry::PtyMaster(_)) => return 0,
        (TIOCSCTTY, FdEntry::PtySlave(id)) => {
            // Session leader claims (or re-affirms) this pty as its ctty.
            crate::pty::claim_session(id);
            return 0;
        }
        (TIOCGWINSZ, FdEntry::PtyMaster(id) | FdEntry::PtySlave(id)) => {
            let Some((row, col)) = crate::pty::winsize(id) else {
                return usize::MAX;
            };
            if arg == 0 {
                return usize::MAX;
            }
            let mut buf = [0u8; 8]; // {row: u16, col: u16, xpixel: u16, ypixel: u16}
            buf[0..2].copy_from_slice(&row.to_ne_bytes());
            buf[2..4].copy_from_slice(&col.to_ne_bytes());
            if !user::copy_to_user(current_aspace(), arg, &buf) {
                return usize::MAX;
            }
            return 0;
        }
        (TIOCSWINSZ, FdEntry::PtyMaster(id) | FdEntry::PtySlave(id)) => {
            if arg == 0 {
                return usize::MAX;
            }
            let aspace = current_aspace();
            let mut ws = [0u8; 8]; // {row: u16, col: u16, xpixel: u16, ypixel: u16}
            if !user::copy_from_user(aspace, arg, &mut ws) {
                return usize::MAX;
            }
            let row = u16::from_ne_bytes([ws[0], ws[1]]);
            let col = u16::from_ne_bytes([ws[2], ws[3]]);
            crate::pty::set_winsize(id, row, col);
            return 0;
        }
        (req @ (TCGETS | TCSETS), FdEntry::PtyMaster(id) | FdEntry::PtySlave(id)) => {
            if arg == 0 {
                return usize::MAX;
            }
            let aspace = current_aspace();
            if req == TCGETS {
                let Some(buf) = crate::pty::termios_get_bytes(id) else {
                    return usize::MAX;
                };
                if !user::copy_to_user(aspace, arg, &buf) {
                    return usize::MAX;
                }
            } else {
                let mut buf = [0u8; crate::tty::TERMIOS_LEN];
                if !user::copy_from_user(aspace, arg, &mut buf) {
                    return usize::MAX;
                }
                crate::pty::termios_set_bytes(id, &buf);
            }
            return 0;
        }
        _ => {}
    }

    // Real TIOCSCTTY: attach the system console as the caller's ctty.
    // Getty passes a non-null arg (force); phase-1 accepts either.
    if request == TIOCSCTTY {
        if !fd_is_console_tty(entry) {
            return usize::MAX;
        }
        set_ctty();
        return 0;
    }

    // TCGETS / TCSETS: maintain per-console termios (raw vs cooked for vim).
    if request == TCGETS || request == TCSETS {
        if !fd_is_console_tty(entry) {
            return usize::MAX;
        }
        if arg == 0 {
            return usize::MAX;
        }
        let aspace = current_aspace();
        if request == TCGETS {
            let buf = crate::input::termios_get_bytes();
            if !user::copy_to_user(aspace, arg, &buf) {
                return usize::MAX;
            }
        } else {
            let mut buf = [0u8; crate::input::TERMIOS_LEN];
            if !user::copy_from_user(aspace, arg, &mut buf) {
                return usize::MAX;
            }
            crate::input::termios_set_bytes(&buf);
        }
        return 0;
    }

    // KDSKMAP / KDGKMAP: loadable keyboard map (see crate::keymap, docs/keymap.md).
    if request == crate::keymap::KDSKMAP || request == crate::keymap::KDGKMAP {
        if !fd_is_console_tty(entry) {
            return usize::MAX;
        }
        if arg == 0 {
            return usize::MAX;
        }
        let aspace = current_aspace();
        if request == crate::keymap::KDGKMAP {
            let v: u32 = if crate::keymap::is_loaded() { 1 } else { 0 };
            if !user::copy_to_user(aspace, arg, &v.to_ne_bytes()) {
                return usize::MAX;
            }
            return 0;
        }
        // KDSKMAP: arg → { len: u32, data: [u8; len] } (len little-endian, max 8 KiB).
        let mut len_buf = [0u8; 4];
        if !user::copy_from_user(aspace, arg, &mut len_buf) {
            return usize::MAX;
        }
        let len = u32::from_ne_bytes(len_buf) as usize;
        if len == 0 || len > 8192 {
            return usize::MAX;
        }
        let mut data = alloc::vec![0u8; len];
        if !user::copy_from_user(aspace, arg + 4, &mut data) {
            return usize::MAX;
        }
        match crate::keymap::load_from_text(&data) {
            Ok(()) => return 0,
            Err(e) => {
                crate::console::status_fail(&alloc::format!("keymap: {e}"));
                return usize::MAX;
            }
        }
    }

    let result = match entry {
        FdEntry::Empty
        | FdEntry::PipeRead(_)
        | FdEntry::PipeWrite(_)
        // pty-pair ioctls (TCGETS/TCSETS/winsize/TIOCSCTTY/TIOCGPTN) are all
        // handled above with userspace copies; nothing falls through here.
        | FdEntry::PtyMaster(_)
        | FdEntry::PtySlave(_) => IoctlResult::Notty,
        FdEntry::Stdin | FdEntry::Console => crate::fs::tty_ioctl(request),
        FdEntry::File { node, .. } => crate::fs::ioctl(&node, request, arg),
    };

    match result {
        IoctlResult::Ok => 0,
        IoctlResult::Winsize { row, col } => {
            if arg == 0 {
                return usize::MAX;
            }
            let mut buf = [0u8; 8];
            buf[0..2].copy_from_slice(&row.to_ne_bytes());
            buf[2..4].copy_from_slice(&col.to_ne_bytes());
            let aspace = current_aspace();
            if !user::copy_to_user(aspace, arg, &buf) {
                return usize::MAX;
            }
            0
        }
        IoctlResult::Notty | IoctlResult::Bad => usize::MAX,
    }
}
