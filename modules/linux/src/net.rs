//! Sockets over the native `/net`: `AF_INET` and `AF_INET6` (see
//! `docs/sockets-curl.md`) and `AF_UNIX` streams and records
//! (`SOCK_SEQPACKET`, `docs/sockets-unix.md`).
//! An `AF_INET6` socket names an IPv4 peer `::ffff:a.b.c.d`, and one it
//! connects or sends to talks IPv4 to netd, as on Linux.
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
use super::sys::{get_bytes, get_u64, native, put, R};
use crate::k::{fs, signal, task, time, user};

const AF_UNIX: usize = 1;
const AF_INET: usize = 2;
const AF_INET6: usize = 10;
const SOCK_STREAM: usize = 1;
const SOCK_DGRAM: usize = 2;
const SOCK_SEQPACKET: usize = 5;
const SOCK_NONBLOCK: usize = 0o4000;
const SOCK_CLOEXEC: usize = 0o2000000;
const MSG_TRUNC: u32 = 0x20;
const MSG_DONTWAIT: usize = 0x40;
const SHUT_RDWR: usize = 2;
const SOL_SOCKET: usize = 1;
const SO_TYPE: usize = 3;
const SO_ERROR: usize = 4;
const SO_SNDBUF: usize = 7;
const SO_RCVBUF: usize = 8;
const SO_PASSCRED: usize = 16;
const SO_PEERCRED: usize = 17;
/// The peer's security label, groups and pidfd: what a kernel without
/// them answers (`ENOPROTOOPT`); an answer of 0 would be taken for a
/// label, a group or an fd (zbus closes a "pidfd" 0: the program's stdin).
const SO_PEERSEC: usize = 31;
const SO_PEERGROUPS: usize = 59;
const SO_PEERPIDFD: usize = 77;

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

/// An IP address (IPv4 in the first four bytes, or IPv6) and port.
#[derive(Clone, Copy)]
pub struct Addr {
    v6: bool,
    ip: [u8; 16],
    port: u16,
}

impl Addr {
    const ANY4: Addr = Addr { v6: false, ip: [0; 16], port: 0 };
    const ANY6: Addr = Addr { v6: true, ip: [0; 16], port: 0 };

    /// The text netd takes and gives: `a.b.c.d` or an IPv6 address.
    fn text(&self) -> String {
        if self.v6 {
            format!("{}", core::net::Ipv6Addr::from(self.ip))
        } else {
            let [a, b, c, d, ..] = self.ip;
            format!("{a}.{b}.{c}.{d}")
        }
    }

    /// netd's `addr!port`.
    fn parse(t: &str) -> Option<Addr> {
        let (ip, port) = t.rsplit_once('!')?;
        let port = port.parse().ok()?;
        match ip.parse::<core::net::IpAddr>().ok()? {
            core::net::IpAddr::V4(a) => {
                let mut ip = [0; 16];
                ip[..4].copy_from_slice(&a.octets());
                Some(Addr { v6: false, ip, port })
            }
            core::net::IpAddr::V6(a) => Some(Addr { v6: true, ip: a.octets(), port }),
        }
    }

    /// The address as in netd's datagram header: an IPv4 one as
    /// `::ffff:a.b.c.d`, `::` all zeros (this host, with a port).
    fn hdr(&self) -> [u8; 16] {
        if self.v6 {
            return self.ip;
        }
        let mut b = [0u8; 16];
        b[10] = 0xff;
        b[11] = 0xff;
        b[12..].copy_from_slice(&self.ip[..4]);
        b
    }

    fn from_hdr(b: &[u8], port: u16) -> Addr {
        let mut ip = [0u8; 16];
        if b[..12] == [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff] {
            ip[..4].copy_from_slice(&b[12..16]);
            Addr { v6: false, ip, port }
        } else {
            ip.copy_from_slice(&b[..16]);
            Addr { v6: true, ip, port }
        }
    }
}

