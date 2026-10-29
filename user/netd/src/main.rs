#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![no_main]

extern crate alloc;

mod lo;
mod nics;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write;

use myos_net::smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use myos_net::smoltcp::socket::{dhcpv4, icmp, tcp, udp};
use myos_net::smoltcp::time::{Duration, Instant};
use myos_net::smoltcp::phy::ChecksumCapabilities;
use myos_net::smoltcp::wire::{
    HardwareAddress, Icmpv4Packet, Icmpv4Repr, Icmpv6Packet, Icmpv6Repr, IpAddress, IpCidr, IpEndpoint,
    IpListenEndpoint, IpVersion, Ipv4Address, Ipv6Address,
};
use myos_net::{device_capabilities, Net0Device, VirtualInstant};

use lo::LoopDevice;
use nics::Nics;
use myos_user::{close, heap_init, open_flags, poll, read, write, write_fd, Heap, PollFd, O_RDWR, POLLIN};

#[global_allocator]
static GLOBAL: Heap = Heap;

const PROTO_TCP: u8 = 1;
const PROTO_UDP: u8 = 2;
const PROTO_ICMP: u8 = 3;

const REQ_CLONE: u8 = 1;
const REQ_CTL: u8 = 2;
const REQ_SEND: u8 = 3;
const REQ_CLOSE: u8 = 4;
/// netfs returns drained receive space (u16 payload); see `Conv::rx_room`.
const REQ_CREDIT: u8 = 5;

const REP_CLONE_OK: u8 = 1;
const REP_DATA: u8 = 2;
const REP_STATUS: u8 = 3;
const REP_ERR: u8 = 4;
/// Bytes of a TCP conv's queued sends that went into its smoltcp socket
/// (u16 payload): netfs lets the writers send that many more (its TX_CAP
/// bounds what waits here).
const REP_TXCREDIT: u8 = 5;
/// The DHCP lease as `/net/ndb` text (empty: none), see [`ndb_text`].
const REP_NDB: u8 = 6;
/// An error for a UDP conv's next operation (`refused`): netfs keeps it
/// until the socket library reads it (ctl) or a read or write reports it.
const REP_SOERR: u8 = 7;
/// The interfaces as `/net/ifaddrs` text, see [`ifaddrs_text`].
const REP_IFADDRS: u8 = 8;
/// A UDP conv's new peer (its address as in [`UDP_HDR`], its port
/// big-endian): netfs drops the datagrams it queued from anyone else, as
/// later ones are here.
const REP_PEER: u8 = 9;
/// netd's conversation table as `/net/convs` text, see [`convs_text`].
const REP_CONVS: u8 = 10;

/// The per-datagram header on a UDP conv's REQ_SEND and REP_DATA (Plan 9's
/// udp "headers" without its interface address): remote address, local
/// address (16 bytes each, an IPv4 one as `::ffff:a.b.c.d`, all zeros for
/// none), remote port, local port, the ports big-endian. netfs shows it to
/// readers and takes it from writers of a conv in "headers" mode, and adds
/// a zero one (the connected peer) for the others.
const UDP_HDR: usize = 36;

const REQ_HDR: usize = 6;
const REP_HDR: usize = 9;
/// A request from netfs (its ring's slot), and a reply to it: a data reply
/// carries up to `REP_CAP - REP_HDR` of a TCP stream at once, so a burst
/// crosses to netfs in a few writes instead of one per MSS. A reply must
/// reach netfs in one piece: no more than the kernel's write chunk
/// (`FILE_IO_TMP` in task/fd.rs).
const MSG_CAP: usize = 2048;
const REP_CAP: usize = 8192;
/// Conversations at once (netfs has the same number of slots). 32 was too
/// few for a server on the internet: a scanner's connection every few
/// seconds, each held by nginx or dropbear for its request timeout and
/// then for the peer's half of the close, kept the table at the brim and
/// every other new connection was reset at the accept.
const MAX_CONV: usize = 128;
const FILE_IO: usize = 2048;
const DHCP_POLLS: usize = 3000;
/// Minimum clock advance per poll (when not a whole millisecond went by); the
/// stack clock otherwise follows the monotonic clock (`VirtualInstant::bump`).
const TICK_MS: u64 = 1;
/// How long a conversation its process closed may take to finish closing
/// before netd aborts it: a peer that never answers the FIN (a scanner
/// behind a firewall, a host that is gone) left the socket retransmitting
/// in FinWait1 or LastAck, or waiting in FinWait2 for a FIN that never
/// comes, and the index with it, for good. netfs had freed its slot at the
/// close's acknowledgement, so nothing showed them; on a scanned VPS they
/// filled the table within the hour, and every new connection was aborted
/// at the accept. Linux gives an orphaned connection about as long
/// (tcp_fin_timeout).
const CLOSE_GRACE: Duration = Duration::from_secs(60);
/// How long an incoming connection may take to complete its handshake.
/// netd answers a SYN with its SYN-ACK and keeps the half-open connection
/// to itself until the peer's ACK makes it a conversation; an ACK that
/// never comes (a scanner probing the port, a SYN flood with forged
/// sources: a public host sees them all day) ends it here, the
/// retransmitted SYN-ACKs unanswered. Linux gives up after about a minute
/// (tcp_synack_retries); a client that is there answers within a round
/// trip.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
/// Half-open connections kept at a time, with the completed ones waiting
/// for room in their listener's queue, every listener together (each
/// holds a socket's buffers, 96 KiB); past this a SYN is answered with a
/// reset, as Linux drops a SYN when its queue is full.
const SYN_BACKLOG: usize = 64;
/// Completed connections one listener may have waiting for its queue
/// ([`queue_full`]): half the backlog, so that a server that stopped
/// taking its connections leaves the other ports their handshakes. Past
/// this a completed connection is reset, as it always was past two.
const ACCEPT_WAIT: usize = SYN_BACKLOG / 2;
/// Closed connections kept in TIME-WAIT at a time. The side that closed
/// first keeps its socket, buffers and all (96 KiB), for smoltcp's 10 s of
/// TIME-WAIT; a burst of short connections (a test's dozens over loopback,
/// where netd is both ends and every connection leaves one; a busy server
/// that closes first) piled up more than netd's heap holds, and netd died
/// of the failed allocation. Past this the oldest go at once, as Linux
/// skips TIME-WAIT past tcp_max_tw_buckets.
const MAX_TIME_WAIT: usize = 16;
/// Idle sleeps between polls when the NIC has no RX interrupt (poll-mode
/// fallback): a bounded sleep sets the worst-case RX latency; kernel events
/// (channel requests from netfs, pipe/console traffic) end the sleep early.
const IDLE_SLEEP_ACTIVE_NS: u64 = 2_000_000;
const IDLE_SLEEP_QUIET_NS: u64 = 10_000_000;
/// With RX interrupts the idle wait only needs a backstop (smoltcp's own
/// `poll_delay` cuts it shorter when a timer is due).
const IDLE_WAIT_IRQ_NS: u64 = 1_000_000_000;

const ICMP_IDENT_BASE: u16 = 0x22b;
/// A TCP socket's receive and send buffers: the receive one is the window
/// offered to the peer (4 KiB of it capped a download at a few segments per
/// round trip, 1-2 MB/s under QEMU), the send one what a writer may have
/// in flight.
const TCP_RX: usize = 65536;
const TCP_TX: usize = 32768;
/// netfs's per-conv receive buffer (modules/netfs DATA_CAP). It drops what
/// does not fit, so netd never has more than this in flight to it.
const NETFS_RX_CAP: u16 = 32768;
/// A UDP socket's receive and send buffers: a few datagrams up to what one
/// netfs message carries.
const UDP_BUF: usize = 4096;
const UDP_PACKETS: usize = 4;
/// Ephemeral local ports for outbound TCP/UDP (avoid sticky 49152+conv reuse).
const LOCAL_PORT_BASE: u16 = 49152;

fn next_local_port(counter: &mut u16) -> u16 {
    let p = *counter;
    let next = p.wrapping_add(1);
    *counter = if next < LOCAL_PORT_BASE {
        LOCAL_PORT_BASE
    } else {
        next
    };
    p
}

#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    main()
}

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_argc: usize, _argv: *const usize) -> ! {
    main()
}

#[derive(Clone, Copy)]
enum Kind {
    Empty,
    Icmp,
    Udp,
    Tcp,
}

struct Conv {
    kind: Kind,
    handle: Option<SocketHandle>,
    remote: IpAddress,
    remote_port: u16,
    have_remote: bool,
    ident: u16,
    seq: u16,
    connected: bool,
    /// Peer closed (or socket inactive); status hangup already sent once.
    hungup: bool,
    /// TCP bytes written but not yet in the smoltcp socket, in order: the
    /// socket's send buffer was full, or the handshake not done. netfs has
    /// already reported them written; REP_TXCREDIT gives back the room as
    /// they move on, so this stays within netfs's TX_CAP (8 KiB).
    pending: Vec<u8>,
    /// TCP listener (Plan 9 "announce"): nonzero = advertised port.
    listen_port: u16,
    /// A ctl "accept" arrived and is parked until a connection lands.
    accept_wait: bool,
    /// Head of the parked accept queue (next ctl "accept" / status read).
    accepted: Option<u16>,
    /// Second parked accept. Prefer queueing over holding a SYN on the listen
    /// handle (which leaves no Listen socket). Always re-arm Listen when the
    /// queue has room. Do not take from accept_from_status — that raced KEX.
    accepted_pending: Option<u16>,
    /// Room left in netfs's receive buffer for this conv: TCP data is only
    /// taken from smoltcp (and sent as REP_DATA) up to this; REQ_CREDIT adds
    /// back what the reader drained. While it is 0 the data waits in the
    /// smoltcp socket and the TCP window throttles the peer.
    rx_room: u16,
    /// Monotonic per-listener handoff counter. Letting libgloss tell a fresh
    /// "accepted <N> <seq>" from a stale one (the status file keeps the last
    /// accepted string until netd replies again) prevents the app's blocking
    /// accept() from returning the same connection several times.
    accept_seq: u32,
    /// Graceful close in flight: close() sent, socket removed once Closed.
    closing: bool,
    /// close() requested while the handshake was still completing and
    /// pending TX could not be flushed yet: pump flushes pending first,
    /// then finishes the close. Dropping the conv here would lose the
    /// deferred REQ_SEND bytes (forked-child banner write race).
    closing_after_flush: bool,
    /// When `closing` or `closing_after_flush` was first seen set: the
    /// close is aborted CLOSE_GRACE later if the peer has not let it end.
    closing_since: Option<Instant>,
    /// Created by pump_accepts (incoming). Orphan reclaim must only touch
    /// these: client clones start in TCP Closed and would be freed before
    /// connect() (aarch64 socket_smoke connect fail on tip 0b4444e).
    from_accept: bool,
    /// A process holds the conversation's files (a client clone, or an
    /// accepted connection the library took): the index is that process's
    /// until its REQ_CLOSE, whatever the socket does meanwhile. Freeing it
    /// at the peer's hangup let pump_accepts hand the same index to the
    /// next connection while the old holder still read and wrote it: an
    /// HTTP server's keepalive connection answered an SSH client's banner.
    held: bool,
    /// REQ_CLOSE arrived: the holder let go.
    released: bool,
    /// UDP: the local address (unspecified: any of its version, see
    /// `v6only`) and port (0: not bound yet; the first connect or send
    /// picks one). A TCP listener: the address it listens on, its port
    /// `listen_port`.
    local: IpAddress,
    local_port: u16,
    /// On `::`, IPv6 only (`IPV6_V6ONLY`); otherwise IPv4 too, as on Linux.
    /// On `0.0.0.0`, IPv4 only.
    v6only: bool,
    /// UDP: an IPv6 socket's (bound or autobound on an IPv6 address): its
    /// any address, which a disconnect goes back to, is `::`.
    inet6: bool,
    /// UDP: a `bind` named that address / port; otherwise connect chose
    /// the address and an unconnect gives both up, as on Linux.
    addr_bound: bool,
    port_bound: bool,
    /// UDP: bound with SO_REUSEADDR (shares the port with others that were).
    reuse: bool,
}

