#![no_std]
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
use myos_user::{close, heap_init, open_flags, read, write, write_fd, Heap, O_RDWR};

#[global_allocator]
static GLOBAL: Heap = Heap;

const PROTO_TCP: u8 = 1;
const PROTO_UDP: u8 = 2;
const PROTO_ICMP: u8 = 3;

const REQ_CLONE: u8 = 1;
const REQ_CTL: u8 = 2;
const REQ_SEND: u8 = 3;
const REQ_CLOSE: u8 = 4;

const REP_CLONE_OK: u8 = 1;
const REP_DATA: u8 = 2;
const REP_STATUS: u8 = 3;
const REP_ERR: u8 = 4;



const REQ_HDR: usize = 6;
const REP_HDR: usize = 9;
const MSG_CAP: usize = 2048;
const MAX_CONV: usize = 16;
const FILE_IO: usize = 2048;
const DHCP_POLLS: usize = 3000;
const TICK_MS: u64 = 100;

const ICMP_IDENT_BASE: u16 = 0x22b;
const TCP_RX: usize = 4096;
const TCP_TX: usize = 4096;
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
    /// REQ_SEND payload deferred when the smoltcp socket could not accept it.
    /// Without this, netfs already reported write success and the bytes vanish.
    pending_len: u16,
    pending: [u8; MSG_CAP],
    /// TCP listener (Plan 9 "announce"): nonzero = advertised port.
    listen_port: u16,
    /// A ctl "accept" arrived and is parked until a connection lands.
    accept_wait: bool,
    /// Accepted connection parked for the next ctl "accept": new conv id.
    accepted: Option<u16>,
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
        pending_len: 0,
        pending: [0; MSG_CAP],
        listen_port: 0,
        accept_wait: false,
        accepted: None,
        accept_seq: 0,
        closing: false,
        closing_after_flush: false,
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
    open_flags(b"/dev/netd", O_RDWR)
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
                    write(b"netd: no /dev/netd\n");
                    printed_netd = true;
                }
            } else if !printed_net0 {
                write(b"netd: no /dev/net0\n");
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
        iface.poll(now, device, sockets);
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



fn drop_conv(convs: &mut [Conv; MAX_CONV], sockets: &mut SocketSet<'_>, i: usize) {
    if i >= MAX_CONV {
        return;
    }
    if matches!(convs[i].kind, Kind::Tcp) && convs[i].listen_port != 0 {
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
            if convs[i].pending_len > 0 {
                let s = sockets.get_mut::<tcp::Socket>(h);
                if s.can_send() {
                    let n = convs[i].pending_len as usize;
                    if s.send_slice(&convs[i].pending[..n]).is_ok() {
                        convs[i].pending_len = 0;
                    }
                }
            }
            if convs[i].pending_len > 0 {
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
    // Accept a pending connection on this listener. One parked wait max.
    if cmd == b"accept" {
        match convs[i].kind {
            Kind::Tcp if convs[i].listen_port != 0 => {}
            _ => {
                reply(chan, REP_ERR, conv, -1, b"not listening");
                return;
            }
        }
        if let Some(n) = convs[i].accepted.take() {
            let seq = convs[i].accept_seq;
            let rep = accept_reply(convs, n, seq);
            reply(chan, REP_STATUS, conv, 0, &rep);
            return;
        }
        if convs[i].accept_wait {
            reply(chan, REP_ERR, conv, -1, b"accept busy");
            return;
        }
        // A select()ing server keys listener readiness off the status file.
        // Clear any stale "accepted <old>" so it does not re-accept the
        // previous connection; status flips back to "accepted <new>" only
        // when pump_accepts parks a fresh connection.
        reply(chan, REP_STATUS, conv, 0, b"listening");
        convs[i].accept_wait = true; // parked; replied from the poll pump
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
fn pump_accepts(
    convs: &mut [Conv; MAX_CONV],
    sockets: &mut SocketSet<'_>,
    chan: usize,
) {
    for i in 0..MAX_CONV {
        if !matches!(convs[i].kind, Kind::Tcp) || convs[i].listen_port == 0 {
            continue;
        }
        let Some(h) = convs[i].handle else {
            continue;
        };
        let arrived = {
            let s = sockets.get_mut::<tcp::Socket>(h);
            let st = s.state();
            !matches!(st, tcp::State::Listen | tcp::State::Closed)
        };
        if !arrived {
            // Retry a re-arm listener that failed to bind (stuck Closed).
            let s = sockets.get_mut::<tcp::Socket>(h);
            if s.state() == tcp::State::Closed {
                match s.listen(convs[i].listen_port) {
                    Ok(()) => {}
                    Err(_) => {}
                }
            }
            continue;
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
        // Move the connected socket to a free conv.
        let slot = (0..MAX_CONV)
            .find(|&n| matches!(convs[n].kind, Kind::Empty) && n != i);
        let Some(n) = slot else {
            // Backlog full: hold the connection in the listener socket until
            // a conv frees up (checked again next tick).
            if convs[i].accept_wait {
                // Only fail a parked wait on real exhaustion; keep waiting.
                continue;
            }
            continue;
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
        convs[i].accepted = Some(n as u16);
        convs[i].accept_seq = convs[i].accept_seq.wrapping_add(1);
        if convs[i].accept_wait {
            convs[i].accept_wait = false;
            let seq = convs[i].accept_seq;
            let rep = accept_reply(convs, n as u16, seq);
            reply(chan, REP_STATUS, i as u16, 0, &rep);
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
            let s = sockets.get_mut::<tcp::Socket>(h);
            if s.can_send() {
                match s.send_slice(payload) {
                    Ok(n) if n < payload.len() => {
                        // Partial accept — stash the rest for pump_sockets.
                        let rest = &payload[n..];
                        let m = rest.len().min(MSG_CAP);
                        convs[i].pending[..m].copy_from_slice(&rest[..m]);
                        convs[i].pending_len = m as u16;
                    }
                    Ok(_) => {
                        convs[i].pending_len = 0;
                    }
                    Err(_) => {
                        let m = payload.len().min(MSG_CAP);
                        convs[i].pending[..m].copy_from_slice(&payload[..m]);
                        convs[i].pending_len = m as u16;
                    }
                }
            } else {
                let m = payload.len().min(MSG_CAP);
                convs[i].pending[..m].copy_from_slice(&payload[..m]);
                convs[i].pending_len = m as u16;
            }
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
                let pend_len = convs[i].pending_len as usize;
                if pend_len != 0 {
                    let mut tmp_pend = [0u8; MSG_CAP];
                    tmp_pend[..pend_len].copy_from_slice(&convs[i].pending[..pend_len]);
                    let s = sockets.get_mut::<tcp::Socket>(h);
                    if s.can_send() {
                        match s.send_slice(&tmp_pend[..pend_len]) {
                            Ok(n) if n >= pend_len => {
                                convs[i].pending_len = 0;
                            }
                            Ok(n) if n > 0 => {
                                let left = pend_len - n;
                                convs[i].pending.copy_within(n..pend_len, 0);
                                convs[i].pending_len = left as u16;
                            }
                            _ => {}
                        }
                    }
                }
                // Deferred close (drop_conv couldn't flush pending yet):
                // pending fully queued now -> finish the graceful close.
                if convs[i].closing_after_flush && convs[i].pending_len == 0 {
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
                if s.can_recv() {
                    let mut tmp = [0u8; 1400];
                    if let Ok(n) = s.recv_slice(&mut tmp) {
                        if n != 0 {
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

    // Daemon poll: nic, /dev/netd requests, sockets. Bound work per tick.
    loop {
        clock.bump(TICK_MS);
        let now = clock.now();
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
        pump_accepts(&mut convs, &mut sockets, chan);
        pump_sockets(&mut convs, &mut sockets, &device, chan);
        // Finish graceful closes: once a closing socket reaches Closed, free it.
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
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