/// A socket's state beyond its fd (kept in the fd table, `files`).
#[derive(Clone, Copy)]
pub struct Sock {
    pub stream: bool,
    pub nonblock: bool,
    /// `AF_INET6` (its addresses are `sockaddr_in6`s).
    pub inet6: bool,
    pub peer: Option<Addr>,
    /// `AF_UNIX`: its names (an `AF_INET` socket has none).
    pub unix: Option<UnixNames>,
    /// `SO_PASSCRED`: reads come with the sender's `SCM_CREDENTIALS`.
    pub passcred: bool,
    /// `AF_UNIX` `SOCK_SEQPACKET`: its conversation keeps records.
    pub records: bool,
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
        // is `open` until it connects or announces. A UDP one's answer to a
        // tagged command (`#<n> ok ...`, `udp_local`) says nothing of its
        // state: a datagram socket's peer is the layer's (`Sock::peer`).
        b"" | b"cloned" | b"connecting" | b"open" => State::Pending,
        [b'#', ..] => State::Pending,
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

/// The `sockaddr_in` (`sockaddr_in6` for an `AF_INET6` socket) at `ptr`
/// (`len` bytes): an IPv4-mapped IPv6 address is the IPv4 one.
fn get_addr(ptr: usize, len: usize, inet6: bool) -> Result<Addr, usize> {
    let mut b = [0u8; 24];
    let need = if inet6 { 24 } else { 8 };
    if len < need {
        return Err(EINVAL);
    }
    get_bytes(ptr, &mut b[..need])?;
    let family = u16::from_le_bytes([b[0], b[1]]) as usize;
    let port = u16::from_be_bytes([b[2], b[3]]);
    match (inet6, family) {
        (false, AF_INET) => {
            let mut ip = [0; 16];
            ip[..4].copy_from_slice(&b[4..8]);
            Ok(Addr { v6: false, ip, port })
        }
        // sin6_flowinfo at 4, sin6_addr at 8.
        (true, AF_INET6) => Ok(Addr::from_hdr(&b[8..24], port)),
        _ => Err(EAFNOSUPPORT),
    }
}

/// Store `a` as a `sockaddr_in`, or for an `AF_INET6` socket a
/// `sockaddr_in6` (an IPv4 address `::ffff:a.b.c.d`, 0.0.0.0 `::`), at `ptr`
/// (`*len_ptr` bytes of room) and its size at `len_ptr`.
fn put_addr(ptr: usize, len_ptr: usize, a: Addr, inet6: bool) -> R {
    if ptr == 0 {
        return Ok(0);
    }
    let mut l = [0u8; 4];
    get_bytes(len_ptr, &mut l)?;
    let room = u32::from_le_bytes(l) as usize;
    let mut b = [0u8; 28];
    b[2..4].copy_from_slice(&a.port.to_be_bytes());
    let size = if inet6 {
        b[..2].copy_from_slice(&(AF_INET6 as u16).to_le_bytes());
        if a.v6 || a.ip[..4] != [0; 4] {
            b[8..24].copy_from_slice(&a.hdr());
        }
        28
    } else {
        b[..2].copy_from_slice(&(AF_INET as u16).to_le_bytes());
        if !a.v6 {
            b[4..8].copy_from_slice(&a.ip[..4]);
        }
        16
    };
    put(ptr, &b[..room.min(size)])?;
    put(len_ptr, &(size as u32).to_le_bytes())?;
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
        return new_unix(ty);
    }
    if domain != AF_INET && domain != AF_INET6 {
        return Err(EAFNOSUPPORT);
    }
    let stream = match ty & !(SOCK_NONBLOCK | SOCK_CLOEXEC) {
        SOCK_STREAM => true,
        SOCK_DGRAM => false,
        _ => return Err(EPROTOTYPE),
    };
    let proto = if stream { "tcp" } else { "udp" };
    let inet6 = domain == AF_INET6;
    let fd = new_conv(proto, ty, Sock { stream, nonblock: false, inet6, peer: None, unix: None, passcred: false, records: false })?;
    // An IPv6 datagram socket's port is on `::`, which takes IPv4 datagrams
    // too, as on Linux (musl's resolver asks IPv4 and IPv6 nameservers
    // from one such socket); tagged, its answer is no `connected`.
    if inet6 && !stream {
        let (conv, _) = sock(fd)?;
        let _ = fs::write(&format!("{conv}/ctl"), 0, b"autobind :: #0");
    }
    Ok(fd)
}

/// `/net/{proto}`'s VFS path. `/net` is the system's, also inside a
/// `linux --root` chroot (a bind): conversations are kept by VFS path.
fn net_dir(proto: &str) -> Result<String, usize> {
    user::resolve_copied_path(&format!("/net/{proto}")).ok_or(EAFNOSUPPORT)
}

/// The conversation number read from `file` (a `listen`).
fn conv_id(file: &str) -> Result<String, usize> {
    let mut b = [0u8; 8];
    let n = fs::read(file, 0, &mut b).unwrap_or(0);
    let id = core::str::from_utf8(&b[..n]).ok().map(str::trim).filter(|s| !s.is_empty());
    id.map(String::from).ok_or(EMFILE)
}

/// A new unix socket's state.
const UNIX_SOCK: Sock = Sock { stream: true, nonblock: false, inet6: false, peer: None, unix: Some(UNNAMED), passcred: false, records: false };