impl Conv {
    const EMPTY: Self = Self {
        kind: Kind::Empty,
        handle: None,
        remote: ANY4,
        remote_port: 0,
        have_remote: false,
        ident: 0,
        seq: 0,
        connected: false,
        hungup: false,
        pending: Vec::new(),
        listen_port: 0,
        accept_wait: false,
        accepted: None,
        accepted_pending: None,
        accept_seq: 0,
        rx_room: NETFS_RX_CAP,
        closing: false,
        closing_after_flush: false,
        closing_since: None,
        from_accept: false,
        held: false,
        released: false,
        local: ANY4,
        local_port: 0,
        v6only: false,
        inet6: false,
        addr_bound: false,
        port_bound: false,
        reuse: false,
    };
}

/// An incoming connection whose handshake is not done: its socket left
/// the listener's Listen state on a SYN. It becomes a conversation (an
/// index, netfs told, the listener's status naming it) once Established,
/// as accept on Linux only returns a completed handshake, and waits here
/// while two of its listener's connections are still to be taken (its
/// queue, [`queue_full`]), as on Linux's accept queue. Handed out at
/// SynReceived, as before, each handshake that never completed pinned an
/// index and a server process (dropbear forks per connection) for the
/// server's own request timeout: a scanned VPS held forty at a time.
#[derive(Clone, Copy)]
struct Handshake {
    handle: SocketHandle,
    /// The listener's conversation index.
    listener: u16,
    since: Instant,
}

/// A TCP connection its holder closed (REQ_CLOSE) that has still to end:
/// queued data and the FIN to send, the peer's FIN to wait for. netfs frees
/// the index at that close's "hangup", so the socket lingers here, nobody's,
/// until Closed or CLOSE_GRACE. It used to stay at the index as a closing
/// conversation, and the next clone of the index overwrote it: the socket
/// and its buffers stayed in the SocketSet for good, and netd's heap ran out
/// after a few dozen connections (issue #483).
struct Lingering {
    handle: SocketHandle,
    /// Data written before the close that the socket had no room for yet
    /// (`Conv::pending`); the FIN follows it.
    pending: Vec<u8>,
    /// The conversation's `closing_since`.
    since: Option<Instant>,
}

const ANY4: IpAddress = IpAddress::Ipv4(Ipv4Address::UNSPECIFIED);
const ANY6: IpAddress = IpAddress::Ipv6(Ipv6Address::UNSPECIFIED);

/// 127.0.0.1 or ::1.
fn localhost(v: IpVersion) -> IpAddress {
    match v {
        IpVersion::Ipv4 => IpAddress::Ipv4(Ipv4Address::LOCALHOST),
        IpVersion::Ipv6 => IpAddress::Ipv6(Ipv6Address::LOCALHOST),
    }
}

/// 127.0.0.0/8 or ::1.
fn is_loopback(a: IpAddress) -> bool {
    match a {
        IpAddress::Ipv4(a) => a.is_loopback(),
        IpAddress::Ipv6(a) => a.is_loopback(),
    }
}

/// An unspecified address is this host, as on Linux: the loopback address
/// of its version.
fn or_localhost(a: IpAddress) -> IpAddress {
    if a.is_unspecified() { localhost(a.version()) } else { a }
}

/// `a.b.c.d` or an IPv6 address in its text form.
fn parse_addr(s: &[u8]) -> Option<IpAddress> {
    match core::str::from_utf8(s).ok()?.parse::<core::net::IpAddr>().ok()? {
        core::net::IpAddr::V4(a) => Some(IpAddress::Ipv4(a)),
        core::net::IpAddr::V6(a) => Some(IpAddress::Ipv6(a)),
    }
}

/// An address as in [`UDP_HDR`]: an IPv4 one as `::ffff:a.b.c.d`.
fn addr16(a: IpAddress) -> [u8; 16] {
    match a {
        IpAddress::Ipv4(a) => a.to_ipv6_mapped().octets(),
        IpAddress::Ipv6(a) => a.octets(),
    }
}

/// The address of [`UDP_HDR`]'s 16 bytes `b`: `None` for all zeros.
fn from_addr16(b: &[u8]) -> Option<IpAddress> {
    let a = Ipv6Address::from(<[u8; 16]>::try_from(b).ok()?);
    if a.is_unspecified() {
        return None;
    }
    Some(match a.to_ipv4_mapped() {
        Some(v4) => IpAddress::Ipv4(v4),
        None => IpAddress::Ipv6(a),
    })
}

/// The IP version a conv on address `local` takes (`None`: either): its
/// own when it names one; on `0.0.0.0` IPv4, on `::` IPv6 and, unless
/// `v6only`, IPv4 too, as on Linux.
fn version_of(local: IpAddress, v6only: bool) -> Option<IpVersion> {
    match local {
        IpAddress::Ipv6(a) if a.is_unspecified() && !v6only => None,
        a => Some(a.version()),
    }
}

/// Whether convs on `a` (`a6only`) and on `b` (`b6only`) would both take
/// some packet to a port they share: a version both take, and an address
/// both do (the same one, or either on any).
fn overlaps(a: IpAddress, a6only: bool, b: IpAddress, b6only: bool) -> bool {
    let takes = |l: IpAddress, only: bool, v: IpVersion| version_of(l, only).is_none_or(|x| x == v);
    [IpVersion::Ipv4, IpVersion::Ipv6].into_iter().any(|v| {
        takes(a, a6only, v) && takes(b, b6only, v) && (a.is_unspecified() || b.is_unspecified() || a == b)
    })
}

fn parse_port(s: &[u8]) -> Option<u16> {
    if s.is_empty() {
        return None;
    }
    let mut n = 0u32;
    for &b in s {
        if !b.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add((b - b'0') as u32)?;
        if n > 65535 {
            return None;
        }
    }
    Some(n as u16)
}

/// `connect 10.0.2.2`, `connect 10.0.2.2!80`, `connect fec0::2!80`
fn parse_connect(cmd: &[u8]) -> Option<(IpAddress, Option<u16>)> {
    let rest = trim(cmd.strip_prefix(b"connect")?);
    match rest.iter().rposition(|&b| b == b'!') {
        Some(n) => Some((parse_addr(&rest[..n])?, Some(parse_port(&rest[n + 1..])?))),
        None => Some((parse_addr(rest)?, None)),
    }
}

/// Status payload for an accepted connection:
/// "accepted <N> <ip>!<port> <seq>".
fn accept_reply(convs: &mut [Conv; MAX_CONV], n: u16, seq: u32) -> Vec<u8> {
    let mut out = String::new();
    let _ = write!(out, "accepted {n}");
    let c = &convs[n as usize];
    if c.have_remote {
        let _ = write!(out, " {}!{}", c.remote, c.remote_port);
    }
    let _ = write!(out, " {seq}");
    out.into_bytes()
}

fn trim(s: &[u8]) -> &[u8] {
    let mut t = s;
    while first_ws(t) {
        t = &t[1..];
    }
    while last_ws(t) {
        t = &t[..t.len() - 1];
    }
    t
}

fn first_ws(s: &[u8]) -> bool {
    matches!(s.first(), Some(b' ' | b'\t' | b'\n' | b'\r'))
}

fn last_ws(s: &[u8]) -> bool {
    matches!(s.last(), Some(b' ' | b'\t' | b'\n' | b'\r'))
}

fn encode_rep(typ: u8, conv: u16, status: i32, payload: &[u8], out: &mut [u8]) -> Option<usize> {
    let n = REP_HDR + payload.len();
    if n > out.len() {
        return None;
    }
    out[0] = typ;
    out[1..3].copy_from_slice(&conv.to_le_bytes());
    out[3..7].copy_from_slice(&status.to_le_bytes());
    out[7..9].copy_from_slice(&(payload.len() as u16).to_le_bytes());
    if !payload.is_empty() {
        out[9..n].copy_from_slice(payload);
    }
    Some(n)
}

fn reply(fd: usize, typ: u8, conv: u16, status: i32, payload: &[u8]) {
    let mut tmp = [0u8; REP_CAP];
    let Some(n) = encode_rep(typ, conv, status, payload, &mut tmp) else {
        return;
    };
    let _ = write_fd(fd, &tmp[..n]);
}

fn open_chan() -> Option<usize> {
    open_flags(b"/dev/netd/data", O_RDWR)
}

/// netfs's channel, and the wired NIC when the board has one. Without
/// `/dev/net0` netd starts all the same: loopback works, and a request to
/// another address is refused at once instead of waiting on a netd that
/// never came (issue #538); the NIC is taken if it appears (`Nics::rescan`).
fn wait_devices() -> (Option<Net0Device>, usize) {
    let mut printed = false;
    loop {
        if let Some(fd) = open_chan() {
            let dev = Net0Device::open();
            if dev.is_none() {
                write(b"netd: no /dev/net0/data, loopback only\n");
            }
            return (dev, fd);
        }
        if !printed {
            write(b"netd: no /dev/netd/data\n");
            printed = true;
        }
        myos_user::sleep_ns(10_000_000, false);
    }
}

/// Wait for net0's lease at start (a bounded time: without one netd
/// starts anyway, and a lease that comes later is taken in the main loop).
fn poll_dhcp(
    nics: &mut Nics,
    lo: &mut Interface,
    sockets: &mut SocketSet<'_>,
    chan: usize,
    clock: &mut VirtualInstant,
) {
    for _ in 0..DHCP_POLLS {
        clock.bump(TICK_MS);
        let now = clock.now();
        let rx_before = nics.rx_frames();
        nics.poll(now, sockets);
        if nics.rx_frames() == rx_before {
            // Nothing arrived: wait a little instead of spinning on the NIC.
            myos_user::sleep_ns(1_000_000, false);
        }
        nics.slaac(now, lo, chan);
        if nics.dhcp(sockets, lo, chan) {
            return;
        }
    }
}

/// The loopback interface (`lo`): smoltcp's `local-pair` patch has it carry
/// everything to 127.0.0.0/8, ::1 and the host's own addresses, which the
/// NIC's interface then leaves alone.
fn build_loopback(dev: &mut LoopDevice, now: Instant) -> Interface {
    let mut config = Config::new(HardwareAddress::Ip);
    config.random_seed = 0x6c6f;
    let mut lo = Interface::new(config, dev, now);
    lo.set_local_only(true);
    // Every NIC's addresses join these with its lease (`Nics`).
    lo.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::new(IpAddress::v4(127, 0, 0, 1), 8));
        let _ = addrs.push(IpCidr::new(IpAddress::Ipv6(Ipv6Address::LOCALHOST), 128));
    });
    lo
}

