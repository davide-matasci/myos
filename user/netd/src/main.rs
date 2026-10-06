#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![no_main]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use myos_net::smoltcp::iface::{Interface, SocketHandle, SocketSet};
use myos_net::smoltcp::phy::Device;
use myos_net::smoltcp::socket::{dhcpv4, icmp, tcp, udp};
use myos_net::smoltcp::wire::{
    Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, IpEndpoint, Ipv4Address,
};
use myos_net::{build_interface, Net0Device, VirtualInstant};
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



const REQ_HDR: usize = 6;
const REP_HDR: usize = 9;
const MSG_CAP: usize = 2048;
const MAX_CONV: usize = 32;
const FILE_IO: usize = 2048;
const DHCP_POLLS: usize = 3000;
/// Minimum clock advance per poll when `gettimeofday` is unavailable; the
/// stack clock otherwise follows wall time (`VirtualInstant::bump`).
const TICK_MS: u64 = 1;
/// Idle sleeps between polls when the NIC has no RX interrupt (poll-mode
/// fallback): a bounded sleep sets the worst-case RX latency; kernel events
/// (channel requests from netfs, pipe/console traffic) end the sleep early.
const IDLE_SLEEP_ACTIVE_NS: u64 = 2_000_000;
const IDLE_SLEEP_QUIET_NS: u64 = 10_000_000;
/// With RX interrupts the idle wait only needs a backstop (smoltcp's own
/// `poll_delay` cuts it shorter when a timer is due).
const IDLE_WAIT_IRQ_NS: u64 = 1_000_000_000;

const ICMP_IDENT_BASE: u16 = 0x22b;
const TCP_RX: usize = 4096;
const TCP_TX: usize = 4096;
/// netfs's per-conv receive buffer (modules/netfs DATA_CAP). It drops what
/// does not fit, so netd never has more than this in flight to it.
const NETFS_RX_CAP: u16 = 8192;
const UDP_BUF: usize = 512;
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
    remote4: Ipv4Address,
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
    /// Created by pump_accepts (incoming). Orphan reclaim must only touch
    /// these: client clones start in TCP Closed and would be freed before
    /// connect() (aarch64 socket_smoke connect fail on tip 0b4444e).
    from_accept: bool,
}

impl Conv {
    const EMPTY: Self = Self {
        kind: Kind::Empty,
        handle: None,
        remote4: Ipv4Address::new(0, 0, 0, 0),
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
        from_accept: false,
    };
}

fn parse_u8(s: &[u8]) -> Option<(u8, usize)> {
    if s.is_empty() || !s[0].is_ascii_digit() {
        return None;
    }
    let mut n = 0u32;
    let mut i = 0usize;
    while i < s.len() && s[i].is_ascii_digit() {
        n = n.checked_mul(10)?.checked_add((s[i] - b'0') as u32)?;
        i += 1;
        if n > 255 {
            return None;
        }
    }
    Some((n as u8, i))
}

