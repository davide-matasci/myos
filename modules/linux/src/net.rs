//! `AF_INET` sockets over the native `/net` (see `docs/sockets-curl.md`).
//!
//! A socket is a `/net/{tcp,udp}/N` conversation and its fd is the
//! conversation's `data` file, so reads, writes, dup, fork and close are
//! ordinary fd operations, and the last close hangs the conversation up.
//! `connect` is a `ctl` write; readiness comes from `status` and the size of
//! `data` (the bytes netd delivered and nobody read yet). The `/net` files
//! never block, so waits sleep until netd's next reply wakes the pollers.

use alloc::format;
use alloc::string::String;

use super::abi::*;
use super::files;
use super::sys::{get_bytes, native, put, R};
use crate::k::{fs, signal, task, time, user};

const AF_INET: usize = 2;
const SOCK_STREAM: usize = 1;
const SOCK_DGRAM: usize = 2;
const SOCK_NONBLOCK: usize = 0o4000;
const SOCK_CLOEXEC: usize = 0o2000000;
const MSG_DONTWAIT: usize = 0x40;
const SHUT_RDWR: usize = 2;
const SOL_SOCKET: usize = 1;
const SO_TYPE: usize = 3;
const SO_ERROR: usize = 4;

const ENOTSOCK: usize = 88;
const EPROTOTYPE: usize = 91;
const EOPNOTSUPP: usize = 95;
const EAFNOSUPPORT: usize = 97;
const ECONNRESET: usize = 104;
const ENOTCONN: usize = 107;
const ECONNREFUSED: usize = 111;
const EINPROGRESS: usize = 115;
const EPIPE: usize = 32;

/// How long a blocking `connect` waits for the handshake.
const CONNECT_TIMEOUT_NS: u64 = 60_000_000_000;
/// A blocking write waiting for room in netd's request ring re-tries this
/// often (draining it wakes nobody).
const RETRY_NS: u64 = 10_000_000;

/// An IPv4 address and port.
pub type Addr = ([u8; 4], u16);

/// A socket's state beyond its fd (kept in the fd table, `files`).
#[derive(Clone, Copy)]
pub struct Sock {
    pub stream: bool,
    pub nonblock: bool,
    pub peer: Option<Addr>,
}

/// A conversation's state, from its `status` file.
#[derive(PartialEq, Eq)]
enum State {
    /// Not connected yet (or a TCP handshake in flight).
    Pending,
    Connected,
    /// The peer (or we) hung up; buffered data can still be read.
    HungUp,
    /// The connect failed or netd reported an error.
    Failed,
}

fn state(conv: &str) -> State {
    let mut b = [0u8; 64];
    let n = fs::read(&format!("{conv}/status"), 0, &mut b).unwrap_or(0);
    match &b[..n] {
        // Empty until netd acknowledges the `clone`.
        b"" | b"cloned" | b"connecting" => State::Pending,
        b"connected" => State::Connected,
        b"hangup" => State::HungUp,
        _ => State::Failed,
    }
}

/// Bytes waiting in the conversation's `data`.
fn pending(conv: &str) -> bool {
    fs::stat(&format!("{conv}/data")).is_some_and(|st| st.size > 0)
}

fn sock(fd: usize) -> Result<(String, Sock), usize> {
    match files::get(fd) {
        Some(e) => e.sock.map(|s| (e.path, s)).ok_or(ENOTSOCK),
        None if task::fd_kind(fd).is_some() => Err(ENOTSOCK),
        None => Err(EBADF),
    }
}

/// Sleep until `ready` holds, re-checked whenever something happens (netd's
/// replies wake pollers), until a signal or the monotonic `deadline` (0 =
/// none).
fn wait(mut ready: impl FnMut() -> bool, deadline: u64) -> Result<(), usize> {
    loop {
        let seq = task::wait_seq();
        if ready() {
            return Ok(());
        }
        if signal::interrupt_wait() {
            return Err(EINTR);
        }
        if deadline != 0 && time::monotonic_ns() >= deadline {
            return Err(ETIMEDOUT);
        }
        task::block_until(task::WAIT_ANY, seq, deadline);
    }
}

fn get_addr(ptr: usize, len: usize) -> Result<Addr, usize> {
    let mut b = [0u8; 8];
    if len < 8 {
        return Err(EINVAL);
    }
    get_bytes(ptr, &mut b)?;
    if u16::from_le_bytes([b[0], b[1]]) as usize != AF_INET {
        return Err(EAFNOSUPPORT);
    }
    Ok(([b[4], b[5], b[6], b[7]], u16::from_be_bytes([b[2], b[3]])))
}

