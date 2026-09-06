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
const MAX_CONV: usize = 8;
const FILE_IO: usize = 2048;
const DHCP_POLLS: usize = 3000;
const TICK_MS: u64 = 100;
const ICMP_IDENT_BASE: u16 = 0x22b;
const TCP_RX: usize = 4096;
const TCP_TX: usize = 4096;
const UDP_BUF: usize = 512;

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
    if let Some(h) = convs[i].handle.take() {
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
) {
    let i = conv as usize;
    if i >= MAX_CONV {
        reply(chan, REP_ERR, conv, -1, b"no conv");
        return;
    }
    let cmd = trim(payload);
    if cmd == b"hangup" {
        drop_conv(convs, sockets, i);
        reply(chan, REP_STATUS, conv, 0, b"hangup");
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
                    let local = 49152u16.wrapping_add(conv);
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
            let local = 49152u16.wrapping_add(conv);
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
                let s = sockets.get_mut::<tcp::Socket>(h);
                if s.is_active() && s.may_send() && !convs[i].connected && !convs[i].hungup {
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
                // Peer FIN / inactive: surface hangup so blocking recv gets EOF.
                // CloseWait with drained RX has may_recv()==false; Closed is !active.
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
        REQ_CTL => handle_ctl(convs, sockets, iface, chan, conv, payload),
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
            );
        }

        // Push any newly queued TCP segments (e.g. TLS ClientHello) out the NIC
        // in this same tick instead of waiting for the next VirtualInstant bump.
        if got_req {
            let now = clock.now();
            iface.poll(now, &mut device, &mut sockets);
        }

        pump_sockets(&mut convs, &mut sockets, &device, chan);
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