fn parse_ipv4(s: &[u8]) -> Option<(Ipv4Address, usize)> {
    let mut o = 0usize;
    let mut oct = [0u8; 4];
    for i in 0..4 {
        let (v, n) = parse_u8(&s[o..])?;
        oct[i] = v;
        o += n;
        if i != 3 {
            if o >= s.len() || s[o] != b'.' {
                return None;
            }
            o += 1;
        }
    }
    Some((Ipv4Address::new(oct[0], oct[1], oct[2], oct[3]), o))
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

/// `connect 10.0.2.2` or `connect 10.0.2.2!80`
fn parse_connect(cmd: &[u8]) -> Option<(Ipv4Address, Option<u16>)> {
    let rest = cmd.strip_prefix(b"connect")?;
    let rest = trim(rest);
    let (addr, n) = parse_ipv4(rest)?;
    let rest = &rest[n..];
    if rest.is_empty() {
        return Some((addr, None));
    }
    if rest[0] != b'!' {
        return None;
    }
    let port = parse_port(&rest[1..])?;
    Some((addr, Some(port)))
}

/// Status payload for an accepted connection:
/// "accepted <N> <ip>!<port> <seq>".
/// Hand-rolled decimal formatting keeps the image free of fmt/memcpy deps.
fn accept_reply(convs: &mut [Conv; MAX_CONV], n: u16, seq: u32) -> alloc::vec::Vec<u8> {
    fn push_dec(out: &mut alloc::vec::Vec<u8>, mut v: u32) {
        let mut tmp = [0u8; 10];
        let mut i = 0;
        if v == 0 {
            out.push(b'0');
            return;
        }
        while v > 0 {
            tmp[i] = b'0' + (v % 10) as u8;
            v /= 10;
            i += 1;
        }
        while i > 0 {
            i -= 1;
            out.push(tmp[i]);
        }
    }
    let mut out = alloc::vec::Vec::with_capacity(32);
    out.extend_from_slice(b"accepted ");
    push_dec(&mut out, n as u32);
    let c = &convs[n as usize];
    if c.have_remote {
        let o = c.remote4.octets();
        out.push(b' ');
        for (k, b) in o.iter().enumerate() {
            if k > 0 {
                out.push(b'.');
            }
            push_dec(&mut out, *b as u32);
        }
        out.push(b'!');
        push_dec(&mut out, c.remote_port as u32);
    }
    out.push(b' ');
    push_dec(&mut out, seq);
    out
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
    let mut tmp = [0u8; MSG_CAP];
    let Some(n) = encode_rep(typ, conv, status, payload, &mut tmp) else {
        return;
    };
    let _ = write_fd(fd, &tmp[..n]);
}

fn open_chan() -> Option<usize> {
    open_flags(b"/dev/netd/data", O_RDWR)
}

fn wait_devices() -> (Net0Device, usize) {
    let mut printed_net0 = false;
    let mut printed_netd = false;
    // Bound inner tries so a missing chrdev does not look like a hang; keep
    // retrying (daemon) without panicking the kernel.
    loop {
        for _ in 0..4096 {
            if let Some(dev) = Net0Device::open() {
                if let Some(fd) = open_chan() {
                    return (dev, fd);
                }
                close(dev.fd());
                if !printed_netd {
                    write(b"netd: no /dev/netd/data\n");
                    printed_netd = true;
                }
            } else if !printed_net0 {
                write(b"netd: no /dev/net0/data\n");
                printed_net0 = true;
            }
        }
    }
}

fn poll_dhcp(
    iface: &mut Interface,
    device: &mut Net0Device,
    sockets: &mut SocketSet<'_>,
    dhcp: SocketHandle,
    clock: &mut VirtualInstant,
) -> bool {
    for _ in 0..DHCP_POLLS {
        clock.bump(TICK_MS);
        let now = clock.now();
        let rx_before = device.rx_frames();
        iface.poll(now, device, sockets);
        if device.rx_frames() == rx_before {
            // Nothing arrived: wait a little instead of spinning on the NIC.
            myos_user::sleep_ns(1_000_000, false);
        }
        match sockets.get_mut::<dhcpv4::Socket>(dhcp).poll() {
            Some(dhcpv4::Event::Configured(cfg)) => {
                iface.update_ip_addrs(|addrs| {
                    addrs.clear();
                    let _ = addrs.push(IpCidr::Ipv4(cfg.address));
                });
                if let Some(router) = cfg.router {
                    let _ = iface.routes_mut().add_default_ipv4_route(router);
                } else {
                    iface.routes_mut().remove_default_ipv4_route();
                }
                return true;
            }
            Some(dhcpv4::Event::Deconfigured) => {
                iface.update_ip_addrs(|addrs| addrs.clear());
                iface.routes_mut().remove_default_ipv4_route();
            }
            None => {}
        }
    }
    false
}

fn handle_clone(convs: &mut [Conv; MAX_CONV], sockets: &mut SocketSet<'_>, proto: u8, conv: u16) {
    let i = conv as usize;
    if i >= MAX_CONV {
        return;
    }
    drop_conv(convs, sockets, i);
    match proto {
        PROTO_ICMP => {
            let rx = icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY], vec![0; 256]);
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
            let rx = udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 2], vec![0; UDP_BUF]);
            let tx = udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 2], vec![0; UDP_BUF]);
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
    // Listeners must stay up across client hangups / forked closes. Empty
    // body was a bug (listen sockets were destroyed on ctl hangup).
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
            // A REQ_SEND may have been stashed while the handshake was still
            // completing (accept returns on SynReceived). Flush what we can
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
    convs[i] = Conv::EMPTY;
}

