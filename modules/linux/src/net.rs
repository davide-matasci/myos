//! Sockets over the native `/net`: `AF_INET` (see `docs/sockets-curl.md`)
//! and `AF_UNIX` streams (`docs/sockets-unix.md`).
//!
//! A socket is a `/net/{tcp,udp,unix}/N` conversation and its fd is the
//! conversation's `data` file, so reads, writes, dup, fork and close are
//! ordinary fd operations, and the last close hangs the conversation up.
//! `connect` is a `ctl` write; readiness comes from `status` and the size of
//! `data` (the bytes netd delivered and nobody read yet). The `/net` files
//! never block, so waits sleep until netd's next reply (or, for a unix
//! socket, its peer) wakes the pollers. A unix socket's name is a name in
//! `/net/unix`, as for libgloss's sockets: a Linux program in a
//! `linux --root` reaches a native server by the path it listens on (the
//! X server's `/tmp/.X11-unix/X0`) with nothing in its root.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::abi::*;
use super::files;
use super::sys::{get_bytes, native, put, R};
use crate::k::{fs, signal, task, time, user};

const AF_UNIX: usize = 1;
const AF_INET: usize = 2;
const SOCK_STREAM: usize = 1;
const SOCK_DGRAM: usize = 2;
const SOCK_SEQPACKET: usize = 5;
const SOCK_NONBLOCK: usize = 0o4000;
const SOCK_CLOEXEC: usize = 0o2000000;
const MSG_DONTWAIT: usize = 0x40;
const SHUT_RDWR: usize = 2;
const SOL_SOCKET: usize = 1;
const SO_TYPE: usize = 3;
const SO_ERROR: usize = 4;

const EADDRINUSE: usize = 98;
const EISCONN: usize = 106;
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
    /// `AF_UNIX`: its names (an `AF_INET` socket has none).
    pub unix: Option<UnixNames>,
}

/// The names of an `AF_UNIX` socket in `/net/unix`: its own (`bind`; an
/// accepted socket has its listener's) and the one it connected to.
#[derive(Clone, Copy)]
pub struct UnixNames {
    pub name: Name,
    pub peer: Name,
}

/// `sun_path`'s size: a name is at most this long.
const NAME_CAP: usize = 108;

/// A `/net/unix` name: the `sun_path`, an abstract one (leading NUL) as
/// `@name`, as libgloss writes them. Empty: unnamed.
#[derive(Clone, Copy)]
pub struct Name {
    len: u8,
    b: [u8; NAME_CAP],
}

impl Name {
    const NONE: Name = Name { len: 0, b: [0; NAME_CAP] };

    fn as_bytes(&self) -> &[u8] {
        &self.b[..self.len as usize]
    }
}

const UNNAMED: UnixNames = UnixNames { name: Name::NONE, peer: Name::NONE };

/// A conversation's state, from its `status` file.
#[derive(PartialEq, Eq)]
enum State {
    /// Not connected yet (or a TCP handshake in flight).
    Pending,
    Connected,
    /// A unix listener (`listen`).
    Listening,
    /// The peer (or we) hung up; buffered data can still be read.
    HungUp,
    /// The connect failed or netd reported an error.
    Failed,
}

fn state(conv: &str) -> State {
    let mut b = [0u8; 64];
    let n = fs::read(&format!("{conv}/status"), 0, &mut b).unwrap_or(0);
    match &b[..n] {
        // Empty until netd acknowledges the `clone`; a unix conversation
        // is `open` until it connects or announces.
        b"" | b"cloned" | b"connecting" | b"open" => State::Pending,
        b"connected" => State::Connected,
        b"announced" => State::Listening,
        b"hangup" => State::HungUp,
        _ => State::Failed,
    }
}

/// Bytes waiting in the conversation's `data`.
fn pending(conv: &str) -> bool {
    fs::stat(&format!("{conv}/data")).is_some_and(|st| st.size > 0)
}