/// Store a `sockaddr_in` at `ptr` (`*len_ptr` bytes of room) and its size
/// at `len_ptr`.
fn put_addr(ptr: usize, len_ptr: usize, (ip, port): Addr) -> R {
    if ptr == 0 {
        return Ok(0);
    }
    let mut l = [0u8; 4];
    get_bytes(len_ptr, &mut l)?;
    let room = u32::from_le_bytes(l) as usize;
    let mut b = [0u8; 16];
    b[..2].copy_from_slice(&(AF_INET as u16).to_le_bytes());
    b[2..4].copy_from_slice(&port.to_be_bytes());
    b[4..8].copy_from_slice(&ip);
    put(ptr, &b[..room.min(16)])?;
    put(len_ptr, &16u32.to_le_bytes())?;
    Ok(0)
}

pub fn socket(domain: usize, ty: usize) -> R {
    if domain != AF_INET {
        return Err(EAFNOSUPPORT);
    }
    let stream = match ty & !(SOCK_NONBLOCK | SOCK_CLOEXEC) {
        SOCK_STREAM => true,
        SOCK_DGRAM => false,
        _ => return Err(EPROTOTYPE),
    };
    let proto = if stream { "tcp" } else { "udp" };
    // `/net` is the system's, also inside a `linux --root` chroot (a bind):
    // the conversation is kept by its VFS path.
    let dir = user::resolve_copied_path(&format!("/net/{proto}")).ok_or(EAFNOSUPPORT)?;
    // Reading `clone` allocates a conversation and names it.
    let mut b = [0u8; 8];
    let n = fs::read(&format!("{dir}/clone"), 0, &mut b).unwrap_or(0);
    let id = core::str::from_utf8(&b[..n]).ok().map(str::trim).filter(|s| !s.is_empty()).ok_or(EMFILE)?;
    let conv = format!("{dir}/{id}");
    let fd = user::open_path(&format!("/net/{proto}/{id}/data"), 2);
    if fd >= signal::SYSERR_EINTR {
        let _ = fs::write(&format!("{conv}/ctl"), 0, b"hangup");
        return Err(EMFILE);
    }
    files::set_sock(fd, conv, Sock { stream, nonblock: ty & SOCK_NONBLOCK != 0, peer: None });
    Ok(fd)
}

pub fn connect(fd: usize, addr: usize, len: usize) -> R {
    let (conv, s) = sock(fd)?;
    let peer = get_addr(addr, len)?;
    ctl_connect(&conv, peer)?;
    files::with_sock(fd, |s| s.peer = Some(peer));
    if !s.stream {
        return Ok(0);
    }
    if s.nonblock {
        return Err(EINPROGRESS);
    }
    wait(|| state(&conv) != State::Pending, time::monotonic_ns() + CONNECT_TIMEOUT_NS)?;
    match state(&conv) {
        State::Connected => Ok(0),
        _ => Err(ECONNREFUSED),
    }
}

/// Read a socket with `io` (a native read) once there is something to read
/// or the stream ended; a non-blocking socket (or `dontwait`) does not wait.
pub fn recv(fd: usize, dontwait: bool, io: impl FnOnce() -> R) -> R {
    let (conv, s) = sock(fd)?;
    let ready = || pending(&conv) || matches!(state(&conv), State::HungUp | State::Failed);
    if s.nonblock || dontwait {
        if !ready() {
            return Err(EAGAIN);
        }
    } else {
        wait(ready, 0)?;
    }
    io()
}

/// Write a connected socket with `io` (a native write). netd's request ring
/// may be full: a blocking socket retries as netd drains it.
pub fn send(fd: usize, mut io: impl FnMut() -> R) -> R {
    loop {
        let (conv, s) = sock(fd)?;
        match state(&conv) {
            State::Connected => {}
            State::Pending if !s.stream && s.peer.is_some() => {}
            State::Pending => return Err(ENOTCONN),
            State::HungUp => return Err(EPIPE),
            State::Failed => return Err(ECONNRESET),
        }
        let seq = task::wait_seq();
        match io() {
            Ok(n) => return Ok(n),
            Err(_) if s.nonblock => return Err(EAGAIN),
            Err(_) if signal::interrupt_wait() => return Err(EINTR),
            Err(_) => task::block_until(task::WAIT_ANY, seq, time::monotonic_ns() + RETRY_NS),
        }
    }
}

/// A datagram socket sends to `to`: connect it there first.
fn send_to(fd: usize, to: usize, len: usize) -> Result<(), usize> {
    let (conv, s) = sock(fd)?;
    if to == 0 || s.stream {
        return Ok(());
    }
    let peer = get_addr(to, len)?;
    if s.peer != Some(peer) {
        ctl_connect(&conv, peer)?;
        files::with_sock(fd, |s| s.peer = Some(peer));
    }
    Ok(())
}