/// The largest record: what the kernel hands a file in one write (a larger
/// `write` would reach `/net/unix` in pieces, each a record).
const RECORD_MAX: usize = 8 << 10;

/// A new unix socket of type `ty` (with its flags): a stream, or a record
/// socket whose conversation keeps records from the start (`seqpacket`).
fn new_unix(ty: usize) -> R {
    let records = match ty & !(SOCK_NONBLOCK | SOCK_CLOEXEC) {
        SOCK_STREAM => false,
        SOCK_SEQPACKET => true,
        _ => return Err(EPROTOTYPE),
    };
    let fd = new_conv("unix", ty, Sock { records, ..UNIX_SOCK })?;
    if records {
        let (conv, _) = sock(fd)?;
        if fs::write(&format!("{conv}/ctl"), 0, b"seqpacket").is_none() {
            super::sys::close(fd).ok();
            return Err(EMFILE);
        }
    }
    Ok(fd)
}

/// readv / writev / sendmsg / recvmsg on a record socket: one record,
/// gathered from or scattered over the `iovec`s (a write per piece would
/// make each its own record). `None` for any other fd.
pub fn record_rw(fd: usize, iov: usize, cnt: usize, write: bool) -> Option<R> {
    let (conv, s) = sock(fd).ok()?;
    if !s.records {
        return None;
    }
    let mut pieces = Vec::new();
    let mut total = 0usize;
    for i in 0..cnt {
        let (Ok(base), Ok(len)) = (get_u64(iov + i * 16), get_u64(iov + i * 16 + 8)) else {
            return Some(Err(EFAULT));
        };
        pieces.push((base as usize, len as usize));
        total = total.saturating_add(len as usize);
    }
    let data = format!("{conv}/data");
    if write {
        if total > RECORD_MAX {
            return Some(Err(EMSGSIZE));
        }
        let mut rec = alloc::vec![0u8; total];
        let mut at = 0;
        for &(base, len) in &pieces {
            if get_bytes(base, &mut rec[at..at + len]).is_err() {
                return Some(Err(EFAULT));
            }
            at += len;
        }
        return Some(send(fd, || fs::write(&data, 0, &rec).ok_or(EAGAIN)));
    }
    let mut rec = alloc::vec![0u8; total.min(RECORD_MAX)];
    Some(recv(fd, false, || {
        let n = fs::read(&data, 0, &mut rec).unwrap_or(0);
        let mut left = &rec[..n];
        for &(base, len) in &pieces {
            let take = len.min(left.len());
            put(base, &left[..take])?;
            left = &left[take..];
        }
        Ok(n)
    }))
}

/// Whether a write of `len` bytes to `fd` is a record no socket buffer
/// holds (`EMSGSIZE`).
pub fn record_too_big(fd: usize, len: usize) -> bool {
    len > RECORD_MAX && sock(fd).is_ok_and(|(_, s)| s.records)
}

/// Whether the last read of record socket `conv` dropped the rest of a
/// record (its `ctl` says `seqpacket cut`).
fn record_cut(conv: &str) -> bool {
    let mut b = [0u8; 32];
    let n = fs::read(&format!("{conv}/ctl"), 0, &mut b).unwrap_or(0);
    b[..n].windows(3).any(|w| w == b"cut")
}

/// A new conversation of `/net/{proto}` as socket `sock`: opening `clone`
/// makes it, and the fd is on its `data` (whose link names it), until
/// whose last close it lives.
fn new_conv(proto: &str, ty: usize, sock: Sock) -> R {
    let fd = user::open_path(&format!("/net/{proto}/clone"), 2);
    if fd >= signal::SYSERR_LOWEST {
        return Err(EMFILE);
    }
    let prefix = format!("/net/{proto}/");
    let id = task::fd_path(fd).and_then(|path| {
        let path = core::str::from_utf8(&path).ok()?;
        Some(String::from(path.strip_prefix(prefix.as_str())?.strip_suffix("/data")?))
    });
    let Some(id) = id else {
        super::sys::close(fd).ok();
        return Err(EMFILE);
    };
    as_sock(fd, proto, &id, ty, sock)
}

/// Conversation `id` of `/net/{proto}` (one a `listen` handed out) as
/// socket `sock`: its `data` open.
fn open_conv(proto: &str, id: &str, ty: usize, sock: Sock) -> R {
    let fd = user::open_path(&format!("/net/{proto}/{id}/data"), 2);
    if fd >= signal::SYSERR_LOWEST {
        let _ = fs::write(&format!("{}/{id}/ctl", net_dir(proto)?), 0, b"hangup");
        return Err(EMFILE);
    }
    as_sock(fd, proto, id, ty, sock)
}