/// netd's conversation table as `/net/convs` text, a line per index in
/// use: `N tcp|udp|icmp [STATE] flags`, STATE a TCP socket's (`-`: none, a
/// tombstone), then `listen a.b.c.d!p` (a listener on an address, IPv4 or
/// IPv6, `*` for any IPv4 one, `[::]` for any IPv6 one and IPv4 too,
/// `[::]6` for IPv6 only), `peer a.b.c.d!p`, `accept` (netd accepted it), `held` (a process has
/// its files), `released` (that process closed them), `connected`,
/// `hungup`, `closing Ns` (closing for N seconds). netfs's view
/// (`/net/tcp/N/status`) ends at the close's acknowledgement, netd's at the
/// socket's end: this is where the indices went when the table is full.
/// Then a line per handshake not done, `- tcp STATE peer a.b.c.d!p port P
/// handshake Ns`: not a conversation yet, nobody's.
fn convs_text(
    convs: &[Conv; MAX_CONV],
    sockets: &SocketSet<'_>,
    handshakes: &[Handshake],
    lingering: &[Lingering],
    now: Instant,
) -> String {
    let mut s = String::new();
    for (i, c) in convs.iter().enumerate() {
        let kind = match c.kind {
            Kind::Empty => continue,
            Kind::Tcp => "tcp",
            Kind::Udp => "udp",
            Kind::Icmp => "icmp",
        };
        let _ = write!(s, "{i} {kind}");
        if matches!(c.kind, Kind::Tcp) {
            match c.handle {
                Some(h) => {
                    let _ = write!(s, " {:?}", sockets.get::<tcp::Socket>(h).state());
                }
                None => s.push_str(" -"),
            }
        }
        if c.listen_port != 0 {
            match c.local {
                IpAddress::Ipv4(a) if a.is_unspecified() => {
                    let _ = write!(s, " listen *!{}", c.listen_port);
                }
                IpAddress::Ipv6(a) if a.is_unspecified() => {
                    let only = if c.v6only { "6" } else { "" };
                    let _ = write!(s, " listen [::]{only}!{}", c.listen_port);
                }
                a => {
                    let _ = write!(s, " listen {a}!{}", c.listen_port);
                }
            }
        }
        if c.have_remote {
            let _ = write!(s, " peer {}!{}", c.remote, c.remote_port);
        }
        for (flag, on) in [
            ("accept", c.from_accept),
            ("held", c.held),
            ("released", c.released),
            ("connected", c.connected),
            ("hungup", c.hungup),
        ] {
            if on {
                s.push(' ');
                s.push_str(flag);
            }
        }
        if c.closing || c.closing_after_flush {
            match c.closing_since {
                Some(t) => {
                    let _ = write!(s, " closing {}s", (now - t).secs());
                }
                None => s.push_str(" closing"),
            }
        }
        if matches!(c.kind, Kind::Tcp) && c.listen_port == 0 {
            // Where the bytes wait: sent ones not yet in the socket (`tx`)
            // and in it, not yet acknowledged (`sendq`); received ones in
            // the socket, netfs's room for them (`recvq`, `rxroom`).
            let _ = write!(s, " tx {} rxroom {}", c.pending.len(), c.rx_room);
            if let Some(h) = c.handle {
                let sock = sockets.get::<tcp::Socket>(h);
                if let Some(IpEndpoint { addr, port }) = sock.local_endpoint() {
                    let _ = write!(s, " local {addr}!{port}");
                }
                let _ = write!(s, " sendq {} recvq {}", sock.send_queue(), sock.recv_queue());
            }
        }
        s.push('\n');
    }
    for hs in handshakes {
        let sock = sockets.get::<tcp::Socket>(hs.handle);
        let _ = write!(s, "- tcp {:?}", sock.state());
        if let Some(IpEndpoint { addr, port }) = sock.remote_endpoint() {
            let _ = write!(s, " peer {addr}!{port}");
        }
        let l = hs.listener as usize;
        let _ = writeln!(s, " port {} handshake {}s", convs[l].listen_port, (now - hs.since).secs());
    }
    for l in lingering {
        let sock = sockets.get::<tcp::Socket>(l.handle);
        let _ = write!(s, "- tcp {:?}", sock.state());
        if let Some(IpEndpoint { addr, port }) = sock.remote_endpoint() {
            let _ = write!(s, " peer {addr}!{port}");
        }
        let secs = l.since.map_or(0, |t| (now - t).secs());
        let _ = writeln!(s, " closing {secs}s tx {} sendq {}", l.pending.len(), sock.send_queue());
    }
    s
}

/// An address of this host's: 127.0.0.0/8, ::1, a NIC's, and 0.0.0.0 or
/// :: (which a connect or send means as the loopback address, as on
/// Linux).
fn is_local(nics: &Nics, a: IpAddress) -> bool {
    is_loopback(a) || a.is_unspecified() || nics.has_ip_addr(a)
}

/// Where TCP listener conv `c` listens: its port, on the address its
/// announce named (any when that was unspecified).
fn listen_endpoint(c: &Conv) -> IpListenEndpoint {
    IpListenEndpoint {
        addr: (!c.local.is_unspecified()).then_some(c.local),
        port: c.listen_port,
    }
}

/// Make `s` TCP listener conv `c`'s: on its endpoint, for the IP version
/// it takes (`version_of`).
fn listen_on(s: &mut tcp::Socket, c: &Conv) -> Result<(), tcp::ListenError> {
    s.set_listen_version(version_of(c.local, c.v6only));
    s.listen(listen_endpoint(c))
}

fn handle_clone(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    lingering: &mut Vec<Lingering>,
    proto: u8,
    conv: u16,
) {
    let i = conv as usize;
    if i >= MAX_CONV {
        return;
    }
    release_conv(convs, sockets, lingering, i);
    match proto {
        PROTO_ICMP => {
            // Room for two: to a local address the socket also takes its
            // own echo request (its ident matches), then the reply.
            let rx = icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY; 2], vec![0; 512]);
            let tx = icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY], vec![0; 256]);
            let handle = sockets.add(icmp::Socket::new(rx, tx));
            convs[i] = Conv {
                kind: Kind::Icmp,
                handle: Some(handle),
                ident: ICMP_IDENT_BASE.wrapping_add(conv),
                ..Conv::EMPTY
            };
            convs[i].kind = Kind::Icmp;
            convs[i].handle = Some(handle);
        }
        PROTO_UDP => {
            let meta = || vec![udp::PacketMetadata::EMPTY; UDP_PACKETS];
            let rx = udp::PacketBuffer::new(meta(), vec![0; UDP_BUF]);
            let tx = udp::PacketBuffer::new(meta(), vec![0; UDP_BUF]);
            let handle = sockets.add(udp::Socket::new(rx, tx));
            convs[i] = Conv {
                kind: Kind::Udp,
                handle: Some(handle),
                ..Conv::EMPTY
            };
        }
        PROTO_TCP => {
            let rx = tcp::SocketBuffer::new(vec![0; TCP_RX]);
            let tx = tcp::SocketBuffer::new(vec![0; TCP_TX]);
            let handle = sockets.add(tcp::Socket::new(rx, tx));
            convs[i] = Conv {
                kind: Kind::Tcp,
                handle: Some(handle),
                ..Conv::EMPTY
            };
        }
        _ => {}
    }
    // The process that opened `clone` holds the conversation.
    convs[i].held = true;
}



/// Install `head` as listener `l`'s advertised handoff and publish it.
///
/// Every head change bumps `accept_seq` and replies with the new status, so
/// libgloss (which only accepts a seq it has not consumed yet) can never miss
/// a parked connection: a head promoted by `taken`, by orphan reclaim or by the
/// stale-pointer cleanup used to be installed silently, and with no fresh
/// "accepted <N> <seq>" the listener looked idle while clients hung (riscv64
/// SSH smoke: 5 minutes without a `Child connection`).
fn publish_head(convs: &mut [Conv; MAX_CONV], chan: usize, l: usize, head: Option<u16>) {
    convs[l].accepted = head;
    match head {
        Some(n) => {
            convs[l].accept_seq = convs[l].accept_seq.wrapping_add(1);
            convs[l].accept_wait = false;
            let seq = convs[l].accept_seq;
            let rep = accept_reply(convs, n, seq);
            reply(chan, REP_STATUS, l as u16, 0, &rep);
        }
        None => reply(chan, REP_STATUS, l as u16, 0, b"listening"),
    }
}

/// Parse the optional "<seq>" argument of ctl "taken <seq>".
fn parse_seq(b: &[u8]) -> Option<u32> {
    if b.is_empty() {
        return None;
    }
    let mut v: u32 = 0;
    for &c in b {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v.wrapping_mul(10).wrapping_add((c - b'0') as u32);
    }
    Some(v)
}

/// Move what fits of `c`'s queued sends into its socket, oldest first.
/// Returns how many bytes moved (room to hand back to netfs).
fn flush_pending(c: &mut Conv, s: &mut tcp::Socket) -> usize {
    if c.pending.is_empty() || !s.can_send() {
        return 0;
    }
    let n = s.send_slice(&c.pending).unwrap_or(0);
    c.pending.drain(..n);
    n
}

/// Give netfs back `n` bytes of a TCP conv's send room.
fn tx_credit(chan: usize, conv: u16, mut n: usize) {
    while n > 0 {
        let m = n.min(u16::MAX as usize);
        reply(chan, REP_TXCREDIT, conv, 0, &(m as u16).to_le_bytes());
        n -= m;
    }
}

fn drop_conv(convs: &mut [Conv; MAX_CONV], sockets: &mut SocketSet<'_>, i: usize) {
    if i >= MAX_CONV {
        return;
    }
    // Listeners stay up across client hangups and forked closes: only their
    // last holder's REQ_CLOSE takes them down (close_listener).
    if matches!(convs[i].kind, Kind::Tcp) && convs[i].listen_port != 0 {
        return;
    }
    // Idempotent: a forked child's exit closes the inherited netfs fd AND the
    // parent's close() may arrive in the same request burst. A second
    // drop_conv on an already-closing conv used to hit the !live path and
    // sockets.remove() the socket, discarding the queued TX data + FIN.
    if convs[i].closing || convs[i].closing_after_flush {
        return;
    }
    if let Some(h) = convs[i].handle.take() {
        // Graceful close for live TCP sockets: close() drains queued TX and
        // sends FIN; sockets.remove() would discard pending data (the peer
        // sees a bare FIN/RST and loses bytes written just before close).
        let live = matches!(convs[i].kind, Kind::Tcp)
            && !matches!(sockets.get_mut::<tcp::Socket>(h).state(), tcp::State::Closed);
        if live {
            // A REQ_SEND may have been stashed while a connect was still
            // completing (the handshake not done). Flush what we can
            // now; if the socket still cannot send, defer the close until the
            // pump drains pending, then finish the close.
            flush_pending(&mut convs[i], sockets.get_mut::<tcp::Socket>(h));
            if !convs[i].pending.is_empty() {
                convs[i].closing_after_flush = true;
                convs[i].handle = Some(h);
                return;
            }
            sockets.get_mut::<tcp::Socket>(h).close();
            convs[i].handle = Some(h);
            convs[i].closing = true;
            return;
        }
        sockets.remove(h);
    }
    retire(convs, i);
}