fn handle_ctl(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    iface: &mut Interface,
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
    // Plan 9 announce: start a TCP listener on this conv ("announce <port>").
    if let Some(rest) = cmd.strip_prefix(b"announce") {
        let port = match parse_port(trim(rest)) {
            Some(p) if p != 0 => p,
            _ => {
                reply(chan, REP_ERR, conv, -1, b"need port");
                return;
            }
        };
        match convs[i].kind {
            Kind::Tcp => {
                let Some(h) = convs[i].handle else {
                    reply(chan, REP_ERR, conv, -1, b"no conv");
                    return;
                };
                let s = sockets.get_mut::<tcp::Socket>(h);
                match s.listen(port) {
                    Ok(()) => {
                        convs[i].listen_port = port;
                        reply(chan, REP_STATUS, conv, 0, b"listening");
                    }
                    Err(_) => reply(chan, REP_ERR, conv, -1, b"listen"),
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
            convs[i].remote4 = addr;
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
        Kind::Udp => {
            let p = match port {
                Some(p) => p,
                None => {
                    reply(chan, REP_ERR, conv, -1, b"need port");
                    return;
                }
            };
            convs[i].remote4 = addr;
            convs[i].remote_port = p;
            convs[i].have_remote = true;
            if let Some(h) = convs[i].handle {
                let s = sockets.get_mut::<udp::Socket>(h);
                if !s.is_open() {
                    let local = next_local_port(local_ports);
                    let _ = s.bind(local);
                }
            }
            convs[i].connected = true;
            reply(chan, REP_STATUS, conv, 0, b"connected");
        }
        Kind::Tcp => {
            let p = match port {
                Some(p) => p,
                None => {
                    reply(chan, REP_ERR, conv, -1, b"need port");
                    return;
                }
            };
            convs[i].remote4 = addr;
            convs[i].remote_port = p;
            convs[i].have_remote = true;
            let local = next_local_port(local_ports);
            let remote = IpEndpoint::new(IpAddress::Ipv4(addr), p);
            if let Some(h) = convs[i].handle {
                let s = sockets.get_mut::<tcp::Socket>(h);
                match s.connect(iface.context(), remote, local) {
                    Ok(()) => reply(chan, REP_STATUS, conv, 0, b"connecting"),
                    Err(_) => reply(chan, REP_ERR, conv, -1, b"tcp connect"),
                }
            }
        }
    }
}

/// Poll TCP listeners: when smoltcp moved a listening socket out of the
/// Listen state, an incoming connection landed. Hand it to a fresh conv,
/// re-arm a listener on the same port, and complete any parked "accept".
fn rearm_listener(convs: &mut [Conv; MAX_CONV], sockets: &mut SocketSet<'_>, i: usize) {
    let port = convs[i].listen_port;
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
    {
        let ls = sockets.get_mut::<tcp::Socket>(nh);
        let _ = ls.listen(port);
    }
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
    let mut tmp = [0u8; 1400];
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

fn pump_accepts(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    chan: usize,
) {
    for i in 0..MAX_CONV {
        if !matches!(convs[i].kind, Kind::Tcp) || convs[i].listen_port == 0 {
            continue;
        }
        // Stale park: a hangup drop emptied a slot but left a queue pointer.
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
        let Some(h) = convs[i].handle else {
            continue;
        };
        let st = sockets.get_mut::<tcp::Socket>(h).state();
        // Only SynReceived/Established are real accept handoffs. Peer-aborted
        // sockets left on the listen handle (CloseWait/FinWait/…) used to look
        // like "arrived" and were handed to dropbear as Child + write EIO.
        match st {
            tcp::State::Listen => continue,
            tcp::State::Closed => {
                let port = convs[i].listen_port;
                let s = sockets.get_mut::<tcp::Socket>(h);
                let _ = s.listen(port);
                continue;
            }
            tcp::State::SynReceived | tcp::State::Established => {}
            _ => {
                rearm_listener(convs, sockets, i);
                continue;
            }
        }
        // Peer endpoint captured from the connected socket.
        let (remote4, remote_port) = {
            let s = sockets.get_mut::<tcp::Socket>(h);
            match s.remote_endpoint() {
                Some(e) => match e.addr {
                    IpAddress::Ipv4(a) => (a, e.port),
                    _ => (Ipv4Address::new(0, 0, 0, 0), 0),
                },
                None => (Ipv4Address::new(0, 0, 0, 0), 0),
            }
        };
        // Park up to two handoffs. Prefer this over holding a SYN on the
        // listen handle (no Listen socket + former pump_sockets wedge).
        // A third concurrent connection is dropped + re-armed below rather
        // than parked on-listen (pump_sockets skips such sockets).
        if convs[i].accepted.is_some() && convs[i].accepted_pending.is_some() {
            // Accept queue full (both handoff slots parked, dropbear decoding
            // sessions): drop this extra connection and re-arm Listen. Holding
            // it on the listener socket instead would leave no Listen socket
            // and silently drop every later SYN (the CI #1164 port-:22 stall).
            rearm_listener(convs, sockets, i);
            convs[i].ident = 0;
            continue;
        }
        // Move the connected socket to a free conv.
        let slot = (0..MAX_CONV)
            .find(|&n| matches!(convs[n].kind, Kind::Empty) && n != i);
        let n = match slot {
            Some(n) => n,
            None => {
                // No free slot. Do NOT park the fresh connection on the
                // listener: holding a connected socket there leaves no Listen
                // socket and silently drops every later SYN until the stale
                // half-open ages out on its own (minutes). That wedged the
                // riscv64 SSH smoke's port :22 for the whole stage (CI #1164).
                // Reclaim the oldest handshake-incomplete accept orphan to free
                // a slot; if every slot is a live session, abort this incoming
                // connection and re-arm Listen so the port stays answerable.
                let victim = (0..MAX_CONV).find(|&n| {
                    n != i
                        && convs[n].from_accept
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
                        if convs[i].accepted == Some(v as u16) {
                            convs[i].accepted = convs[i].accepted_pending.take();
                        }
                        if convs[i].accepted_pending == Some(v as u16) {
                            convs[i].accepted_pending = None;
                        }
                        v
                    }
                    None => {
                        rearm_listener(convs, sockets, i);
                        convs[i].ident = 0;
                        continue;
                    }
                }
            }
        };
        let port = convs[i].listen_port;
        convs[n] = Conv {
            kind: Kind::Tcp,
            handle: Some(h),
            remote4,
            remote_port,
            have_remote: true,
            // Do NOT mark connected here: at handoff the socket may still be
            // SynReceived (may_recv()==false). pump_sockets owns the
            // connected flag and sets it only once Established; marking it now
            // made the hangup check fire immediately (may_recv()==false) and
            // tore the conv down before the server could send its banner.
            connected: false,
            from_accept: true,
            ..Conv::EMPTY
        };
        // Tell netfs about the new conv: it only allocates convs on clone, so
        // without this the guest cannot open /net/tcp/<n>/data (conv_ok
        // fails) and accept() dies with EIO.
        // Tell netfs about the new conv exactly once: pump-accepted convs
        // bypass clone, so without this netfs never marks the slot used and
        // the client's open of /net/tcp/<n>/data fails.
        reply(chan, REP_CLONE_OK, n as u16, 0, b"tcp");
        // Re-arm a fresh listener on the same port for the next connection.
        let rx = tcp::SocketBuffer::new(vec![0; TCP_RX]);
        let tx = tcp::SocketBuffer::new(vec![0; TCP_TX]);
        let nh = sockets.add(tcp::Socket::new(rx, tx));
        {
            let ls = sockets.get_mut::<tcp::Socket>(nh);
            match ls.listen(port) {
                Ok(()) => {}
                Err(_) => {}
            }
        }
        convs[i].handle = Some(nh);
        convs[i].connected = false;
        convs[i].hungup = false;
        if convs[i].accepted.is_none() {
            // Publish even when no accept is parked: libgloss arms only once
            // per consumed handoff, and a lost/late arm must not hide this
            // connection behind a stale "listening".
            publish_head(convs, chan, i, Some(n as u16));
        } else {
            // Head still parked; queue second without bumping head seq/status.
            convs[i].accepted_pending = Some(n as u16);
        }
    }
}

fn handle_send(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    device: &Net0Device,
    chan: usize,
    conv: u16,
    payload: &[u8],
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
            let dst = IpAddress::Ipv4(convs[i].remote4);
            let checksum = device.capabilities().checksum;
            let s = sockets.get_mut::<icmp::Socket>(h);
            if !s.is_open() {
                let _ = s.bind(icmp::Endpoint::Ident(ident));
            }
            if !s.can_send() {
                return;
            }
            let echo = Icmpv4Repr::EchoRequest {
                ident,
                seq_no: seq,
                data: payload,
            };
            if let Ok(buf) = s.send(echo.buffer_len(), dst) {
                let mut pkt = Icmpv4Packet::new_unchecked(buf);
                echo.emit(&mut pkt, &checksum);
            }
        }
        Kind::Udp => {
            if !convs[i].have_remote {
                reply(chan, REP_ERR, conv, -1, b"not connected");
                return;
            }
            let Some(h) = convs[i].handle else {
                return;
            };
            let ep = IpEndpoint::new(IpAddress::Ipv4(convs[i].remote4), convs[i].remote_port);
            let s = sockets.get_mut::<udp::Socket>(h);
            if s.can_send() {
                let _ = s.send_slice(payload, ep);
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
    device: &Net0Device,
    chan: usize,
) {
    let checksum = device.capabilities().checksum;
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
                    if let Ok((payload, _)) = s.recv() {
                        if let Ok(pkt) = Icmpv4Packet::new_checked(payload) {
                            if let Ok(Icmpv4Repr::EchoReply { data, .. }) =
                                Icmpv4Repr::parse(&pkt, &checksum)
                            {
                                reply(chan, REP_DATA, conv, 0, data);
                            }
                        }
                    }
                }
            }
            Kind::Udp => {
                let Some(h) = convs[i].handle else {
                    continue;
                };
                let s = sockets.get_mut::<udp::Socket>(h);
                if s.can_recv() {
                    if let Ok((payload, _meta)) = s.recv() {
                        reply(chan, REP_DATA, conv, 0, payload);
                    }
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
                let room = (convs[i].rx_room as usize).min(1400);
                if s.can_recv() && room != 0 {
                    let mut tmp = [0u8; 1400];
                    if let Ok(n) = s.recv_slice(&mut tmp[..room]) {
                        if n != 0 {
                            convs[i].rx_room -= n as u16;
                            reply(chan, REP_DATA, conv, 0, &tmp[..n]);
                        }
                    }
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
    iface: &mut Interface,
    device: &Net0Device,
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
            handle_clone(convs, sockets, proto, conv);
            reply(chan, REP_CLONE_OK, conv, 0, &[]);
        }
        REQ_CTL => handle_ctl(convs, sockets, iface, chan, conv, payload, local_ports),
        REQ_SEND => handle_send(convs, sockets, device, chan, conv, payload),
        REQ_CLOSE => {
            drop_conv(convs, sockets, conv as usize);
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
    let (mut device, chan) = wait_devices();

    let mut clock = VirtualInstant::new();
    let mut iface = build_interface(&mut device, clock.now());
    let mut sockets = SocketSet::new(Vec::new());
    let dhcp = sockets.add(dhcpv4::Socket::new());
    let mut convs = [Conv::EMPTY; MAX_CONV];
    let mut local_ports = LOCAL_PORT_BASE;
    let mut ticks: u32 = 0;
    let mut dhcp_ok = poll_dhcp(&mut iface, &mut device, &mut sockets, dhcp, &mut clock);
    // RX interrupts available? (`irq on` in the NIC's ctl.) Not announced on
    // the console: netd starts around the `login:` prompt and a line there
    // confuses serial-driven harnesses; `/proc/interrupts` shows it.
    let rx_irq = device.rx_irq();

    // Daemon poll: nic, /dev/netd requests, sockets. Bound work per tick.
    loop {
        clock.bump(TICK_MS);
        let now = clock.now();
        let rx_before = device.rx_frames();
        iface.poll(now, &mut device, &mut sockets);

        if !dhcp_ok {
            match sockets.get_mut::<dhcpv4::Socket>(dhcp).poll() {
                Some(dhcpv4::Event::Configured(cfg)) => {
                    iface.update_ip_addrs(|addrs| {
                        addrs.clear();
                        let _ = addrs.push(IpCidr::Ipv4(cfg.address));
                    });
                    if let Some(router) = cfg.router {
                        let _ = iface.routes_mut().add_default_ipv4_route(router);
                    } else {
                        iface.routes_mut().remove_default_ipv4_route();
                    }
                    dhcp_ok = true;
                }
                Some(dhcpv4::Event::Deconfigured) => {
                    iface.update_ip_addrs(|addrs| addrs.clear());
                    iface.routes_mut().remove_default_ipv4_route();
                    dhcp_ok = false;
                }
                None => {}
            }
        }

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
                &mut iface,
                &device,
                chan,
                &req[..n],
                &mut local_ports,
            );
        }

        // Push any newly queued TCP segments (e.g. TLS ClientHello) out the NIC
        // in this same tick instead of waiting for the next VirtualInstant bump.
        if got_req {
            let now = clock.now();
            iface.poll(now, &mut device, &mut sockets);
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
                convs[i] = Conv::EMPTY;
            }
        }
        // Handoff + deliver BEFORE orphan/hungup destroy. #161's accept queue
        // hands off SynReceived/Established with connected=false; if the peer
        // already pushed payload+FIN, iface.poll leaves CloseWait with RX data.
        // Orphan reclaim used to run *before* pump_sockets and treat CloseWait
        // as dead → abort discarded the bytes → userspace hangup / "no data".
        pump_accepts(&mut convs, &mut sockets, chan);
        pump_sockets(&mut convs, &mut sockets, &device, chan);
        // Push segments enqueued by pump (pending flush / ACKs) same tick.
        {
            let now = clock.now();
            iface.poll(now, &mut device, &mut sockets);
        }
        for i in 0..MAX_CONV {
            if convs[i].hungup
                && !convs[i].closing
                && !convs[i].closing_after_flush
                && convs[i].listen_port == 0
                && !matches!(convs[i].kind, Kind::Empty)
            {
                drain_tcp_rx_into_rep(&mut convs, &mut sockets, chan, i);
                drop_conv(&mut convs, &mut sockets, i);
            }
        }
        // Failed-handshake orphans: only after pump had a chance to set
        // `connected` and drain RX. SynReceived/SynSent that die still age out.
        // Only accept-originated slots: client clones start Closed until
        // connect() and must not be force-freed (broke aarch64 socket_smoke).
        for i in 0..MAX_CONV {
            if !matches!(convs[i].kind, Kind::Tcp) || convs[i].listen_port != 0 {
                continue;
            }
            if !convs[i].from_accept {
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
                        tcp::State::SynReceived | tcp::State::SynSent => {
                            // `ident` is ICMP-only; reuse as handshake age ticks.
                            convs[i].ident = convs[i].ident.saturating_add(1);
                            convs[i].ident >= 100 // ~10s at TICK_MS=100
                        }
                        _ => false,
                    }
                }
            };
            if dead {
                force_free_orphan(&mut convs, &mut sockets, chan, i);
            }
        }

        // Idle? Sleep until the next kernel event (a request on the channel,
        // console/pipe traffic) or a short bound, instead of spinning: netd
        // used to pin a CPU at 100% forever. Frames or requests seen this
        // round mean the NIC/peer is active, so poll again at once.
        let rx_now = device.rx_frames();
        if !got_req && rx_now == rx_before {
            let active = convs
                .iter()
                .any(|c| !matches!(c.kind, Kind::Empty) && c.listen_port == 0);
            let mut ns = if rx_irq {
                IDLE_WAIT_IRQ_NS
            } else if active {
                IDLE_SLEEP_ACTIVE_NS
            } else {
                IDLE_SLEEP_QUIET_NS
            };
            if let Some(d) = iface.poll_delay(clock.now(), &sockets) {
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
                let mut fds = [
                    PollFd { fd: device.fd() as i32, events: POLLIN, revents: 0 },
                    PollFd { fd: chan as i32, events: POLLIN, revents: 0 },
                ];
                let _ = poll(&mut fds, ms);
            } else {
                myos_user::sleep_ns(ns, true);
            }
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