/// Connections queued on a unix listener's `listen`.
fn queued(conv: &str) -> bool {
    fs::stat(&format!("{conv}/listen")).is_some_and(|st| st.size > 0)
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

/// The `/net/unix` name of the `sockaddr_un` at `ptr` (`len` bytes): the
/// path up to its NUL, an abstract name (leading NUL) as `@name`.
fn get_name(ptr: usize, len: usize) -> Result<Name, usize> {
    const PATH: usize = 2;
    let mut b = [0u8; PATH + NAME_CAP];
    let len = len.min(b.len());
    if len <= PATH {
        return Err(EINVAL);
    }
    get_bytes(ptr, &mut b[..len])?;
    if u16::from_le_bytes([b[0], b[1]]) as usize != AF_UNIX {
        return Err(EAFNOSUPPORT);
    }
    let path = &b[PATH..len];
    let mut name = Name::NONE;
    let rest = if path[0] == 0 {
        name.b[0] = b'@';
        name.len = 1;
        &path[1..]
    } else {
        path
    };
    let n = rest.iter().position(|&c| c == 0).unwrap_or(rest.len());
    if n == 0 {
        return Err(EINVAL);
    }
    let at = name.len as usize;
    name.b[at..at + n].copy_from_slice(&rest[..n]);
    name.len = (at + n) as u8;
    Ok(name)
}

/// Store `name` as a `sockaddr_un` at `ptr` (`*len_ptr` bytes of room) and
/// its size at `len_ptr`: the family alone for an unnamed socket.
fn put_name(ptr: usize, len_ptr: usize, name: &Name) -> R {
    if ptr == 0 {
        return Ok(0);
    }
    let mut l = [0u8; 4];
    get_bytes(len_ptr, &mut l)?;
    let room = u32::from_le_bytes(l) as usize;
    let mut b = [0u8; 2 + NAME_CAP];
    b[..2].copy_from_slice(&(AF_UNIX as u16).to_le_bytes());
    let n = name.as_bytes();
    b[2..2 + n.len()].copy_from_slice(n);
    let size = match n.first() {
        None => 2,
        Some(b'@') => {
            b[2] = 0; // abstract: no NUL after it
            2 + n.len()
        }
        Some(_) => (2 + n.len() + 1).min(b.len()),
    };
    put(ptr, &b[..room.min(size)])?;
    put(len_ptr, &(size as u32).to_le_bytes())?;
    Ok(0)
}

pub fn socket(domain: usize, ty: usize) -> R {
    if domain == AF_UNIX {
        if !matches!(ty & !(SOCK_NONBLOCK | SOCK_CLOEXEC), SOCK_STREAM | SOCK_SEQPACKET) {
            return Err(EPROTOTYPE);
        }
        let id = conv_id(&format!("{}/clone", net_dir("unix")?))?;
        return open_conv("unix", &id, ty, UNIX_SOCK);
    }
    if domain != AF_INET {
        return Err(EAFNOSUPPORT);
    }
    let stream = match ty & !(SOCK_NONBLOCK | SOCK_CLOEXEC) {
        SOCK_STREAM => true,
        SOCK_DGRAM => false,
        _ => return Err(EPROTOTYPE),
    };
    let proto = if stream { "tcp" } else { "udp" };
    // Reading `clone` allocates a conversation and names it.
    let dir = net_dir(proto)?;
    let id = conv_id(&format!("{dir}/clone"))?;
    open_conv(proto, &id, ty, Sock { stream, nonblock: false, peer: None, unix: None })
}

/// `/net/{proto}`'s VFS path. `/net` is the system's, also inside a
/// `linux --root` chroot (a bind): conversations are kept by VFS path.
fn net_dir(proto: &str) -> Result<String, usize> {
    user::resolve_copied_path(&format!("/net/{proto}")).ok_or(EAFNOSUPPORT)
}

/// The conversation number read from `file` (`clone` or `listen`).
fn conv_id(file: &str) -> Result<String, usize> {
    let mut b = [0u8; 8];
    let n = fs::read(file, 0, &mut b).unwrap_or(0);
    let id = core::str::from_utf8(&b[..n]).ok().map(str::trim).filter(|s| !s.is_empty());
    id.map(String::from).ok_or(EMFILE)
}

/// A new unix socket's state.
const UNIX_SOCK: Sock = Sock { stream: true, nonblock: false, peer: None, unix: Some(UNNAMED) };

/// Conversation `id` of `/net/{proto}` as socket `sock`, non-blocking and
/// close-on-exec as `ty`'s flags say: its `data` open.
fn open_conv(proto: &str, id: &str, ty: usize, sock: Sock) -> R {
    let conv = format!("{}/{id}", net_dir(proto)?);
    let fd = user::open_path(&format!("/net/{proto}/{id}/data"), 2);
    if fd >= signal::SYSERR_LOWEST {
        let _ = fs::write(&format!("{conv}/ctl"), 0, b"hangup");
        return Err(EMFILE);
    }
    files::set_sock(fd, conv, Sock { nonblock: ty & SOCK_NONBLOCK != 0, ..sock });
    files::set_cloexec(fd, ty & SOCK_CLOEXEC != 0);
    Ok(fd)
}

pub fn connect(fd: usize, addr: usize, len: usize) -> R {
    let (conv, s) = sock(fd)?;
    if let Some(u) = s.unix {
        // Queued on the listener, or refused, at once: no handshake.
        if state(&conv) != State::Pending {
            return Err(EISCONN);
        }
        let peer = get_name(addr, len)?;
        let mut cmd = Vec::from(&b"connect "[..]);
        cmd.extend_from_slice(peer.as_bytes());
        fs::write(&format!("{conv}/ctl"), 0, &cmd).ok_or(ECONNREFUSED)?;
        files::with_sock(fd, |s| s.unix = Some(UnixNames { peer, ..u }));
        // The listener has a connection queued: wake its server, asleep in
        // `poll` or `accept`. A `ctl` written without an fd wakes nobody.
        task::wake_any();
        return Ok(0);
    }
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
            State::Pending | State::Listening => return Err(ENOTCONN),
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
    let s = sock(fd)?.1;
    if let Some(peer) = s.peer {
        put_addr(from, fromlen, peer)?;
    } else if s.unix.is_some() && from != 0 {
        // A stream's bytes have no source address.
        put(fromlen, &0u32.to_le_bytes())?;
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
    let s = sock(fd)?.1;
    if let Some(peer) = s.peer {
        put_addr(name, msg + 8, peer)?;
    } else if s.unix.is_some() && name != 0 {
        put(msg + 8, &0u32.to_le_bytes())?;
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
        // The peer's reads end: news for its pollers.
        task::wake_any();
    }
    Ok(0)
}

/// A unix socket's own name; an `AF_INET` socket's local address is not
/// known: `0.0.0.0:0`.
pub fn getsockname(fd: usize, addr: usize, alen: usize) -> R {
    match sock(fd)?.1.unix {
        Some(u) => put_name(addr, alen, &u.name),
        None => put_addr(addr, alen, ([0; 4], 0)),
    }
}

/// The name a unix socket connected to (unnamed: an accepted or paired
/// one's peer); an `AF_INET` socket's peer address.
pub fn getpeername(fd: usize, addr: usize, alen: usize) -> R {
    let (conv, s) = sock(fd)?;
    match s.unix {
        Some(u) if state(&conv) == State::Connected => put_name(addr, alen, &u.peer),
        Some(_) => Err(ENOTCONN),
        None => put_addr(addr, alen, s.peer.ok_or(ENOTCONN)?),
    }
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

/// `setsockopt`: accepted and ignored (options do not apply).
pub fn ignored(fd: usize) -> R {
    sock(fd).map(|_| 0)
}

/// `bind`: a unix socket takes the name its `listen` announces; an
/// `AF_INET` one ignores it (a client socket binds implicitly).
pub fn bind(fd: usize, addr: usize, len: usize) -> R {
    let Some(u) = sock(fd)?.1.unix else {
        return Ok(0);
    };
    let name = get_name(addr, len)?;
    files::with_sock(fd, |s| s.unix = Some(UnixNames { name, ..u }));
    Ok(0)
}

/// `listen`: a bound unix socket announces its name in `/net/unix`. No
/// `AF_INET` listeners.
pub fn listen(fd: usize) -> R {
    let (conv, s) = sock(fd)?;
    let Some(u) = s.unix else {
        return Err(EOPNOTSUPP);
    };
    match state(&conv) {
        State::Listening => return Ok(0),
        State::Pending if u.name.len != 0 => {}
        _ => return Err(EINVAL),
    }
    let mut cmd = Vec::from(&b"announce "[..]);
    cmd.extend_from_slice(u.name.as_bytes());
    fs::write(&format!("{conv}/ctl"), 0, &cmd).map(|_| 0).ok_or(EADDRINUSE)
}

/// `accept` / `accept4`: the next connection queued on a unix listener,
/// waited for unless the listener is non-blocking; `flags` as `socket`'s
/// type flags. The new socket has the listener's name, its peer none.
pub fn accept(fd: usize, addr: usize, alen: usize, flags: usize) -> R {
    let (conv, s) = sock(fd)?;
    let Some(u) = s.unix else {
        return Err(EOPNOTSUPP);
    };
    if state(&conv) != State::Listening {
        return Err(EINVAL);
    }
    let listen = format!("{conv}/listen");
    let id = loop {
        if let Ok(id) = conv_id(&listen) {
            break id;
        }
        if s.nonblock {
            return Err(EAGAIN);
        }
        wait(|| queued(&conv), 0)?;
    };
    let names = UnixNames { name: u.name, peer: Name::NONE };
    let new = open_conv("unix", &id, flags, Sock { unix: Some(names), ..UNIX_SOCK })?;
    put_name(addr, alen, &Name::NONE)?;
    Ok(new)
}

/// `socketpair(AF_UNIX, ...)`: a `/net/unix` conversation and the one its
/// `pair` connects to it (taken from its `listen`), both kept as sockets
/// like TCP's. Stream semantics for `SOCK_SEQPACKET` too: no message
/// boundaries, and no fd passing.
pub fn socketpair(domain: usize, ty: usize, sv: usize) -> R {
    if domain != AF_UNIX {
        return Err(EAFNOSUPPORT);
    }
    if !matches!(ty & !(SOCK_NONBLOCK | SOCK_CLOEXEC), SOCK_STREAM | SOCK_SEQPACKET) {
        return Err(EPROTOTYPE);
    }
    let dir = net_dir("unix")?;
    let id = conv_id(&format!("{dir}/clone"))?;
    let conv = format!("{dir}/{id}");
    let a = open_conv("unix", &id, ty, UNIX_SOCK)?;
    let b = fs::write(&format!("{conv}/ctl"), 0, b"pair")
        .ok_or(EMFILE)
        .and_then(|_| conv_id(&format!("{conv}/listen")))
        .and_then(|id| open_conv("unix", &id, ty, UNIX_SOCK));
    let b = match b {
        Ok(b) => b,
        Err(e) => {
            super::sys::close(a).ok();
            return Err(e);
        }
    };
    let mut v = [0u8; 8];
    v[..4].copy_from_slice(&(a as i32).to_le_bytes());
    v[4..].copy_from_slice(&(b as i32).to_le_bytes());
    put(sv, &v)?;
    Ok(0)
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
        State::Listening if queued(&conv) => POLLIN,
        State::Listening => 0,
        State::HungUp => POLLIN | POLLHUP,
        State::Failed => POLLIN | POLLOUT | POLLERR,
    })
}

/// `O_NONBLOCK` from `fcntl(F_SETFL)` / `ioctl(FIONBIO)`; false if `fd` is
/// not a socket.
pub fn set_nonblock(fd: usize, on: bool) -> bool {
    files::with_sock(fd, |s| s.nonblock = on).is_some()
}