/// The holder let go of conversation `i` (its REQ_CLOSE; a clone of the
/// index, netfs having freed it): a TCP socket that has still to end goes
/// on closing in `lingering` and the index is free.
fn release_conv(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    lingering: &mut Vec<Lingering>,
    i: usize,
) {
    drop_conv(convs, sockets, i);
    let c = &mut convs[i];
    if !c.closing && !c.closing_after_flush {
        return;
    }
    if let Some(h) = c.handle.take() {
        if c.closing_after_flush && c.pending.is_empty() {
            sockets.get_mut::<tcp::Socket>(h).close();
        }
        lingering.push(Lingering {
            handle: h,
            pending: core::mem::take(&mut c.pending),
            since: c.closing_since,
        });
    }
    *c = Conv::EMPTY;
}

/// Finish the lingering closes: the socket goes once Closed, or is aborted
/// CLOSE_GRACE after the close began, as Linux gives up on an orphaned
/// connection; past MAX_TIME_WAIT in TIME-WAIT, the oldest of those go.
/// What the peer still sends is read and dropped.
fn sweep_lingering(sockets: &mut SocketSet<'_>, lingering: &mut Vec<Lingering>, now: Instant) {
    lingering.retain_mut(|l| {
        let s = sockets.get_mut::<tcp::Socket>(l.handle);
        if !l.pending.is_empty() && s.can_send() {
            let n = s.send_slice(&l.pending).unwrap_or(0);
            l.pending.drain(..n);
            if l.pending.is_empty() {
                s.close();
            }
        }
        while s.can_recv() && s.recv(|b| (b.len(), ())).is_ok() {}
        let since = *l.since.get_or_insert(now);
        let done = s.state() == tcp::State::Closed;
        if !done && now - since < CLOSE_GRACE {
            return true;
        }
        s.abort();
        sockets.remove(l.handle);
        false
    });
    // Oldest first: `lingering` is in the order the closes began. The
    // socket goes without a reset; the peer has its ACK of the FIN.
    let time_wait = |sockets: &SocketSet<'_>, l: &Lingering| {
        sockets.get::<tcp::Socket>(l.handle).state() == tcp::State::TimeWait
    };
    let mut excess = lingering.iter().filter(|l| time_wait(sockets, l)).count().saturating_sub(MAX_TIME_WAIT);
    lingering.retain(|l| {
        if excess == 0 || !time_wait(sockets, l) {
            return true;
        }
        excess -= 1;
        sockets.remove(l.handle);
        false
    });
}

/// The last holder of a listener closed it (netfs reports that close only,
/// a forked child's does not reach netd): its socket goes, the index is
/// free, and the connections it had accepted that nobody took are
/// aborted, as a closed port's are. Listeners used to live on here, their
/// socket still taking the port's connections for a netfs slot that had
/// moved on: a restarted server shared its port with its own ghost, which
/// answered nothing (nginx after `-s quit`: curl's "Empty reply"), and a
/// ghost's status replies landed on whoever had its index since (a
/// listener's "accepted" overwritten with "listening": its connection
/// never accepted).
fn close_listener(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    handshakes: &mut Vec<Handshake>,
    chan: usize,
    i: usize,
) {
    handshakes.retain(|hs| {
        if hs.listener as usize != i {
            return true;
        }
        drop_handshake(sockets, hs.handle);
        false
    });
    let parked = [convs[i].accepted, convs[i].accepted_pending];
    if let Some(h) = convs[i].handle.take() {
        let s = sockets.get_mut::<tcp::Socket>(h);
        if !matches!(s.state(), tcp::State::Closed | tcp::State::Listen) {
            s.abort();
        }
        sockets.remove(h);
    }
    convs[i] = Conv::EMPTY;
    for n in parked.into_iter().flatten().map(usize::from) {
        if n < MAX_CONV && convs[n].from_accept && !convs[n].held {
            force_free_orphan(convs, sockets, chan, n);
        }
    }
}

/// Reset a connection that is nobody's (its handshake not done, or done
/// with no room for it) and forget its socket.
fn drop_handshake(sockets: &mut SocketSet<'_>, h: SocketHandle) {
    let s = sockets.get_mut::<tcp::Socket>(h);
    // A reset to its SYN-ACK sent the socket back to Listen (smoltcp):
    // nothing to abort, but it must not take the port's next SYN.
    if !matches!(s.state(), tcp::State::Closed | tcp::State::Listen) {
        s.abort();
    }
    sockets.remove(h);
}

/// The socket is gone: the index goes back to the pool, unless a process
/// still holds the conversation's files; then it stays as a tombstone
/// (hung up, no socket) until that REQ_CLOSE.
fn retire(convs: &mut [Conv; MAX_CONV], i: usize) {
    if convs[i].held && !convs[i].released {
        convs[i].handle = None;
        convs[i].hungup = true;
        convs[i].closing = true;
        convs[i].pending = Vec::new();
        return;
    }
    convs[i] = Conv::EMPTY;
}

fn handle_ctl(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    nics: &mut Nics,
    lo: &mut Interface,
    chan: usize,
    conv: u16,
    payload: &[u8],
    local_ports: &mut u16,
) {
    let i = conv as usize;
    if i >= MAX_CONV {
        reply(chan, REP_ERR, conv, -1, b"no conv");
        return;
    }
    let cmd = trim(payload);
    if cmd == b"hangup" {
        // A parked accept on a closing listener fails loudly.
        if convs[i].accept_wait {
            reply(chan, REP_ERR, conv, -1, b"hangup");
        }
        drop_conv(convs, sockets, i);
        reply(chan, REP_STATUS, conv, 0, b"hangup");
        return;
    }
    if matches!(convs[i].kind, Kind::Udp) {
        handle_udp_ctl(convs, sockets, nics, chan, i, cmd, local_ports);
        return;
    }
    // Plan 9 announce: start a TCP listener on this conv ("announce
    // [addr!]<port>[ v6only]": on that address, one of the host's, else on
    // any IPv4 one; `::` is any IPv6 one, and IPv4 too unless v6only).
    // Port 0 is a free one, which the answer names ("listening <port>"):
    // Linux's listen after a bind to port 0.
    if let Some(rest) = cmd.strip_prefix(b"announce") {
        let rest = trim(rest);
        let (rest, v6only) = match rest.strip_suffix(b"v6only") {
            Some(r) => (trim(r), true),
            None => (rest, false),
        };
        let (addr, mut port) = match parse_endpoint(rest) {
            Some((a, p)) => (a, p),
            _ => match parse_port(rest) {
                Some(p) => (ANY4, p),
                _ => {
                    reply(chan, REP_ERR, conv, -1, b"need port");
                    return;
                }
            },
        };
        if !is_local(nics, addr) {
            reply(chan, REP_ERR, conv, -1, b"addrnotavail");
            return;
        }
        match convs[i].kind {
            Kind::Tcp => {
                let Some(h) = convs[i].handle else {
                    reply(chan, REP_ERR, conv, -1, b"no conv");
                    return;
                };
                // A port another listener has, on this address or any, is
                // taken (Linux's EADDRINUSE, SO_REUSEADDR or not): a second
                // nginx used to start beside the first and take over its pid
                // file, so `nginx -s quit` left the first one running.
                // IPv4 and IPv6 apart: `0.0.0.0` beside `::` with v6only.
                let taken = |convs: &[Conv; MAX_CONV], port: u16| {
                    convs.iter().enumerate().any(|(j, c)| {
                        j != i
                            && matches!(c.kind, Kind::Tcp)
                            && c.listen_port == port
                            && overlaps(c.local, c.v6only, addr, v6only)
                    })
                };
                if port == 0 {
                    match (0..u16::MAX - LOCAL_PORT_BASE)
                        .map(|_| next_local_port(local_ports))
                        .find(|&p| !taken(convs, p))
                    {
                        Some(p) => port = p,
                        None => {
                            reply(chan, REP_ERR, conv, -1, b"addrinuse");
                            return;
                        }
                    }
                }
                if taken(convs, port) {
                    reply(chan, REP_ERR, conv, -1, b"addrinuse");
                    return;
                }
                convs[i].local = addr;
                convs[i].v6only = v6only;
                convs[i].listen_port = port;
                let s = sockets.get_mut::<tcp::Socket>(h);
                match listen_on(s, &convs[i]) {
                    Ok(()) => {
                        let mut st = String::new();
                        let _ = write!(st, "listening {port}");
                        reply(chan, REP_STATUS, conv, 0, st.as_bytes());
                    }
                    Err(_) => {
                        convs[i].local = ANY4;
                        convs[i].v6only = false;
                        convs[i].listen_port = 0;
                        reply(chan, REP_ERR, conv, -1, b"listen");
                    }
                }
            }
            _ => reply(chan, REP_ERR, conv, -1, b"tcp only"),
        }
        return;
    }
    // Userspace finished open() on the advertised head handoff. Drop that
    // head without re-advertising it (ctl "accept" used to take+reply the
    // same <N>, and with accepted_pending a seq bump made the next accept()
    // re-open the live Child — dropbear "bad packet size 0x53534831" / SSH1).
    // Promote pending to head and advertise it; otherwise status=listening.
    //
    // "taken <seq>" names the handoff it consumed, which makes it idempotent:
    // libgloss re-sends it while the status still shows that seq (the first
    // write can fail when the netfs request ring is full), and a late
    // duplicate must not drop the *next* head. A bare "taken" is accepted
    // for older userspace.
    if let Some(rest) = cmd.strip_prefix(b"taken") {
        match convs[i].kind {
            Kind::Tcp if convs[i].listen_port != 0 => {}
            _ => {
                reply(chan, REP_ERR, conv, -1, b"not listening");
                return;
            }
        }
        let want = parse_seq(trim(rest));
        let stale = convs[i].accepted.is_none()
            || want.is_some_and(|s| s != convs[i].accept_seq);
        if stale {
            // Already consumed: re-publish the current state unchanged.
            match convs[i].accepted {
                Some(n) => {
                    let rep = accept_reply(convs, n, convs[i].accept_seq);
                    reply(chan, REP_STATUS, conv, 0, &rep);
                }
                None => reply(chan, REP_STATUS, conv, 0, b"listening"),
            }
            return;
        }
        if let Some(n) = convs[i].accepted {
            if let Some(c) = convs.get_mut(n as usize) {
                c.held = !matches!(c.kind, Kind::Empty);
            }
        }
        let next = convs[i].accepted_pending.take();
        publish_head(convs, chan, i, next);
        return;
    }
    // Arm accept-wait (or re-advertise a parked head). Never *take* the
    // head here — that used to re-hand the live Child to the next accept()
    // (dropbear Integrity / bad packet size 0x53534831). Only ctl "taken"
    // (libgloss after open) removes the head / promotes pending.
    if cmd == b"accept" {
        match convs[i].kind {
            Kind::Tcp if convs[i].listen_port != 0 => {}
            _ => {
                reply(chan, REP_ERR, conv, -1, b"not listening");
                return;
            }
        }
        if let Some(n) = convs[i].accepted {
            let seq = convs[i].accept_seq;
            let rep = accept_reply(convs, n, seq);
            reply(chan, REP_STATUS, conv, 0, &rep);
            return;
        }
        // Idempotent: a repeated arm used to answer REP_ERR "accept busy",
        // which overwrote the listener status. pump_accepts publishes every
        // new head whether or not an accept is parked.
        reply(chan, REP_STATUS, conv, 0, b"listening");
        convs[i].accept_wait = true;
        return;
    }
    let Some((addr, port)) = parse_connect(cmd) else {
        reply(chan, REP_ERR, conv, -1, b"bad ctl");
        return;
    };
    match convs[i].kind {
        Kind::Empty => {
            reply(chan, REP_ERR, conv, -1, b"no conv");
        }
        Kind::Icmp => {
            convs[i].remote = addr;
            convs[i].have_remote = true;
            if let Some(h) = convs[i].handle {
                let s = sockets.get_mut::<icmp::Socket>(h);
                if !s.is_open() {
                    let _ = s.bind(icmp::Endpoint::Ident(convs[i].ident));
                }
            }
            convs[i].connected = true;
            reply(chan, REP_STATUS, conv, 0, b"connected");
        }
        // handle_udp_ctl
        Kind::Udp => {}
        Kind::Tcp => {
            let p = match port {
                Some(p) => p,
                None => {
                    reply(chan, REP_ERR, conv, -1, b"need port");
                    return;
                }
            };
            let addr = or_localhost(addr);
            convs[i].remote = addr;
            convs[i].remote_port = p;
            convs[i].have_remote = true;
            let port = next_local_port(local_ports);
            let local = IpListenEndpoint { addr: nics.source_for(addr), port };
            let remote = IpEndpoint::new(addr, p);
            // The loopback interface carries a local connection.
            let src = nics.source_for(addr);
            let cx = if is_local(nics, addr) { Some(lo.context()) } else { nics.context_for(src) };
            if let Some(h) = convs[i].handle {
                let s = sockets.get_mut::<tcp::Socket>(h);
                // No NIC to leave through: refused at once, like a connect
                // without a route.
                let r = match cx {
                    Some(cx) => s.connect(cx, remote, local),
                    None => Err(tcp::ConnectError::Unaddressable),
                };
                match r {
                    Ok(()) => reply(chan, REP_STATUS, conv, 0, b"connecting"),
                    Err(_) => reply(chan, REP_ERR, conv, -1, b"tcp connect"),
                }
            }
        }
    }
}

