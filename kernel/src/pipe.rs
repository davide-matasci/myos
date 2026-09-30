//! In-kernel pipe ring buffers for `pipe(2)`.

use spin::Mutex;

// Named FIFOs (tmpfs `mkfifo`) hold a slot for as long as the node exists.
const MAX_PIPES: usize = 32;
const PIPE_BUF: usize = 512;

struct Pipe {
    data: [u8; PIPE_BUF],
    head: usize,
    len: usize,
    readers: u8,
    writers: u8,
    write_closed: bool,
    /// Backs a named FIFO: the slot outlives its open ends and is only
    /// released by [`fifo_unlink`] once nothing has it open.
    named: bool,
    /// Opens ever attached to this FIFO as reader / writer. A blocked
    /// `open()` waits for the other side's count to move, so a peer that
    /// opened and closed again before we were scheduled still releases it.
    read_opens: u32,
    write_opens: u32,
}

static PIPES: Mutex<[Option<Pipe>; MAX_PIPES]> = Mutex::new([const { None }; MAX_PIPES]);

pub fn free(id: usize) {
    let mut pipes = PIPES.lock();
    if let Some(slot) = pipes.get_mut(id) {
        *slot = None;
    }
}

pub fn alloc() -> Option<usize> {
    let mut pipes = PIPES.lock();
    for (i, slot) in pipes.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(Pipe {
                data: [0; PIPE_BUF],
                head: 0,
                len: 0,
                readers: 0,
                writers: 0,
                write_closed: false,
                named: false,
                read_opens: 0,
                write_opens: 0,
            });
            return Some(i);
        }
    }
    None
}

pub fn add_reader(id: usize) -> bool {
    let mut pipes = PIPES.lock();
    let Some(p) = pipes.get_mut(id).and_then(|s| s.as_mut()) else {
        return false;
    };
    p.readers = p.readers.saturating_add(1);
    true
}

pub fn add_writer(id: usize) -> bool {
    let mut pipes = PIPES.lock();
    let Some(p) = pipes.get_mut(id).and_then(|s| s.as_mut()) else {
        return false;
    };
    p.writers = p.writers.saturating_add(1);
    true
}

pub fn drop_reader(id: usize) {
    let mut pipes = PIPES.lock();
    let Some(p) = pipes.get_mut(id).and_then(|s| s.as_mut()) else {
        return;
    };
    p.readers = p.readers.saturating_sub(1);
    if p.readers == 0 {
        p.write_closed = true;
    }
    release_if_idle(&mut pipes, id);
}

pub fn drop_writer(id: usize) {
    let mut pipes = PIPES.lock();
    let Some(p) = pipes.get_mut(id).and_then(|s| s.as_mut()) else {
        return;
    };
    p.writers = p.writers.saturating_sub(1);
    if p.writers == 0 {
        p.write_closed = true;
    }
    release_if_idle(&mut pipes, id);
}

/// Last end closed: free an anonymous pipe; a named FIFO keeps its slot but
/// drops unread data (POSIX discards FIFO contents on last close).
fn release_if_idle(pipes: &mut [Option<Pipe>; MAX_PIPES], id: usize) {
    let Some(p) = pipes.get_mut(id).and_then(|s| s.as_mut()) else {
        return;
    };
    if p.readers != 0 || p.writers != 0 {
        return;
    }
    if p.named {
        p.head = 0;
        p.len = 0;
        p.write_closed = false;
    } else {
        pipes[id] = None;
    }
}

/// Allocate the pipe behind a new named FIFO.
pub fn alloc_named() -> Option<usize> {
    let id = alloc()?;
    if let Some(p) = PIPES.lock()[id].as_mut() {
        p.named = true;
    }
    Some(id)
}

/// The FIFO node was unlinked: free its pipe now, or when the last open end
/// closes.
pub fn fifo_unlink(id: usize) {
    let mut pipes = PIPES.lock();
    let Some(p) = pipes.get_mut(id).and_then(|s| s.as_mut()) else {
        return;
    };
    p.named = false;
    if p.readers == 0 && p.writers == 0 {
        pipes[id] = None;
    }
}

/// Open state of a named FIFO: `(readers, writers, read_opens, write_opens)`,
/// or `None` if the pipe is gone.
pub fn fifo_ends(id: usize) -> Option<(u8, u8, u32, u32)> {
    let pipes = PIPES.lock();
    pipes
        .get(id)
        .and_then(|s| s.as_ref())
        .map(|p| (p.readers, p.writers, p.read_opens, p.write_opens))
}

/// Attach one open of a named FIFO. Readers see EOF only once every writer
/// is gone, so (re)opening a writer clears the "closed" state.
pub fn fifo_attach(id: usize, read: bool, write: bool) -> bool {
    let mut pipes = PIPES.lock();
    let Some(p) = pipes.get_mut(id).and_then(|s| s.as_mut()) else {
        return false;
    };
    if read {
        p.readers = p.readers.saturating_add(1);
        p.read_opens = p.read_opens.wrapping_add(1);
    }
    if write {
        p.writers = p.writers.saturating_add(1);
        p.write_opens = p.write_opens.wrapping_add(1);
    }
    p.write_closed = p.writers == 0;
    true
}

pub fn read(id: usize, out: &mut [u8]) -> usize {
    let mut pipes = PIPES.lock();
    let Some(p) = pipes.get_mut(id).and_then(|s| s.as_mut()) else {
        return usize::MAX;
    };
    if p.len == 0 {
        return if p.write_closed { 0 } else { 0 };
    }
    let n = out.len().min(p.len);
    for i in 0..n {
        out[i] = p.data[(p.head + i) % PIPE_BUF];
    }
    p.head = (p.head + n) % PIPE_BUF;
    p.len -= n;
    n
}

pub fn write(id: usize, data: &[u8]) -> usize {
    let mut pipes = PIPES.lock();
    let Some(p) = pipes.get_mut(id).and_then(|s| s.as_mut()) else {
        return usize::MAX;
    };
    if p.readers == 0 {
        return usize::MAX;
    }
    let mut n = 0usize;
    for &b in data {
        if p.len >= PIPE_BUF {
            break;
        }
        let tail = (p.head + p.len) % PIPE_BUF;
        p.data[tail] = b;
        p.len += 1;
        n += 1;
    }
    n
}

pub fn read_closed(id: usize) -> bool {
    let pipes = PIPES.lock();
    pipes
        .get(id)
        .and_then(|s| s.as_ref())
        .is_some_and(|p| p.write_closed)
}

pub fn write_would_block(id: usize) -> bool {
    let pipes = PIPES.lock();
    pipes
        .get(id)
        .and_then(|s| s.as_ref())
        .is_some_and(|p| p.len >= PIPE_BUF && !p.write_closed)
}

pub fn read_would_block(id: usize) -> bool {
    let pipes = PIPES.lock();
    pipes
        .get(id)
        .and_then(|s| s.as_ref())
        .is_some_and(|p| p.len == 0 && !p.write_closed)
}