/// `fd`, open on conversation `id`'s `data`, as socket `sock`, non-blocking
/// and close-on-exec as `ty`'s flags say.
fn as_sock(fd: usize, proto: &str, id: &str, ty: usize, sock: Sock) -> R {
    let conv = format!("{}/{id}", net_dir(proto)?);
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
    let mut peer = get_addr(addr, len, s.inet6)?;
    ctl_connect(&conv, peer)?;
    // An unspecified address is this host (netd's way, and Linux's).
    if peer.ip == [0; 16] {
        if peer.v6 {
            peer.ip[15] = 1;
        } else {
            peer.ip[..4].copy_from_slice(&[127, 0, 0, 1]);
        }
    }
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

/// A UDP datagram's header in a conversation's `hdata`
/// (`docs/sockets-curl.md`): the remote and local addresses (16 bytes each,
/// IPv4 as `::ffff:a.b.c.d`), the remote and local ports.
const UDP_HDR: usize = 36;
/// The most a datagram carries.
const UDP_MAX: usize = 65535;

/// `sendto` on a datagram socket: the datagram `data` to `to`, with its
/// header, through the conversation's `hdata` (a connected socket's peer
/// is not touched: musl's resolver asks every nameserver from one socket
/// and takes the first answer, from whichever).
fn send_datagram(fd: usize, data: &[u8], to: usize, tolen: usize) -> R {
    let (conv, s) = sock(fd)?;
    let to = get_addr(to, tolen, s.inet6)?;
    let mut msg = alloc::vec![0u8; UDP_HDR + data.len()];
    msg[..16].copy_from_slice(&to.hdr());
    msg[32..34].copy_from_slice(&to.port.to_be_bytes());
    msg[UDP_HDR..].copy_from_slice(data);
    // netd's request ring may be full: a blocking socket retries.
    loop {
        let seq = task::wait_seq();
        if fs::write(&format!("{conv}/hdata"), 0, &msg).is_some() {
            return Ok(data.len());
        }
        if s.nonblock {
            return Err(EAGAIN);
        }
        if signal::interrupt_wait() {
            return Err(EINTR);
        }
        task::block_until(task::WAIT_ANY, seq, time::monotonic_ns() + RETRY_NS);
    }
}

/// `recvfrom` on a datagram socket: the next datagram from `hdata`, its
/// sender from its header.
fn recv_datagram(fd: usize, buf: usize, len: usize, flags: usize, from: usize, fromlen: usize) -> R {
    let (conv, s) = sock(fd)?;
    let mut msg = alloc::vec![0u8; UDP_HDR + len.min(UDP_MAX)];
    let got = recv(fd, flags & MSG_DONTWAIT != 0, || Ok(fs::read(&format!("{conv}/hdata"), 0, &mut msg).unwrap_or(0)))?;
    if got < UDP_HDR {
        return Ok(0);
    }
    let n = got - UDP_HDR;
    put(buf, &msg[UDP_HDR..UDP_HDR + n])?;
    put_addr(from, fromlen, Addr::from_hdr(&msg, u16::from_be_bytes([msg[32], msg[33]])), s.inet6)?;
    Ok(n)
}

fn ctl_connect(conv: &str, a: Addr) -> Result<(), usize> {
    let cmd = format!("connect {}!{}", a.text(), a.port);
    fs::write(&format!("{conv}/ctl"), 0, cmd.as_bytes()).map(|_| ()).ok_or(EIO)
}

/// A UDP socket's local address and port, netd's answer to a tagged
/// `local` (`docs/sockets-curl.md`): what a connect or send chose.
fn udp_local(conv: &str) -> Option<Addr> {
    let tag = format!("#{}", time::monotonic_ns());
    fs::write(&format!("{conv}/ctl"), 0, format!("local {tag}").as_bytes())?;
    let want = format!("{tag} ok ");
    let mut found = None;
    let _ = wait(
        || {
            let mut b = [0u8; 160];
            let n = fs::read(&format!("{conv}/status"), 0, &mut b).unwrap_or(0);
            let st = core::str::from_utf8(&b[..n]).unwrap_or("");
            found = st.strip_prefix(want.as_str()).and_then(|t| Addr::parse(t.split(' ').next()?));
            found.is_some()
        },
        time::monotonic_ns() + 1_000_000_000,
    );
    found
}

/// Whether `fd` is an `AF_INET` datagram socket (`hdata` carries its
/// addressed datagrams).
fn is_datagram(s: &Sock) -> bool {
    !s.stream && s.unix.is_none()
}

pub fn sendto(fd: usize, buf: usize, len: usize, to: usize, tolen: usize) -> R {
    let s = sock(fd)?.1;
    if to != 0 && is_datagram(&s) {
        if len > UDP_MAX {
            return Err(EMSGSIZE);
        }
        let mut data = alloc::vec![0u8; len];
        get_bytes(buf, &mut data)?;
        return send_datagram(fd, &data, to, tolen);
    }
    super::sys::write(fd, buf, len)
}

pub fn recvfrom(fd: usize, buf: usize, len: usize, flags: usize, from: usize, fromlen: usize) -> R {
    let s = sock(fd)?.1;
    if is_datagram(&s) {
        return recv_datagram(fd, buf, len, flags, from, fromlen);
    }
    let n = recv(fd, flags & MSG_DONTWAIT != 0, || native(user::sys_read(fd, buf, len), EBADF))?;
    if let Some(peer) = s.peer {
        put_addr(from, fromlen, peer, s.inet6)?;
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

/// `SCM_RIGHTS`: a control message of fds.
const SCM_RIGHTS: u32 = 1;
/// `SCM_CREDENTIALS`: a control message of the sender's `struct ucred`.
const SCM_CREDENTIALS: u32 = 2;
/// `recvmsg`: the fds received close at exec.
const MSG_CMSG_CLOEXEC: usize = 0x4000_0000;
/// `msg_flags`: control data was cut (fds that did not fit are closed).
const MSG_CTRUNC: u32 = 8;
/// A `struct cmsghdr`: `cmsg_len` (64 bits), `cmsg_level`, `cmsg_type`.
const CMSG_HDR: usize = 16;
/// The most control data a `sendmsg` hands over (Linux's `optmem_max`).
const CONTROL_MAX: usize = 20480;

/// The fds of `msg`'s `SCM_RIGHTS` control messages (`msg_control` and
/// `msg_controllen` at 32 and 40); other messages (credentials) are
/// ignored.
fn sent_rights(msg: usize) -> Result<Vec<i32>, usize> {
    let mut b = [0u8; 16];
    get_bytes(msg + 32, &mut b)?;
    let (control, len) = (u64::from_le_bytes(b[..8].try_into().unwrap()) as usize, u64::from_le_bytes(b[8..].try_into().unwrap()) as usize);
    let mut fds = Vec::new();
    if control == 0 || len < CMSG_HDR {
        return Ok(fds);
    }
    if len > CONTROL_MAX {
        return Err(ENOBUFS);
    }
    let mut data = alloc::vec![0u8; len];
    get_bytes(control, &mut data)?;
    let mut off = 0;
    while off + CMSG_HDR <= len {
        let clen = u64::from_le_bytes(data[off..off + 8].try_into().unwrap()) as usize;
        let level = u32::from_le_bytes(data[off + 8..off + 12].try_into().unwrap());
        let ty = u32::from_le_bytes(data[off + 12..off + 16].try_into().unwrap());
        if clen < CMSG_HDR || clen > len - off {
            return Err(EINVAL);
        }
        if level as usize == SOL_SOCKET && ty == SCM_RIGHTS {
            for f in data[off + CMSG_HDR..off + clen].chunks_exact(4) {
                fds.push(i32::from_le_bytes(f.try_into().unwrap()));
            }
        }
        off += clen.next_multiple_of(8);
    }
    Ok(fds)
}

/// `sendmsg`: the data, with the fds of an `SCM_RIGHTS` message on a unix
/// socket (`/net/unix`'s `rights`, `docs/sockets-unix.md`): they come with
/// its first byte.
pub fn sendmsg(fd: usize, msg: usize) -> R {
    let [name, namelen, iov, iovlen] = msghdr(msg)?;
    let fds = sent_rights(msg)?;
    if name != 0 && is_datagram(&sock(fd)?.1) {
        // One datagram of the gathered pieces.
        if iovlen > 1024 {
            return Err(EINVAL);
        }
        let mut data = Vec::new();
        for i in 0..iovlen {
            let (base, len) = (get_u64(iov + i * 16)? as usize, get_u64(iov + i * 16 + 8)? as usize);
            if data.len() + len > UDP_MAX {
                return Err(EMSGSIZE);
            }
            let at = data.len();
            data.resize(at + len, 0);
            get_bytes(base, &mut data[at..])?;
        }
        return send_datagram(fd, &data, name, namelen);
    }
    if fds.is_empty() {
        return super::sys::rw_vec(fd, iov, iovlen, true);
    }
    let (conv, s) = sock(fd)?;
    if s.unix.is_none() {
        return Err(EINVAL);
    }
    let mut cmd = String::from("rights");
    for f in fds {
        cmd.push_str(&format!(" {f}"));
    }
    let ctl = format!("{conv}/ctl");
    fs::write(&ctl, 0, cmd.as_bytes()).ok_or(EBADF)?;
    let r = super::sys::rw_vec(fd, iov, iovlen, true);
    if !matches!(r, Ok(n) if n > 0) {
        // Not sent: a retry sends them again.
        let _ = fs::write(&ctl, 0, b"norights");
    }
    r
}

/// The fds that came with the data a unix socket's last read took, made
/// the caller's (`/net/unix`'s `rights`) and given the layer's state.
fn received_rights(conv: &str, cloexec: bool) -> Vec<usize> {
    let mut b = [0u8; 4 * 253 + 2];
    let n = fs::read(&format!("{conv}/rights"), 0, &mut b).unwrap_or(0);
    let fds: Vec<usize> = core::str::from_utf8(&b[..n])
        .unwrap_or("")
        .split_ascii_whitespace()
        .filter_map(|w| w.parse().ok())
        .collect();
    for &fd in &fds {
        adopt(fd);
        files::set_cloexec(fd, cloexec);
    }
    fds
}

/// An fd another process sent: the state the layer keeps for what it is
/// open on, from its path (`/proc/self/fd`'s): a unix socket's
/// conversation, a signal fd's translation, a file's path.
fn adopt(fd: usize) {
    let Some(path) = task::fd_path(fd).and_then(|p| String::from_utf8(p).ok()) else {
        return;
    };
    // `.../unix/N/data`, whichever mount names it (a `linux --root` sees
    // `/net` through a bind): the conversation as this process reaches it.
    let unix = path.strip_suffix("/data").and_then(|p| {
        let (dir, id) = p.rsplit_once('/')?;
        (dir.ends_with("/unix") && !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then_some(id)
    });
    let unix = unix.and_then(|id| Some(format!("{}/{id}", net_dir("unix").ok()?)));
    if let Some(conv) = unix {
        // A record socket's ctl says so.
        let mut b = [0u8; 32];
        let n = fs::read(&format!("{conv}/ctl"), 0, &mut b).unwrap_or(0);
        let records = b[..n].starts_with(b"seqpacket");
        files::set_sock(fd, conv, Sock { records, ..UNIX_SOCK });
    } else if path.starts_with("/dev/sig/") {
        files::set_sigfd(fd, true);
    } else if path.starts_with('/') {
        let dir = fs::stat(&path).is_some_and(|st| st.mode & fs::S_IFMT == 0o040000);
        files::set(fd, path, dir);
    }
}

/// `recvmsg`: the data, and on a unix socket the fds that came with it as
/// an `SCM_RIGHTS` message (the ones `msg_control` has no room for are
/// closed, `MSG_CTRUNC`), after the sender's `SCM_CREDENTIALS` with
/// `SO_PASSCRED`.
pub fn recvmsg(fd: usize, msg: usize, flags: usize) -> R {
    let [name, _, iov, iovlen] = msghdr(msg)?;
    let (conv, s) = sock(fd)?;
    if is_datagram(&s) {
        // One datagram, scattered over the pieces, its sender named (how
        // musl's resolver reads its answers); no control data.
        if iovlen > 1024 {
            return Err(EINVAL);
        }
        let mut room = 0usize;
        for i in 0..iovlen {
            room = room.saturating_add(get_u64(iov + i * 16 + 8)? as usize);
        }
        let mut data = alloc::vec![0u8; UDP_HDR + room.min(UDP_MAX)];
        let got = recv(fd, flags & MSG_DONTWAIT != 0, || Ok(fs::read(&format!("{conv}/hdata"), 0, &mut data).unwrap_or(0)))?;
        let n = got.saturating_sub(UDP_HDR);
        let mut left = &data[UDP_HDR..UDP_HDR + n];
        for i in 0..iovlen {
            let (base, len) = (get_u64(iov + i * 16)? as usize, get_u64(iov + i * 16 + 8)? as usize);
            let take = len.min(left.len());
            put(base, &left[..take])?;
            left = &left[take..];
        }
        if got >= UDP_HDR {
            put_addr(name, msg + 8, Addr::from_hdr(&data, u16::from_be_bytes([data[32], data[33]])), s.inet6)?;
        }
        put(msg + 40, &0u64.to_le_bytes())?;
        put(msg + 48, &0u32.to_le_bytes())?;
        return Ok(n);
    }
    let n = recv(fd, flags & MSG_DONTWAIT != 0, || super::sys::rw_vec(fd, iov, iovlen, false))?;
    let mut room = 0usize;
    for i in 0..iovlen.min(1024) {
        room = room.saturating_add(get_u64(iov + i * 16 + 8)? as usize);
    }
    // A record that filled the buffers may have been longer.
    let cut = s.records && n > 0 && n >= room.min(RECORD_MAX) && record_cut(&conv);
    if let Some(peer) = s.peer {
        put_addr(name, msg + 8, peer, s.inet6)?;
    } else if s.unix.is_some() && name != 0 {
        put(msg + 8, &0u32.to_le_bytes())?;
    }
    let fds = if s.unix.is_some() && n > 0 { received_rights(&conv, flags & MSG_CMSG_CLOEXEC != 0) } else { Vec::new() };
    let from = if s.unix.is_some() && s.passcred && n > 0 { sender(&conv) } else { None };
    let (used, truncated) = put_control(msg, from, &fds)?;
    put(msg + 40, &(used as u64).to_le_bytes())?;
    let flags = (if truncated { MSG_CTRUNC } else { 0 }) | (if cut { MSG_TRUNC } else { 0 });
    put(msg + 48, &flags.to_le_bytes())?;
    Ok(n)
}

/// The pid of the process whose write a read of `conv` returned
/// (`/net/unix`'s `from`).
fn sender(conv: &str) -> Option<u32> {
    let mut b = [0u8; 12];
    let n = fs::read(&format!("{conv}/from"), 0, &mut b).unwrap_or(0);
    core::str::from_utf8(&b[..n]).ok()?.trim().parse().ok()
}

/// Write `msg`'s control buffer: the sender's credentials when `from`
/// (every process is uid and gid 0 to a Linux program), then `fds` as one
/// `SCM_RIGHTS` message, closing those it has no room for. The bytes used
/// and whether anything was left out.
fn put_control(msg: usize, from: Option<u32>, fds: &[usize]) -> Result<(usize, bool), usize> {
    if fds.is_empty() && from.is_none() {
        return Ok((0, false));
    }
    let mut b = [0u8; 16];
    get_bytes(msg + 32, &mut b)?;
    let (control, len) = (u64::from_le_bytes(b[..8].try_into().unwrap()) as usize, u64::from_le_bytes(b[8..].try_into().unwrap()) as usize);
    let len = if control == 0 { 0 } else { len };
    let mut out = Vec::new();
    let mut truncated = false;
    let cmsg = |out: &mut Vec<u8>, ty: u32, data: &[u8]| {
        out.resize(out.len().next_multiple_of(8), 0);
        out.extend_from_slice(&((CMSG_HDR + data.len()) as u64).to_le_bytes());
        out.extend_from_slice(&(SOL_SOCKET as u32).to_le_bytes());
        out.extend_from_slice(&ty.to_le_bytes());
        out.extend_from_slice(data);
    };
    if let Some(pid) = from {
        if len >= CMSG_HDR + 12 {
            let mut cred = [0u8; 12];
            cred[..4].copy_from_slice(&pid.to_le_bytes());
            cmsg(&mut out, SCM_CREDENTIALS, &cred);
        } else {
            truncated = true;
        }
    }
    let start = out.len().next_multiple_of(8);
    let room = len.saturating_sub(start + CMSG_HDR) / 4;
    let kept = fds.len().min(room);
    for &fd in &fds[kept..] {
        super::sys::close(fd).ok();
    }
    truncated |= kept < fds.len();
    if kept > 0 {
        let data: Vec<u8> = fds[..kept].iter().flat_map(|&fd| (fd as i32).to_le_bytes()).collect();
        cmsg(&mut out, SCM_RIGHTS, &data);
    }
    if out.is_empty() {
        return Ok((0, truncated));
    }
    put(control, &out)?;
    Ok((out.len().next_multiple_of(8).min(len), truncated))
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

/// A unix socket's own name; a UDP socket's local address, netd's; a TCP
/// one's is not known: any address, port 0.
pub fn getsockname(fd: usize, addr: usize, alen: usize) -> R {
    let (conv, s) = sock(fd)?;
    if let Some(u) = s.unix {
        return put_name(addr, alen, &u.name);
    }
    let any = if s.inet6 { Addr::ANY6 } else { Addr::ANY4 };
    let local = if s.stream { None } else { udp_local(&conv) };
    put_addr(addr, alen, local.unwrap_or(any), s.inet6)
}

/// The name a unix socket connected to (unnamed: an accepted or paired
/// one's peer); an `AF_INET` socket's peer address.
pub fn getpeername(fd: usize, addr: usize, alen: usize) -> R {
    let (conv, s) = sock(fd)?;
    match s.unix {
        Some(u) if state(&conv) == State::Connected => put_name(addr, alen, &u.peer),
        Some(_) => Err(ENOTCONN),
        None => put_addr(addr, alen, s.peer.ok_or(ENOTCONN)?, s.inet6),
    }
}

/// `getsockopt`: the type, a failed connection's error, and the buffer
/// sizes: 64 KiB each way for a unix socket (`docs/sockets-unix.md`), the
/// 8 KiB netd queues per TCP conversation otherwise. Firefox's IPC
/// sizes its writes by `SO_SNDBUF` and aborts on 0. The peer's label,
/// groups and pidfd are not there (`ENOPROTOOPT`); anything else reads 0.
pub fn getsockopt(fd: usize, level: usize, opt: usize, val: usize, len: usize) -> R {
    let (conv, s) = sock(fd)?;
    // The peer's credentials (`struct ucred`: pid, uid, gid): the layer
    // does not know the other end, so the caller's own. Every process is
    // uid and gid 0 to a Linux program, so only the pid can be off.
    if (level, opt) == (SOL_SOCKET, SO_PEERCRED) {
        let mut cred = [0u8; 12];
        cred[..4].copy_from_slice(&(task::current_pid() as u32).to_le_bytes());
        put(val, &cred)?;
        put(len, &12u32.to_le_bytes())?;
        return Ok(0);
    }
    let v: i32 = match (level, opt) {
        (SOL_SOCKET, SO_PEERSEC | SO_PEERGROUPS | SO_PEERPIDFD) => return Err(ENOPROTOOPT),
        (SOL_SOCKET, SO_TYPE) if s.records => SOCK_SEQPACKET as i32,
        (SOL_SOCKET, SO_TYPE) => (if s.stream { SOCK_STREAM } else { SOCK_DGRAM }) as i32,
        (SOL_SOCKET, SO_ERROR) if state(&conv) == State::Failed => ECONNREFUSED as i32,
        (SOL_SOCKET, SO_SNDBUF | SO_RCVBUF) => if s.unix.is_some() { 64 << 10 } else { 8 << 10 },
        (SOL_SOCKET, SO_PASSCRED) => s.passcred as i32,
        _ => 0,
    };
    put(val, &v.to_le_bytes())?;
    put(len, &4u32.to_le_bytes())?;
    Ok(0)
}

/// `setsockopt`: `SO_PASSCRED` on a unix socket; anything else is
/// accepted and ignored (options do not apply).
pub fn setsockopt(fd: usize, level: usize, opt: usize, val: usize, len: usize) -> R {
    let (_, s) = sock(fd)?;
    if (level, opt) == (SOL_SOCKET, SO_PASSCRED) && s.unix.is_some() {
        let mut b = [0u8; 4];
        if len < 4 {
            return Err(EINVAL);
        }
        get_bytes(val, &mut b)?;
        files::with_sock(fd, |s| s.passcred = i32::from_le_bytes(b) != 0);
    }
    Ok(0)
}

/// `bind`: a unix socket takes the name its `listen` announces; an
/// `AF_INET` or `AF_INET6` one ignores it (a client socket binds
/// implicitly).
pub fn bind(fd: usize, addr: usize, len: usize) -> R {
    let Some(u) = sock(fd)?.1.unix else {
        return Ok(0);
    };
    let name = get_name(addr, len)?;
    files::with_sock(fd, |s| s.unix = Some(UnixNames { name, ..u }));
    Ok(0)
}

/// `listen`: a bound unix socket announces its name in `/net/unix`. No
/// `AF_INET` or `AF_INET6` listeners.
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
    let new = open_conv("unix", &id, flags, Sock { unix: Some(names), records: s.records, ..UNIX_SOCK })?;
    put_name(addr, alen, &Name::NONE)?;
    Ok(new)
}

/// `socketpair(AF_UNIX, ...)`: a `/net/unix` conversation and the one its
/// `pair` connects to it (taken from its `listen`), both kept as sockets
/// like TCP's; with `SOCK_SEQPACKET`, both keep records.
pub fn socketpair(domain: usize, ty: usize, sv: usize) -> R {
    if domain != AF_UNIX {
        return Err(EAFNOSUPPORT);
    }
    let a = new_unix(ty)?;
    let (conv, s) = sock(a)?;
    let b = fs::write(&format!("{conv}/ctl"), 0, b"pair")
        .ok_or(EMFILE)
        .and_then(|_| conv_id(&format!("{conv}/listen")))
        .and_then(|id| open_conv("unix", &id, ty, Sock { records: s.records, ..UNIX_SOCK }));
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