/// A UDP conv's ctl commands. The socket library tags its own with a last
/// `#<n>` word and gets "#<n> ok <addr>!<port>[ <addr>!<port>]" (the local
/// address after it, then the peer's if connected) or "#<n> fail <why>"
/// back; `connect` without a tag is the plain
/// Plan 9 one (DNS, the Linux layer), answered "connected".
///
/// - `bind <addr>!<port>[ reuse][ v6only]`: port 0 picks a free one;
///   `reuse` is SO_REUSEADDR (the port is shared with others bound so),
///   `v6only` IPV6_V6ONLY (on `::`, no IPv4 datagrams).
/// - `connect <addr>!<port>`: the peer, the only one datagrams come from
///   and the one a header-less send goes to; binds a port first if none.
/// - `disconnect`: no peer; gives up an address or port bind did not set.
/// - `autobind[ ::[ v6only]]`: a port, as a first send would bind (the
///   socket library asks first, to know it); on `::` for an IPv6 socket.
/// - `local`: nothing; the answer (an fd's socket the library learns of
///   after an exec).
fn handle_udp_ctl(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    nics: &Nics,
    chan: usize,
    i: usize,
    cmd: &[u8],
    local_ports: &mut u16,
) {
    let (cmd, tag) = match cmd.iter().rposition(|&b| b == b'#') {
        Some(n) if n == 0 || cmd[n - 1] == b' ' => (trim(&cmd[..n]), Some(&cmd[n + 1..])),
        _ => (cmd, None),
    };
    let result = if let Some(rest) = cmd.strip_prefix(b"bind") {
        let mut words = trim(rest).split(|&b| b == b' ').filter(|w| !w.is_empty());
        let ep = words.next().unwrap_or(b"");
        let (mut reuse, mut v6only) = (false, false);
        for w in words {
            reuse |= w == b"reuse";
            v6only |= w == b"v6only";
        }
        match parse_endpoint(ep) {
            Some((addr, port)) => udp_bind(convs, sockets, nics, i, addr, port, reuse, v6only, local_ports),
            None => Err("inval"),
        }
    } else if let Some((addr, port)) = parse_connect(cmd) {
        let port = port.unwrap_or(0);
        let r = udp_connect(convs, sockets, nics, i, addr, port, local_ports);
        if r.is_ok() {
            let c = &convs[i];
            let mut peer = [0u8; 18];
            peer[..16].copy_from_slice(&addr16(c.remote));
            peer[16..].copy_from_slice(&c.remote_port.to_be_bytes());
            reply(chan, REP_PEER, i as u16, 0, &peer);
        }
        r
    } else if cmd == b"disconnect" {
        udp_disconnect(convs, sockets, i);
        Ok(())
    } else if let Some(rest) = cmd.strip_prefix(b"autobind") {
        let rest = trim(rest);
        let (rest, v6only) = match rest.strip_suffix(b"v6only") {
            Some(r) => (trim(r), true),
            None => (rest, false),
        };
        if convs[i].local_port == 0 && rest == b"::" {
            convs[i].local = ANY6;
            convs[i].v6only = v6only;
            convs[i].inet6 = true;
        }
        udp_autobind(convs, sockets, i, local_ports)
    } else if cmd == b"local" {
        Ok(())
    } else {
        Err("inval")
    };
    let Some(tag) = tag else {
        match result {
            Ok(()) => reply(chan, REP_STATUS, i as u16, 0, b"connected"),
            Err(_) => reply(chan, REP_ERR, i as u16, -1, b"error"),
        }
        return;
    };
    let mut out = String::new();
    out.push('#');
    out.push_str(core::str::from_utf8(tag).unwrap_or(""));
    match result {
        Ok(()) => {
            let c = &convs[i];
            let _ = write!(out, " ok {}!{}", c.local, c.local_port);
            if c.have_remote {
                let _ = write!(out, " {}!{}", c.remote, c.remote_port);
            }
        }
        Err(why) => {
            let _ = write!(out, " fail {why}");
        }
    }
    reply(chan, REP_STATUS, i as u16, 0, out.as_bytes());
}

/// `a.b.c.d!port` or `ipv6!port`
fn parse_endpoint(s: &[u8]) -> Option<(IpAddress, u16)> {
    let n = s.iter().rposition(|&b| b == b'!')?;
    Some((parse_addr(&s[..n])?, parse_port(&s[n + 1..])?))
}

/// What a UDP conv may bind to, as on Linux: any address, 127.0.0.0/8,
/// ::1, a NIC's address, and the IPv4 broadcast addresses (all ones, and
/// the first and last of a NIC's prefix).
fn udp_bindable(nics: &Nics, a: IpAddress) -> bool {
    a.is_unspecified() || is_loopback(a) || a.is_broadcast() || nics.bindable(a)
}

/// Whether UDP conv `i` may have `addr`!`port` (`v6only`): no other conv
/// would take a datagram to that port that it would (`overlaps`), unless
/// both were bound with SO_REUSEADDR.
fn udp_port_free(
    convs: &[Conv; MAX_CONV],
    i: usize,
    addr: IpAddress,
    v6only: bool,
    port: u16,
    reuse: bool,
) -> bool {
    convs.iter().enumerate().all(|(j, c)| {
        j == i
            || !matches!(c.kind, Kind::Udp)
            || c.local_port != port
            || (reuse && c.reuse)
            || !overlaps(c.local, c.v6only, addr, v6only)
    })
}

/// Give UDP conv `i` its local endpoint and listen there (smoltcp drops
/// what its socket held: nothing came yet, or the port is given up).
fn udp_set_local(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    i: usize,
    addr: IpAddress,
    port: u16,
) {
    let c = &mut convs[i];
    c.local = addr;
    c.local_port = port;
    let Some(h) = c.handle else {
        return;
    };
    let s = sockets.get_mut::<udp::Socket>(h);
    s.close();
    if port != 0 {
        // Only a bound address filters what arrives; connect's does not
        // (it filters on the peer), but a socket's version does.
        let bound = c.addr_bound && !addr.is_unspecified();
        s.set_ip_version(if bound { None } else { version_of(c.local, c.v6only) });
        let _ = s.bind(IpListenEndpoint { addr: bound.then_some(addr), port });
    }
}

/// A free ephemeral port for UDP conv `i` at its address.
fn udp_free_port(convs: &[Conv; MAX_CONV], i: usize, local_ports: &mut u16) -> Option<u16> {
    let (addr, v6only) = (convs[i].local, convs[i].v6only);
    (0..u16::MAX - LOCAL_PORT_BASE)
        .map(|_| next_local_port(local_ports))
        .find(|&p| udp_port_free(convs, i, addr, v6only, p, false))
}

#[allow(clippy::too_many_arguments)]
fn udp_bind(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    nics: &Nics,
    i: usize,
    addr: IpAddress,
    port: u16,
    reuse: bool,
    v6only: bool,
    local_ports: &mut u16,
) -> Result<(), &'static str> {
    if convs[i].local_port != 0 {
        return Err("inval");
    }
    if !udp_bindable(nics, addr) {
        return Err("addrnotavail");
    }
    convs[i].local = addr;
    convs[i].v6only = v6only;
    convs[i].inet6 = addr.version() == IpVersion::Ipv6;
    let port = match port {
        0 => udp_free_port(convs, i, local_ports).ok_or("addrinuse")?,
        p if udp_port_free(convs, i, addr, v6only, p, reuse) => p,
        _ => return Err("addrinuse"),
    };
    convs[i].addr_bound = !addr.is_unspecified();
    convs[i].port_bound = true;
    convs[i].reuse = reuse;
    udp_set_local(convs, sockets, i, addr, port);
    Ok(())
}

/// Bind UDP conv `i` to a free port on its address if it has none yet
/// (Linux does so on the first connect or send).
fn udp_autobind(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    i: usize,
    local_ports: &mut u16,
) -> Result<(), &'static str> {
    if convs[i].local_port != 0 {
        return Ok(());
    }
    let addr = convs[i].local;
    let port = udp_free_port(convs, i, local_ports).ok_or("addrinuse")?;
    udp_set_local(convs, sockets, i, addr, port);
    Ok(())
}

fn udp_connect(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    nics: &Nics,
    i: usize,
    addr: IpAddress,
    port: u16,
    local_ports: &mut u16,
) -> Result<(), &'static str> {
    let addr = or_localhost(addr);
    // The source address a send there would have, unless bind chose one.
    if !convs[i].addr_bound {
        convs[i].local = nics.source_for(addr).ok_or("netunreach")?;
    }
    udp_autobind(convs, sockets, i, local_ports)?;
    let c = &mut convs[i];
    c.remote = addr;
    c.remote_port = port;
    c.have_remote = true;
    c.connected = true;
    Ok(())
}

fn udp_disconnect(convs: &mut [Conv; MAX_CONV], sockets: &mut SocketSet<'_>, i: usize) {
    let c = &mut convs[i];
    c.have_remote = false;
    c.connected = false;
    let addr = match (c.addr_bound, c.inet6) {
        (true, _) => c.local,
        (false, true) => ANY6,
        (false, false) => ANY4,
    };
    let port = if c.port_bound { c.local_port } else { 0 };
    if (addr, port) != (c.local, c.local_port) {
        udp_set_local(convs, sockets, i, addr, port);
    }
}

