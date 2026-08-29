//! In-kernel pipe ring buffers for `pipe(2)`.

use spin::Mutex;

const MAX_PIPES: usize = 8;
const PIPE_BUF: usize = 512;

struct Pipe {
    data: [u8; PIPE_BUF],
    head: usize,
    len: usize,
    readers: u8,
    writers: u8,
    write_closed: bool,
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
    if p.readers == 0 && p.writers == 0 {
        *pipes.get_mut(id).unwrap() = None;
    }
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
    if p.readers == 0 && p.writers == 0 {
        *pipes.get_mut(id).unwrap() = None;
    }
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
