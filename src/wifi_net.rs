//! The network behind the simulated access points (`src/wifi_sim.rs`):
//! each one is a router [`ROUTER`]/24 (its own address, the BSSID, its
//! MAC) that leases a station 10.0.8.(1 + its association id) by DHCP,
//! answers ARP for its address and the pings sent to it. Enough for the
//! guest's netd to take the link and reach a host across it (`user/wifi`,
//! `docs/wifi.md`).

pub const ROUTER: [u8; 4] = [10, 0, 8, 1];
const ETH_IPV4: u16 = 0x0800;
const ETH_ARP: u16 = 0x0806;
const BROADCAST: [u8; 6] = [0xff; 6];

/// An Ethernet frame's parts, without the source (the router's).
pub struct Out {
    pub dst: [u8; 6],
    pub ethertype: u16,
    pub payload: Vec<u8>,
}

/// What the router `mac` answers station `sta` (association id `aid`)
/// sending it `payload` of `ethertype`.
pub fn answer(mac: [u8; 6], sta: [u8; 6], aid: u16, ethertype: u16, p: &[u8]) -> Option<Out> {
    match ethertype {
        ETH_ARP => arp(mac, p),
        ETH_IPV4 => ipv4(sta, aid, p),
        _ => None,
    }
}

/// A reply to an ARP request for [`ROUTER`].
fn arp(mac: [u8; 6], p: &[u8]) -> Option<Out> {
    if p.len() < 28 || p[6..8] != [0, 1] || p[24..28] != ROUTER {
        return None;
    }
    let mut r = p[..6].to_vec();
    r.extend_from_slice(&[0, 2]);
    r.extend_from_slice(&mac);
    r.extend_from_slice(&ROUTER);
    r.extend_from_slice(&p[8..18]);
    Some(Out { dst: p[8..14].try_into().ok()?, ethertype: ETH_ARP, payload: r })
}

fn ipv4(sta: [u8; 6], aid: u16, p: &[u8]) -> Option<Out> {
    let ihl = usize::from(*p.first()? & 15) * 4;
    if p.len() < ihl + 8 || p[0] >> 4 != 4 {
        return None;
    }
    let (proto, dst, l4) = (p[9], &p[16..20], &p[ihl..]);
    match proto {
        // A DHCP request from the station, to the server port.
        17 if l4[2..4] == [0, 67] => dhcp(aid, &l4[8..]),
        // An echo request to the router.
        1 if dst == ROUTER && l4[0] == 8 => {
            let mut icmp = l4.to_vec();
            icmp[0] = 0;
            icmp[2..4].copy_from_slice(&[0, 0]);
            let c = checksum(&icmp);
            icmp[2..4].copy_from_slice(&c.to_be_bytes());
            let src: [u8; 4] = p[12..16].try_into().ok()?;
            Some(Out { dst: sta, ethertype: ETH_IPV4, payload: ip(1, src, &icmp) })
        }
        _ => None,
    }
}

/// The offer to a discover, the acknowledgement of a request.
fn dhcp(aid: u16, b: &[u8]) -> Option<Out> {
    if b.len() < 240 || b[0] != 1 || b[236..240] != [99, 130, 83, 99] {
        return None;
    }
    let reply = match option(&b[240..], 53)?.first()? {
        1 => 2, // discover → offer
        3 => 5, // request → ack
        _ => return None,
    };
    let lease = [10, 0, 8, 1 + (aid % 250) as u8];
    let mut r = vec![0u8; 240];
    r[..3].copy_from_slice(&[2, 1, 6]);
    r[4..8].copy_from_slice(&b[4..8]); // xid
    r[16..20].copy_from_slice(&lease); // yiaddr
    r[28..44].copy_from_slice(&b[28..44]); // chaddr
    r[236..240].copy_from_slice(&[99, 130, 83, 99]);
    r.extend_from_slice(&[53, 1, reply]);
    r.extend_from_slice(&[54, 4]);
    r.extend_from_slice(&ROUTER);
    r.extend_from_slice(&[51, 4, 0, 0, 0x0e, 0x10]); // an hour
    r.extend_from_slice(&[1, 4, 255, 255, 255, 0]);
    r.extend_from_slice(&[3, 4]);
    r.extend_from_slice(&ROUTER);
    r.push(255);
    let mut udp = Vec::with_capacity(8 + r.len());
    udp.extend_from_slice(&67u16.to_be_bytes());
    udp.extend_from_slice(&68u16.to_be_bytes());
    udp.extend_from_slice(&((8 + r.len()) as u16).to_be_bytes());
    udp.extend_from_slice(&[0, 0]); // no checksum
    udp.extend_from_slice(&r);
    Some(Out { dst: BROADCAST, ethertype: ETH_IPV4, payload: ip(17, [255; 4], &udp) })
}

/// The body of DHCP option `code`.
fn option(mut o: &[u8], code: u8) -> Option<&[u8]> {
    while let [c, rest @ ..] = o {
        match *c {
            255 => return None,
            0 => o = rest,
            c => {
                let len = usize::from(*rest.first()?);
                let body = rest.get(1..1 + len)?;
                if c == code {
                    return Some(body);
                }
                o = &rest[1 + len..];
            }
        }
    }
    None
}

/// An IPv4 packet from the router to `dst`.
fn ip(proto: u8, dst: [u8; 4], body: &[u8]) -> Vec<u8> {
    let mut p = vec![0x45, 0];
    p.extend_from_slice(&((20 + body.len()) as u16).to_be_bytes());
    p.extend_from_slice(&[0, 0, 0, 0, 64, proto, 0, 0]);
    p.extend_from_slice(&ROUTER);
    p.extend_from_slice(&dst);
    let c = checksum(&p);
    p[10..12].copy_from_slice(&c.to_be_bytes());
    p.extend_from_slice(body);
    p
}

/// The Internet checksum of `b`.
fn checksum(b: &[u8]) -> u16 {
    let mut sum = 0u32;
    for c in b.chunks(2) {
        sum += u32::from(u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]));
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}