/// Poll TCP listeners: when smoltcp moved a listening socket out of the
/// Listen state, an incoming connection landed. Hand it to a fresh conv,
/// re-arm a listener on the same port, and complete any parked "accept".
fn rearm_listener(convs: &mut [Conv; MAX_CONV], sockets: &mut SocketSet<'_>, i: usize) {
    if let Some(h) = convs[i].handle.take() {
        {
            let s = sockets.get_mut::<tcp::Socket>(h);
            if !matches!(s.state(), tcp::State::Closed | tcp::State::Listen) {
                s.abort();
            }
        }
        sockets.remove(h);
    }
    let rx = tcp::SocketBuffer::new(vec![0; TCP_RX]);
    let tx = tcp::SocketBuffer::new(vec![0; TCP_TX]);
    let nh = sockets.add(tcp::Socket::new(rx, tx));
    let _ = listen_on(sockets.get_mut::<tcp::Socket>(nh), &convs[i]);
    convs[i].handle = Some(nh);
}


/// Drain any remaining smoltcp RX into REP_DATA before destroying a conv.
/// Orphan reclaim / hungup drop used to abort sockets that still held the
/// peer payload (CloseWait with data after accept handoff), so userspace
/// saw hangup / EOF with zero bytes — the bios `socket_smoke: no data`
/// class of failure on the listen/accept data path.
fn drain_tcp_rx_into_rep(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    chan: usize,
    i: usize,
) {
    if i >= MAX_CONV || !matches!(convs[i].kind, Kind::Tcp) {
        return;
    }
    let Some(h) = convs[i].handle else {
        return;
    };
    let mut tmp = [0u8; REP_CAP - REP_HDR];
    for _ in 0..8 {
        let s = sockets.get_mut::<tcp::Socket>(h);
        let room = (convs[i].rx_room as usize).min(tmp.len());
        if !s.can_recv() || room == 0 {
            break;
        }
        match s.recv_slice(&mut tmp[..room]) {
            Ok(n) if n > 0 => {
                convs[i].rx_room -= n as u16;
                reply(chan, REP_DATA, i as u16, 0, &tmp[..n]);
            }
            _ => break,
        }
    }
}

/// Free a never-connected accepted TCP slot (failed handshake / peer abort).
/// Clears any listener `accepted` pointer at this id and notifies netfs.
fn force_free_orphan(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    chan: usize,
    i: usize,
) {
    for l in 0..MAX_CONV {
        if convs[l].accepted_pending == Some(i as u16) {
            convs[l].accepted_pending = None;
        }
        if convs[l].accepted == Some(i as u16) {
            let next = convs[l].accepted_pending.take();
            publish_head(convs, chan, l, next);
        }
    }
    // Deliver any still-buffered peer payload before abort (CloseWait with
    // data was treated as a dead orphan and discarded).
    drain_tcp_rx_into_rep(convs, sockets, chan, i);
    if let Some(h) = convs[i].handle.take() {
        {
            let s = sockets.get_mut::<tcp::Socket>(h);
            if !matches!(s.state(), tcp::State::Closed) {
                s.abort();
            }
        }
        sockets.remove(h);
    }
    if !matches!(convs[i].kind, Kind::Empty) {
        reply(chan, REP_STATUS, i as u16, 0, b"hangup");
    }
    convs[i] = Conv::EMPTY;
}

/// Move a connection whose handshake is done, socket `h` of listener `l`,
/// to a free conv and tell the listener's user; the caller checks first
/// that the listener's queue has room ([`queue_full`]). False when every
/// index is a live session; the caller resets the connection and the port
/// stays answerable (keeping it on the listener instead left no Listen
/// socket and dropped every later SYN, CI #1164).
fn hand_off(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    chan: usize,
    l: usize,
    h: SocketHandle,
) -> bool {
    // Peer endpoint captured from the connected socket.
    let (remote, remote_port) = match sockets.get_mut::<tcp::Socket>(h).remote_endpoint() {
        Some(IpEndpoint { addr, port }) => (addr, port),
        None => (ANY4, 0),
    };
    if queue_full(convs, l) {
        return false;
    }
    let slot = (0..MAX_CONV).find(|&n| matches!(convs[n].kind, Kind::Empty));
    let n = match slot {
        Some(n) => n,
        None => {
            // No free index: reclaim the oldest accepted connection nobody
            // took that is not a live session; if every index is one, this
            // connection is the one reset.
            let victim = (0..MAX_CONV).find(|&n| {
                convs[n].from_accept
                    && !convs[n].held
                    && !convs[n].connected
                    && !convs[n].hungup
                    && !convs[n].closing
                    && !convs[n].closing_after_flush
            });
            match victim {
                Some(v) => {
                    force_free_orphan(convs, sockets, chan, v);
                    // Drop any queue pointer that named the freed slot; the
                    // handoff below re-queues the fresh conv instead.
                    if convs[l].accepted == Some(v as u16) {
                        convs[l].accepted = convs[l].accepted_pending.take();
                    }
                    if convs[l].accepted_pending == Some(v as u16) {
                        convs[l].accepted_pending = None;
                    }
                    v
                }
                None => return false,
            }
        }
    };
    convs[n] = Conv {
        kind: Kind::Tcp,
        handle: Some(h),
        remote,
        remote_port,
        have_remote: true,
        // pump_sockets owns the connected flag: it sets it on its next pass
        // and tells netfs ("connected") with it.
        connected: false,
        from_accept: true,
        ..Conv::EMPTY
    };
    // Tell netfs about the new conv exactly once: pump-accepted convs
    // bypass clone, so without this netfs never marks the slot used and
    // the client's open of /net/tcp/<n>/data fails.
    reply(chan, REP_CLONE_OK, n as u16, 0, b"tcp");
    if convs[l].accepted.is_none() {
        // Publish even when no accept is parked: libgloss arms only once
        // per consumed handoff, and a lost/late arm must not hide this
        // connection behind a stale "listening".
        publish_head(convs, chan, l, Some(n as u16));
    } else {
        // Head still parked; queue second without bumping head seq/status.
        convs[l].accepted_pending = Some(n as u16);
    }
    true
}

/// Listener `l` has two handoffs its user has still to take (dropbear
/// decoding sessions). A further completed connection waits among the
/// handshakes until one is taken ([`ACCEPT_WAIT`] of them at most); reset
/// instead, it lost what its client had already sent (issue #630).
fn queue_full(convs: &[Conv; MAX_CONV], l: usize) -> bool {
    convs[l].accepted.is_some() && convs[l].accepted_pending.is_some()
}

/// Connection `n` is in a listener's queue, its user has still to take it.
fn queued(convs: &[Conv; MAX_CONV], n: usize) -> bool {
    convs.iter().any(|c| c.accepted == Some(n as u16) || c.accepted_pending == Some(n as u16))
}

/// The completed connections among `handshakes` waiting for listener `l`'s
/// queue ([`queue_full`]).
fn waiting(handshakes: &[Handshake], sockets: &SocketSet<'_>, l: usize) -> usize {
    handshakes
        .iter()
        .filter(|hs| {
            let state = sockets.get::<tcp::Socket>(hs.handle).state();
            usize::from(hs.listener) == l
                && matches!(state, tcp::State::Established | tcp::State::CloseWait)
        })
        .count()
}

/// Incoming connections. A listening socket that left the Listen state
/// took a SYN and is that connection now: it joins the handshakes beside
/// a fresh listener on the port. A handshake that completed is handed to
/// a conv (`hand_off`) once its listener's queue has room, in the order
/// the connections completed (one landing whole on the listener waits
/// behind those parked); one reset by its peer, timed out, or whose
/// listener closed is forgotten.
fn pump_accepts(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    handshakes: &mut Vec<Handshake>,
    chan: usize,
    now: Instant,
) {
    // Stale parks first: a hangup drop emptied a slot but left a queue
    // pointer, and the room it leaves goes to the connection that has
    // waited longest, below, not to the one landing on the listener.
    for i in 0..MAX_CONV {
        if !matches!(convs[i].kind, Kind::Tcp) || convs[i].listen_port == 0 {
            continue;
        }
        if let Some(n) = convs[i].accepted_pending {
            let n = n as usize;
            if n >= MAX_CONV || matches!(convs[n].kind, Kind::Empty) {
                convs[i].accepted_pending = None;
            }
        }
        if let Some(n) = convs[i].accepted {
            let n = n as usize;
            if n >= MAX_CONV || matches!(convs[n].kind, Kind::Empty) {
                let next = convs[i].accepted_pending.take();
                publish_head(convs, chan, i, next);
            }
        }
    }
    let mut k = 0;
    while k < handshakes.len() {
        let Handshake { handle: h, listener, since } = handshakes[k];
        let l = listener as usize;
        let listening = matches!(convs[l].kind, Kind::Tcp) && convs[l].listen_port != 0;
        let s = sockets.get_mut::<tcp::Socket>(h);
        let done = match s.state() {
            // Done, or done and closed by the peer within the same poll (a
            // health check that said its piece and left): accept returns
            // it, as on Linux, and its user reads what came and the end.
            tcp::State::Established | tcp::State::CloseWait => true,
            tcp::State::SynReceived if listening && now - since < HANDSHAKE_TIMEOUT => {
                k += 1;
                continue;
            }
            // Reset by the peer (back in Listen), closed, or timed out.
            _ => false,
        };
        // A completed one waits its turn while the listener's queue is
        // full, behind the ones that waited before it.
        if done
            && listening
            && queue_full(convs, l)
            && waiting(&handshakes[..k], sockets, l) < ACCEPT_WAIT
        {
            k += 1;
            continue;
        }
        handshakes.remove(k);
        if !(done && listening && hand_off(convs, sockets, chan, l, h)) {
            drop_handshake(sockets, h);
        }
    }
    for i in 0..MAX_CONV {
        if !matches!(convs[i].kind, Kind::Tcp) || convs[i].listen_port == 0 {
            continue;
        }
        let Some(h) = convs[i].handle else {
            continue;
        };
        let s = sockets.get_mut::<tcp::Socket>(h);
        let done = match s.state() {
            tcp::State::Listen => continue,
            tcp::State::Closed => {
                let _ = listen_on(s, &convs[i]);
                continue;
            }
            tcp::State::SynReceived => false,
            // The whole handshake within one poll (loopback, a fast peer),
            // closed by the peer already or not.
            tcp::State::Established | tcp::State::CloseWait => true,
            // Peer-aborted sockets left on the listen handle (FinWait, ...)
            // used to look like "arrived" and were handed to dropbear as
            // Child + write EIO.
            _ => {
                rearm_listener(convs, sockets, i);
                continue;
            }
        };
        // The connection keeps the socket; the listener gets a new one.
        convs[i].handle = None;
        rearm_listener(convs, sockets, i);
        convs[i].connected = false;
        convs[i].hungup = false;
        // Straight into the queue only when no completed connection waits
        // for this listener already: those go first, in the order they
        // completed. A "done" that landed whole in one poll took the room a
        // mute connection parked above had waited for, and the listen
        // smoke's accept got the newer one (CI #2339, riscv64-plic).
        if done && !queue_full(convs, i) && waiting(handshakes, sockets, i) == 0 {
            if !hand_off(convs, sockets, chan, i, h) {
                drop_handshake(sockets, h);
            }
        } else if handshakes.len() < SYN_BACKLOG
            && (!done || waiting(handshakes, sockets, i) < ACCEPT_WAIT)
        {
            handshakes.push(Handshake { handle: h, listener: i as u16, since: now });
        } else {
            drop_handshake(sockets, h);
        }
    }
}