fn ctl_connect(conv: &str, ([a, b, c, d], port): Addr) -> Result<(), usize> {
    let cmd = format!("connect {a}.{b}.{c}.{d}!{port}");
    fs::write(&format!("{conv}/ctl"), 0, cmd.as_bytes()).map(|_| ()).ok_or(EIO)
}

pub fn sendto(fd: usize, buf: usize, len: usize, to: usize, tolen: usize) -> R {
    send_to(fd, to, tolen)?;
    super::sys::write(fd, buf, len)
}

pub fn recvfrom(fd: usize, buf: usize, len: usize, flags: usize, from: usize, fromlen: usize) -> R {
    let n = recv(fd, flags & MSG_DONTWAIT != 0, || native(user::sys_read(fd, buf, len), EBADF))?;
    if let Some(peer) = sock(fd)?.1.peer {
        put_addr(from, fromlen, peer)?;
    }
    Ok(n)
}

/// `struct msghdr`: name, namelen, iov, iovlen (the control data is unused).
fn msghdr(msg: usize) -> Result<[usize; 4], usize> {
    let mut b = [0u8; 32];
    get_bytes(msg, &mut b)?;
    let w = |i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap()) as usize;
    Ok([w(0), w(8) & 0xffff_ffff, w(16), w(24)])
}

pub fn sendmsg(fd: usize, msg: usize) -> R {
    let [name, namelen, iov, iovlen] = msghdr(msg)?;
    send_to(fd, name, namelen)?;
    super::sys::rw_vec(fd, iov, iovlen, true)
}

pub fn recvmsg(fd: usize, msg: usize, flags: usize) -> R {
    let [name, _, iov, iovlen] = msghdr(msg)?;
    let n = recv(fd, flags & MSG_DONTWAIT != 0, || super::sys::rw_vec(fd, iov, iovlen, false))?;
    if let Some(peer) = sock(fd)?.1.peer {
        put_addr(name, msg + 8, peer)?;
    }
    // No control data, no flags.
    put(msg + 40, &0u64.to_le_bytes())?;
    put(msg + 48, &0u32.to_le_bytes())?;
    Ok(n)
}

/// Only a full shutdown hangs up: `/net` has no half-close.
pub fn shutdown(fd: usize, how: usize) -> R {
    let (conv, _) = sock(fd)?;
    if how == SHUT_RDWR {
        let _ = fs::write(&format!("{conv}/ctl"), 0, b"hangup");
    }
    Ok(0)
}

/// The local address is not known: `0.0.0.0:0`.
pub fn getsockname(fd: usize, addr: usize, alen: usize) -> R {
    sock(fd)?;
    put_addr(addr, alen, ([0; 4], 0))
}

pub fn getpeername(fd: usize, addr: usize, alen: usize) -> R {
    let peer = sock(fd)?.1.peer.ok_or(ENOTCONN)?;
    put_addr(addr, alen, peer)
}

pub fn getsockopt(fd: usize, level: usize, opt: usize, val: usize, len: usize) -> R {
    let (conv, s) = sock(fd)?;
    let v: i32 = match (level, opt) {
        (SOL_SOCKET, SO_TYPE) => (if s.stream { SOCK_STREAM } else { SOCK_DGRAM }) as i32,
        (SOL_SOCKET, SO_ERROR) if state(&conv) == State::Failed => ECONNREFUSED as i32,
        _ => 0,
    };
    put(val, &v.to_le_bytes())?;
    put(len, &4u32.to_le_bytes())?;
    Ok(0)
}

/// `setsockopt` and `bind`: accepted and ignored (options do not apply,
/// and a client socket binds implicitly).
pub fn ignored(fd: usize) -> R {
    sock(fd).map(|_| 0)
}

/// `listen` and `accept`: no listening sockets yet.
pub fn no_listen(fd: usize) -> R {
    sock(fd)?;
    Err(EOPNOTSUPP)
}

/// `poll` events of a socket (`None`: not a socket).
pub fn poll_events(fd: usize) -> Option<u16> {
    const POLLIN: u16 = 1;
    const POLLOUT: u16 = 4;
    const POLLERR: u16 = 8;
    const POLLHUP: u16 = 0x10;
    let (conv, s) = sock(fd).ok()?;
    let input = if pending(&conv) { POLLIN } else { 0 };
    Some(match state(&conv) {
        State::Connected => input | POLLOUT,
        State::Pending if !s.stream && s.peer.is_some() => input | POLLOUT,
        State::Pending => input,
        State::HungUp => POLLIN | POLLHUP,
        State::Failed => POLLIN | POLLOUT | POLLERR,
    })
}

/// `O_NONBLOCK` from `fcntl(F_SETFL)` / `ioctl(FIONBIO)`; false if `fd` is
/// not a socket.
pub fn set_nonblock(fd: usize, on: bool) -> bool {
    files::with_sock(fd, |s| s.nonblock = on).is_some()
}