fn handle_send(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    nics: &Nics,
    chan: usize,
    conv: u16,
    payload: &[u8],
    local_ports: &mut u16,
) {
    let i = conv as usize;
    if i >= MAX_CONV {
        reply(chan, REP_ERR, conv, -1, b"no conv");
        return;
    }
    match convs[i].kind {
        Kind::Icmp => {
            if !convs[i].have_remote {
                reply(chan, REP_ERR, conv, -1, b"not connected");
                return;
            }
            let Some(h) = convs[i].handle else {
                return;
            };
            let ident = convs[i].ident;
            convs[i].seq = convs[i].seq.wrapping_add(1);
            let seq = convs[i].seq;
            let dst = convs[i].remote;
            let checksum = device_capabilities().checksum;
            let s = sockets.get_mut::<icmp::Socket>(h);
            if !s.is_open() {
                let _ = s.bind(icmp::Endpoint::Ident(ident));
            }
            // From the NIC whose subnet holds the destination (a NIC's
            // interface sends only from its own addresses).
            let src = nics.source_for(dst);
            s.set_source(src);
            if !s.can_send() {
                return;
            }
            match (dst, src) {
                (IpAddress::Ipv4(_), _) => {
                    let echo = Icmpv4Repr::EchoRequest { ident, seq_no: seq, data: payload };
                    if let Ok(buf) = s.send(echo.buffer_len(), dst) {
                        echo.emit(&mut Icmpv4Packet::new_unchecked(buf), &checksum);
                    }
                }
                // The socket checksums an ICMPv6 message itself, once it
                // knows the source.
                (IpAddress::Ipv6(d), Some(IpAddress::Ipv6(src))) => {
                    let echo = Icmpv6Repr::EchoRequest { ident, seq_no: seq, data: payload };
                    if let Ok(buf) = s.send(echo.buffer_len(), dst) {
                        echo.emit(&src, &d, &mut Icmpv6Packet::new_unchecked(buf), &checksum);
                    }
                }
                _ => {}
            }
        }
        Kind::Udp => {
            // A datagram behind its header (UDP_HDR): a zero destination is
            // the peer. Port 0 and a full send buffer drop it, as UDP may.
            if payload.len() < UDP_HDR {
                return;
            }
            let (hdr, data) = payload.split_at(UDP_HDR);
            let rport = u16::from_be_bytes([hdr[32], hdr[33]]);
            // No address: the peer (no port either), else `::` (this
            // host, as `::ffff:0.0.0.0` is).
            let (dst, port) = match from_addr16(&hdr[..16]) {
                None if rport == 0 => {
                    if !convs[i].have_remote {
                        return;
                    }
                    (convs[i].remote, convs[i].remote_port)
                }
                None => (localhost(IpVersion::Ipv6), rport),
                Some(a) => (or_localhost(a), rport),
            };
            // A first send to an IPv6 address binds on `::`: the socket is
            // an IPv6 one (an IPv4 one has none to send to), whose port
            // takes IPv4 datagrams too, as on Linux.
            let c = &mut convs[i];
            if c.local_port == 0 && !c.addr_bound && dst.version() == IpVersion::Ipv6 {
                c.local = ANY6;
                c.inet6 = true;
            }
            if port == 0 || udp_autobind(convs, sockets, i, local_ports).is_err() {
                return;
            }
            let Some(h) = convs[i].handle else {
                return;
            };
            let mut meta = udp::UdpMetadata::from(IpEndpoint::new(dst, port));
            let src = convs[i].local;
            meta.local_address = if !src.is_unspecified() && !src.is_broadcast() && src.version() == dst.version() {
                Some(src)
            } else {
                nics.source_for(dst)
            };
            let s = sockets.get_mut::<udp::Socket>(h);
            if s.can_send() {
                let _ = s.send_slice(data, meta);
            }
        }
        Kind::Tcp => {
            let Some(h) = convs[i].handle else {
                return;
            };
            // Behind what already waits, so the stream keeps its order.
            convs[i].pending.extend_from_slice(payload);
            let n = flush_pending(&mut convs[i], sockets.get_mut::<tcp::Socket>(h));
            tx_credit(chan, conv, n);
        }
        Kind::Empty => reply(chan, REP_ERR, conv, -1, b"no conv"),
    }
}

fn pump_sockets(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    chan: usize,
) {
    let checksum = device_capabilities().checksum;
    let mut rx_tmp = [0u8; REP_CAP - REP_HDR];
    for i in 0..MAX_CONV {
        let conv = i as u16;
        match convs[i].kind {
            Kind::Empty => {}
            Kind::Icmp => {
                let Some(h) = convs[i].handle else {
                    continue;
                };
                let s = sockets.get_mut::<icmp::Socket>(h);
                if s.can_recv() {
                    match s.recv() {
                        Ok((payload, IpAddress::Ipv4(_))) => {
                            if let Ok(pkt) = Icmpv4Packet::new_checked(payload) {
                                if let Ok(Icmpv4Repr::EchoReply { data, .. }) = Icmpv4Repr::parse(&pkt, &checksum) {
                                    reply(chan, REP_DATA, conv, 0, data);
                                }
                            }
                        }
                        // The interface checked the checksum.
                        Ok((payload, IpAddress::Ipv6(src))) => {
                            if let Ok(pkt) = Icmpv6Packet::new_checked(payload) {
                                let dst = Ipv6Address::UNSPECIFIED;
                                let ignored = ChecksumCapabilities::ignored();
                                if let Ok(Icmpv6Repr::EchoReply { data, .. }) = Icmpv6Repr::parse(&src, &dst, &pkt, &ignored) {
                                    reply(chan, REP_DATA, conv, 0, data);
                                }
                            }
                        }
                        Err(_) => {}
                    }
                }
            }
            Kind::Udp => {
                let Some(h) = convs[i].handle else {
                    continue;
                };
                let c = &convs[i];
                let s = sockets.get_mut::<udp::Socket>(h);
                for _ in 0..UDP_PACKETS {
                    let Ok((data, meta)) = s.recv() else {
                        break;
                    };
                    let src = meta.endpoint.addr;
                    // A connected socket hears its peer only.
                    if c.have_remote && (src, meta.endpoint.port) != (c.remote, c.remote_port) {
                        continue;
                    }
                    let dst = meta.local_address.unwrap_or(c.local);
                    let mut msg = Vec::with_capacity(UDP_HDR + data.len());
                    msg.extend_from_slice(&addr16(src));
                    msg.extend_from_slice(&addr16(dst));
                    msg.extend_from_slice(&meta.endpoint.port.to_be_bytes());
                    msg.extend_from_slice(&c.local_port.to_be_bytes());
                    msg.extend_from_slice(data);
                    reply(chan, REP_DATA, conv, 0, &msg);
                }
            }
            Kind::Tcp => {
                let Some(h) = convs[i].handle else {
                    continue;
                };
                // Listeners (listen_port != 0) must not run connected/data/hangup
                // logic — even while a backlog SYN is held on the listen handle.
                // pump_sockets used to mark the listener `connected` when the
                // held socket reached Established, overwrite status, drain KEX
                // bytes as REP_DATA on the listen conv, then after re-arm hit
                // hangup (connected && !is_active on Listen) and wedge further
                // accepts under concurrent dual-SYN (dropbear SSH smoke).
                if convs[i].listen_port != 0 {
                    continue;
                }
                let n = flush_pending(&mut convs[i], sockets.get_mut::<tcp::Socket>(h));
                tx_credit(chan, conv, n);
                // Deferred close (drop_conv couldn't flush pending yet):
                // pending fully queued now -> finish the graceful close.
                if convs[i].closing_after_flush && convs[i].pending.is_empty() {
                    let s = sockets.get_mut::<tcp::Socket>(h);
                    s.close();
                    convs[i].closing = true;
                    convs[i].closing_after_flush = false;
                }
                let s = sockets.get_mut::<tcp::Socket>(h);
                // Only Established (may_send && may_recv). CloseWait has may_send but
                // may_recv==false once RX is empty — never advertise "connected" there
                // or wait_connected races into hangup (curl:7 / socket_smoke no data).
                if s.is_active()
                    && s.may_send()
                    && s.may_recv()
                    && !convs[i].connected
                    && !convs[i].hungup
                {
                    convs[i].connected = true;
                    reply(chan, REP_STATUS, conv, 0, b"connected");
                }
                // A connect the peer refused (a reset to its SYN, as for a
                // port of 127.0.0.1 nobody listens on): Closed without ever
                // having been Established.
                if !convs[i].connected
                    && !convs[i].hungup
                    && convs[i].have_remote
                    && !convs[i].from_accept
                    && s.state() == tcp::State::Closed
                {
                    convs[i].hungup = true;
                    reply(chan, REP_STATUS, conv, 0, b"hangup");
                }
                // An accepted connection gone before it was seen Established:
                // the peer's FIN or reset came with the handshake's last ACK
                // (a port scanner, a health check), so `connected` never got
                // set and the hangup rule below never fired. The process that
                // took it then waited for a client that was gone until its own
                // timeout (dropbear: five minutes), the index pinned meanwhile
                // as a held conversation; on a VPS they ran netd out of indices
                // within minutes of a boot and every new connection was reset
                // before its banner. Past the handshake with nothing left to
                // read, it is hung up like any other.
                if !convs[i].connected
                    && !convs[i].hungup
                    && convs[i].from_accept
                    && !s.can_recv()
                    && !matches!(s.state(), tcp::State::SynReceived | tcp::State::Established)
                {
                    // A reset to its SYN-ACK sent the socket back to
                    // listening: it must not take the port's next SYN.
                    if s.state() == tcp::State::Listen {
                        s.abort();
                    }
                    convs[i].hungup = true;
                    reply(chan, REP_STATUS, conv, 0, b"hangup");
                }
                // All netfs has room for, a message of up to `REP_CAP` at a
                // time: one MSS per pass left a 64 KiB window's worth waiting
                // through as many wake-ups.
                loop {
                    let room = (convs[i].rx_room as usize).min(REP_CAP - REP_HDR);
                    if room == 0 || !s.can_recv() {
                        break;
                    }
                    let Ok(n) = s.recv_slice(&mut rx_tmp[..room]) else {
                        break;
                    };
                    if n == 0 {
                        break;
                    }
                    convs[i].rx_room -= n as u16;
                    reply(chan, REP_DATA, conv, 0, &rx_tmp[..n]);
                }
                // Peer FIN / inactive: hangup only after RX drained into REP_DATA.
                // CloseWait + empty RX => may_recv false; Closed => !active.
                let s = sockets.get_mut::<tcp::Socket>(h);
                if convs[i].connected
                    && !convs[i].hungup
                    && !s.can_recv()
                    && (!s.is_active() || !s.may_recv())
                {
                    convs[i].hungup = true;
                    reply(chan, REP_STATUS, conv, 0, b"hangup");
                }
            }
        }
    }
}

fn handle_req(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    handshakes: &mut Vec<Handshake>,
    lingering: &mut Vec<Lingering>,
    nics: &mut Nics,
    lo: &mut Interface,
    chan: usize,
    msg: &[u8],
    local_ports: &mut u16,
) {
    if msg.len() < REQ_HDR {
        return;
    }
    let typ = msg[0];
    let conv = u16::from_le_bytes([msg[1], msg[2]]);
    let proto = msg[3];
    let plen = u16::from_le_bytes([msg[4], msg[5]]) as usize;
    if REQ_HDR + plen > msg.len() {
        return;
    }
    let payload = &msg[REQ_HDR..REQ_HDR + plen];
    match typ {
        REQ_CLONE => {
            handle_clone(convs, sockets, lingering, proto, conv);
            reply(chan, REP_CLONE_OK, conv, 0, &[]);
        }
        REQ_CTL => handle_ctl(convs, sockets, nics, lo, chan, conv, payload, local_ports),
        REQ_SEND => handle_send(convs, sockets, nics, chan, conv, payload, local_ports),
        REQ_CLOSE => {
            if let Some(c) = convs.get_mut(conv as usize) {
                c.released = true;
            }
            if convs.get(conv as usize).is_some_and(|c| matches!(c.kind, Kind::Tcp) && c.listen_port != 0) {
                close_listener(convs, sockets, handshakes, chan, conv as usize);
            } else if (conv as usize) < MAX_CONV {
                release_conv(convs, sockets, lingering, conv as usize);
            }
            reply(chan, REP_STATUS, conv, 0, b"hangup");
        }
        REQ_CREDIT => {
            if let (Some(c), [a, b, ..]) = (convs.get_mut(conv as usize), payload) {
                c.rx_room = c.rx_room.saturating_add(u16::from_le_bytes([*a, *b])).min(NETFS_RX_CAP);
            }
        }
        _ => {}
    }
}

fn main() -> ! {
    heap_init();
    let (device, chan) = wait_devices();

    let mut clock = VirtualInstant::new();
    // /net/convs: netd's table, pushed to netfs once a second when it changed.
    let mut convs_text_last = String::new();
    let mut convs_text_at = Instant::from_millis(0);
    let mut lodev = LoopDevice::new();
    let mut lo = build_loopback(&mut lodev, clock.now());
    let mut sockets = SocketSet::new(Vec::new());
    let mut nics = Nics::new(device, &mut sockets, clock.now());
    reply(chan, REP_IFADDRS, 0, 0, nics.ifaddrs_text().as_bytes());
    let mut convs = [Conv::EMPTY; MAX_CONV];
    let mut handshakes: Vec<Handshake> = Vec::new();
    let mut lingering: Vec<Lingering> = Vec::new();
    let mut local_ports = LOCAL_PORT_BASE;
    let mut ticks: u32 = 0;
    if !nics.list.is_empty() {
        poll_dhcp(&mut nics, &mut lo, &mut sockets, chan, &mut clock);
    }

    // Daemon poll: nic, /dev/netd requests, sockets. Bound work per tick.
    loop {
        clock.bump(TICK_MS);
        let now = clock.now();
        nics.rescan(now, &mut sockets, &mut lo, chan);
        let rx_before = nics.rx_frames();
        nics.poll(now, &mut sockets);
        poll_loopback(&mut lo, &mut lodev, &mut sockets, now, &convs, chan);
        nics.dhcp(&mut sockets, &mut lo, chan);
        nics.slaac(now, &mut lo, chan);

        let mut req = [0u8; FILE_IO];
        // Drain a few queued reqs per tick (ring is 8); 0 = nothing ready.
        let mut got_req = false;
        for _ in 0..8 {
            let n = read(chan, &mut req);
            if n == 0 || n == usize::MAX {
                break;
            }
            got_req = true;
            handle_req(
                &mut convs,
                &mut sockets,
                &mut handshakes,
                &mut lingering,
                &mut nics,
                &mut lo,
                chan,
                &req[..n],
                &mut local_ports,
            );
        }

        // Push any newly queued TCP segments (e.g. TLS ClientHello) out the NIC
        // in this same tick instead of waiting for the next VirtualInstant bump.
        if got_req {
            let now = clock.now();
            nics.poll(now, &mut sockets);
            poll_loopback(&mut lo, &mut lodev, &mut sockets, now, &convs, chan);
        }

        ticks += 1;
        // Reclaim *finished* closes before accept so a free slot is available
        // when a SYN lands (riscv SSH was starving: pump_accepts ran first,
        // MAX_CONV full of hungup/closing orphans).
        for i in 0..MAX_CONV {
            if !convs[i].closing && !convs[i].closing_after_flush {
                continue;
            }
            let closed = match convs[i].handle {
                Some(h) => matches!(
                    sockets.get_mut::<tcp::Socket>(h).state(),
                    tcp::State::Closed
                ),
                None => true,
            };
            if closed {
                if let Some(h) = convs[i].handle.take() {
                    sockets.remove(h);
                }
                retire(&mut convs, i);
                continue;
            }
            match convs[i].closing_since {
                None => convs[i].closing_since = Some(now),
                Some(since) if now - since >= CLOSE_GRACE => {
                    // The peer never let the close end: give up on it, as
                    // Linux does on an orphaned connection.
                    if let Some(h) = convs[i].handle.take() {
                        sockets.get_mut::<tcp::Socket>(h).abort();
                        sockets.remove(h);
                    }
                    retire(&mut convs, i);
                }
                Some(_) => {}
            }
        }
        sweep_lingering(&mut sockets, &mut lingering, now);
        // Handoff + deliver BEFORE orphan/hungup destroy. #161's accept queue
        // hands off Established with connected=false; if the peer
        // already pushed payload+FIN, iface.poll leaves CloseWait with RX data.
        // Orphan reclaim used to run *before* pump_sockets and treat CloseWait
        // as dead → abort discarded the bytes → userspace hangup / "no data".
        pump_accepts(&mut convs, &mut sockets, &mut handshakes, chan, now);
        pump_sockets(&mut convs, &mut sockets, chan);
        // Push segments enqueued by pump (pending flush / ACKs) same tick.
        {
            let now = clock.now();
            nics.poll(now, &mut sockets);
            poll_loopback(&mut lo, &mut lodev, &mut sockets, now, &convs, chan);
        }
        // A connection its listener's user has still to take stays, ended
        // by its peer or not: accept returns it and its user reads what came
        // and the end, as on Linux. Dropped here, the listen smoke's last
        // connection (its "done", then the host's close) was lost while the
        // smoke still took the ones before it, and it waited forever
        // (issue #630).
        for i in 0..MAX_CONV {
            if convs[i].hungup
                && !convs[i].closing
                && !convs[i].closing_after_flush
                && convs[i].listen_port == 0
                && !matches!(convs[i].kind, Kind::Empty)
                && !queued(&convs, i)
            {
                drain_tcp_rx_into_rep(&mut convs, &mut sockets, chan, i);
                drop_conv(&mut convs, &mut sockets, i);
            }
        }
        // Accepted connections nobody took that died: only after pump had a
        // chance to set `connected` and drain RX. Only accept-originated
        // slots: client clones start Closed until connect() and must not be
        // force-freed (broke aarch64 socket_smoke).
        for i in 0..MAX_CONV {
            if !matches!(convs[i].kind, Kind::Tcp) || convs[i].listen_port != 0 {
                continue;
            }
            if !convs[i].from_accept || convs[i].held {
                continue;
            }
            if convs[i].connected
                || convs[i].closing
                || convs[i].closing_after_flush
                || convs[i].hungup
            {
                continue;
            }
            let dead = match convs[i].handle {
                None => true,
                Some(h) => {
                    let st = sockets.get_mut::<tcp::Socket>(h).state();
                    match st {
                        // Closed/TimeWait: safe to free immediately.
                        tcp::State::Closed | tcp::State::TimeWait => true,
                        // Half-closed with possible RX: age so pump_sockets has
                        // at least one tick after handoff to deliver REP_DATA.
                        tcp::State::CloseWait
                        | tcp::State::LastAck
                        | tcp::State::Closing
                        | tcp::State::FinWait1
                        | tcp::State::FinWait2 => {
                            convs[i].ident = convs[i].ident.saturating_add(1);
                            convs[i].ident >= 2
                        }
                        _ => false,
                    }
                }
            };
            if dead {
                force_free_orphan(&mut convs, &mut sockets, chan, i);
            }
        }

        if now - convs_text_at >= Duration::from_secs(1) {
            convs_text_at = now;
            let text = convs_text(&convs, &sockets, &handshakes, &lingering, now);
            if text != convs_text_last {
                // In pieces of what one reply carries, `status` the piece's
                // offset: netfs starts the table over at 0 (a full table's
                // text is 14 KiB).
                let b = text.as_bytes();
                let mut off = 0;
                loop {
                    let n = (b.len() - off).min(REP_CAP - REP_HDR);
                    reply(chan, REP_CONVS, 0, off as i32, &b[off..off + n]);
                    off += n;
                    if off >= b.len() {
                        break;
                    }
                }
                convs_text_last = text;
            }
        }

        // Idle? Sleep until the next kernel event (a request on the channel,
        // console/pipe traffic) or a short bound, instead of spinning: netd
        // used to pin a CPU at 100% forever. Frames or requests seen this
        // round mean the NIC/peer is active, so poll again at once.
        let rx_now = nics.rx_frames();
        // Received bytes netfs has room for (its credit came back while
        // the socket still held some): deliver them next pass, no sleep.
        let backlog = convs.iter().any(|c| {
            matches!(c.kind, Kind::Tcp)
                && c.rx_room != 0
                && c.handle.is_some_and(|h| sockets.get::<tcp::Socket>(h).can_recv())
        });
        if !got_req && !backlog && rx_now == rx_before && !lodev.pending() {
            let active = convs
                .iter()
                .any(|c| !matches!(c.kind, Kind::Empty) && c.listen_port == 0);
            // RX interrupts on every NIC? (`irq on` in its ctl.) Not
            // announced on the console: netd starts around the `login:`
            // prompt and a line there confuses serial-driven harnesses;
            // `/proc/interrupts` shows it.
            let rx_irq = nics.rx_irq();
            let mut ns = if rx_irq {
                IDLE_WAIT_IRQ_NS
            } else if active {
                IDLE_SLEEP_ACTIVE_NS
            } else {
                IDLE_SLEEP_QUIET_NS
            };
            for d in [nics.poll_delay(clock.now(), &sockets), lo.poll_delay(clock.now(), &sockets)]
                .into_iter()
                .flatten()
            {
                let d_ns = (d.total_micros() as u64).saturating_mul(1000);
                if d_ns < ns {
                    ns = d_ns.max(100_000);
                }
            }
            if rx_irq {
                // Sleep until a frame (the NIC's RX interrupt wakes the
                // pollers; the kernel checks the ring again before it
                // sleeps, so one that landed just before is not missed) or a
                // request on the channel, at most until the next timer.
                let ms = ns.div_ceil(1_000_000).clamp(1, i32::MAX as u64) as i32;
                let mut fds: Vec<PollFd> = nics.poll_fds().collect();
                fds.push(PollFd { fd: chan as i32, events: POLLIN, revents: 0 });
                let _ = poll(&mut fds, ms);
            } else {
                myos_user::sleep_ns(ns, true);
            }
        }
    }
}

/// Poll the loopback interface until what it sent came back round (a
/// local exchange: a datagram and its ICMP error, a TCP handshake), within
/// a bound; then report the refused datagrams to the connected UDP convs
/// that sent them.
fn poll_loopback(
    lo: &mut Interface,
    dev: &mut LoopDevice,
    sockets: &mut SocketSet<'_>,
    now: Instant,
    convs: &[Conv; MAX_CONV],
    chan: usize,
) {
    for _ in 0..16 {
        lo.poll(now, dev, sockets);
        if !dev.pending() {
            break;
        }
    }
    for r in dev.refused.drain(..) {
        for (i, c) in convs.iter().enumerate() {
            if matches!(c.kind, Kind::Udp)
                && c.have_remote
                && (c.local_port, c.remote, c.remote_port) == (r.src_port, r.dst, r.dst_port)
            {
                reply(chan, REP_SOERR, i as u16, 0, b"refused");
            }
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
