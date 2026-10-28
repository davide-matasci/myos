//! The NICs netd drives: the virtio NIC `/dev/net0` (wired) and the virtual
//! NICs of `/dev/tap` (the Wi-Fi link, `docs/wifi.md`), each with its own
//! interface and DHCP lease over the one socket set. There may be none: a
//! board without a network (the Raspberry Pi 3 under QEMU) still has the
//! loopback interface, and a NIC that appears later is taken then.
//!
//! Each NIC has an IPv6 link-local address from its MAC from the start, and
//! an address of its own in the prefix its link's router advertises
//! (SLAAC, RFC 4862: netd solicits the advertisement, `Net0Device` picks it
//! out of the frames; smoltcp leaves router advertisements alone).
//!
//! The default route of each IP version is the preferred NIC's: the wired
//! one when it has a lease (an advertised router for IPv6), else the first
//! other one with one. A connection leaves through the NIC its source
//! address is (the multi-home smoltcp patch: `set_own_source_only`), and a
//! new one takes the address of the NIC whose subnet holds its
//! destination, else the preferred NIC's. The loopback interface carries
//! every NIC's addresses.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use myos_net::smoltcp::iface::{Context, Interface, SocketHandle, SocketSet};
use myos_net::smoltcp::socket::{dhcpv4, tcp, Socket};
use myos_net::smoltcp::time::{Duration, Instant};
use myos_net::smoltcp::wire::{IpAddress, IpCidr, IpEndpoint, Ipv4Address, Ipv4Cidr, Ipv6Address, Ipv6Cidr};
use myos_net::{build_interface, ctl_mac, in_prefix, link_local, link_up, Net0Device, RouterAdvert, Unreachable};
use myos_user::{close, PollFd, POLLIN};

use crate::{reply, REP_IFADDRS, REP_NDB};

/// The virtual NICs looked for (`/dev/tap/0` to `3`).
const TAPS: usize = 4;
/// How often netd looks for virtual NICs come and gone.
const RESCAN: Duration = Duration::from_secs(1);
/// Router solicitations a NIC sends before it waits for an unsolicited
/// advertisement, and how far apart (RFC 4861: 3, 4 s).
const SOLICITS: u8 = 3;
const SOLICIT_EVERY: Duration = Duration::from_secs(4);

/// What a DHCP server gave a NIC.
pub struct Lease {
    pub cidr: Ipv4Cidr,
    pub router: Option<Ipv4Address>,
    pub dns: Vec<Ipv4Address>,
}

/// What a NIC's router advertised: its address in the prefix, the router
/// (none when it is not a default router) and the DNS servers.
#[derive(PartialEq)]
pub struct Slaac {
    pub cidr: Ipv6Cidr,
    pub router: Option<Ipv6Address>,
    pub dns: Vec<Ipv6Address>,
    /// When the address stops being valid (`None`: never).
    until: Option<Instant>,
}

pub struct Nic {
    /// `net0`, `tap0`...: its lines in `/net/ifaddrs`.
    pub name: String,
    /// Its directory (`/dev/net0`, `/dev/tap/0`).
    dir: Vec<u8>,
    pub dev: Net0Device,
    pub iface: Interface,
    dhcp: SocketHandle,
    pub lease: Option<Lease>,
    /// Its IPv6 link-local address, from its MAC.
    link_local: Ipv6Address,
    pub slaac: Option<Slaac>,
    /// Router solicitations sent, and when the last one was.
    solicits: u8,
    solicited_at: Instant,
    wired: bool,
}

impl Nic {
    /// The interface's addresses: the lease's, the link-local one, the
    /// SLAAC one.
    fn set_addrs(&mut self) {
        let v4 = self.lease.as_ref().map(|l| IpCidr::Ipv4(l.cidr));
        let ll = IpCidr::Ipv6(Ipv6Cidr::new(self.link_local, 64));
        let v6 = self.slaac.as_ref().map(|s| IpCidr::Ipv6(s.cidr));
        self.iface.update_ip_addrs(|addrs| {
            addrs.clear();
            for a in v4.into_iter().chain([ll]).chain(v6) {
                let _ = addrs.push(a);
            }
        });
    }

    /// Its addresses of both versions, the link-local one last.
    fn addrs(&self) -> impl Iterator<Item = IpAddress> + '_ {
        let v4 = self.lease.as_ref().map(|l| IpAddress::Ipv4(l.cidr.address()));
        let v6 = self.slaac.as_ref().map(|s| IpAddress::Ipv6(s.cidr.address()));
        v4.into_iter().chain(v6).chain([IpAddress::Ipv6(self.link_local)])
    }

    /// Take router advertisement `ra`: whether what the NIC has changed.
    fn advertised(&mut self, ra: RouterAdvert, now: Instant) -> bool {
        let router = (ra.router_lifetime != 0).then_some(ra.router);
        let dns: Vec<Ipv6Address> = ra.dns.iter().flatten().copied().collect();
        let slaac = match ra.prefix {
            // A /64 is what an address can be formed in (RFC 4862 5.5.3).
            Some((prefix, 64, valid)) if valid != 0 => Some(Slaac {
                cidr: Ipv6Cidr::new(in_prefix(prefix, self.dev.mac()), 64),
                router,
                dns,
                until: (valid != u32::MAX).then(|| now + Duration::from_secs(valid as u64)),
            }),
            // A prefix no longer valid takes its address with it.
            Some((_, 64, _)) => None,
            // No prefix to form one in: the router and servers change.
            _ => self.slaac.take().map(|s| Slaac { router, dns, ..s }),
        };
        let changed = slaac != self.slaac;
        self.slaac = slaac;
        changed
    }
}

pub struct Nics {
    /// `net0` first.
    pub list: Vec<Nic>,
    scanned_at: Instant,
}

impl Nics {
    pub fn new(net0: Option<Net0Device>, sockets: &mut SocketSet<'_>, now: Instant) -> Self {
        let mut nics = Nics { list: Vec::new(), scanned_at: now };
        if let Some(net0) = net0 {
            nics.add(String::from("net0"), Vec::from(b"/dev/net0" as &[u8]), net0, true, sockets, now);
        }
        nics
    }

    fn add(&mut self, name: String, dir: Vec<u8>, mut dev: Net0Device, wired: bool, sockets: &mut SocketSet<'_>, now: Instant) {
        let mut iface = build_interface(&mut dev, now);
        // The loopback interface delivers to this host's addresses; a
        // packet from another NIC's address is that NIC's to send.
        iface.set_local_elsewhere(true);
        iface.set_own_source_only(true);
        let mut dhcp = dhcpv4::Socket::new();
        dhcp.set_owner(Some(iface.hardware_addr()));
        let dhcp = sockets.add(dhcp);
        let link_local = link_local(dev.mac());
        let mut nic = Nic {
            name,
            dir,
            dev,
            iface,
            dhcp,
            lease: None,
            link_local,
            slaac: None,
            solicits: 0,
            solicited_at: now,
            wired,
        };
        nic.set_addrs();
        self.list.push(nic);
    }

    pub fn poll(&mut self, now: Instant, sockets: &mut SocketSet<'_>) {
        for n in self.list.iter_mut() {
            n.iface.poll(now, &mut n.dev, sockets);
            for u in n.dev.take_unreachable() {
                unreachable(sockets, u);
            }
        }
    }

    /// Frames received on every NIC so far.
    pub fn rx_frames(&self) -> u64 {
        self.list.iter().map(|n| n.dev.rx_frames()).sum()
    }

    /// Every NIC's `poll` wakes at a frame (the RX interrupt).
    pub fn rx_irq(&self) -> bool {
        self.list.iter().all(|n| n.dev.rx_irq())
    }

    pub fn poll_delay(&mut self, now: Instant, sockets: &SocketSet<'_>) -> Option<Duration> {
        self.list.iter_mut().filter_map(|n| n.iface.poll_delay(now, sockets)).min()
    }

    /// A `poll` entry for each NIC's frames.
    pub fn poll_fds(&self) -> impl Iterator<Item = PollFd> + '_ {
        self.list.iter().map(|n| PollFd { fd: n.dev.fd() as i32, events: POLLIN, revents: 0 })
    }

    /// The NIC that carries the IPv4 default route: the wired one with a
    /// lease, else the first other one with a lease.
    fn preferred(&self) -> Option<usize> {
        let leased = |n: &Nic| n.lease.is_some();
        self.list.iter().position(|n| n.wired && leased(n)).or_else(|| self.list.iter().position(leased))
    }

    /// The NIC that carries the IPv6 default route: the wired one with an
    /// advertised router, else the first other one with one.
    fn preferred6(&self) -> Option<usize> {
        let routed = |n: &Nic| n.slaac.as_ref().is_some_and(|s| s.router.is_some());
        self.list.iter().position(|n| n.wired && routed(n)).or_else(|| self.list.iter().position(routed))
    }

    /// An address of a NIC's.
    pub fn has_ip_addr(&self, a: IpAddress) -> bool {
        self.list.iter().any(|n| n.iface.has_ip_addr(a))
    }

    /// The source address of what goes to `dst`, as on Linux: the loopback
    /// address to the loopback ones and to an unspecified one, an address
    /// of this host's to itself, else per version ([`source_for4`],
    /// [`source_for6`]).
    ///
    /// [`source_for4`]: Self::source_for4
    /// [`source_for6`]: Self::source_for6
    pub fn source_for(&self, dst: IpAddress) -> Option<IpAddress> {
        match dst {
            IpAddress::Ipv4(d) => self.source_for4(d).map(IpAddress::Ipv4),
            IpAddress::Ipv6(d) => self.source_for6(d).map(IpAddress::Ipv6),
        }
    }

    /// 127.0.0.1 to 127.0.0.0/8, the address of the NIC whose subnet
    /// holds `dst`, else the preferred NIC's (none before a lease).
    fn source_for4(&self, dst: Ipv4Address) -> Option<Ipv4Address> {
        if dst.is_loopback() || dst.is_unspecified() {
            return Some(Ipv4Address::LOCALHOST);
        }
        if self.has_ip_addr(IpAddress::Ipv4(dst)) {
            return Some(dst);
        }
        let on_link = self.list.iter().filter_map(|n| n.lease.as_ref()).find(|l| l.cidr.contains_addr(&dst));
        on_link.or_else(|| self.preferred().and_then(|i| self.list[i].lease.as_ref())).map(|l| l.cidr.address())
    }

    /// ::1 to ::1, a link-local address to a link-local one (the
    /// preferred NIC's, else the first's: a scope a destination does not
    /// name), the address of the NIC whose prefix holds `dst`, else the
    /// preferred NIC's (none before an advertised router).
    fn source_for6(&self, dst: Ipv6Address) -> Option<Ipv6Address> {
        if dst.is_loopback() || dst.is_unspecified() {
            return Some(Ipv6Address::LOCALHOST);
        }
        if self.has_ip_addr(IpAddress::Ipv6(dst)) {
            return Some(dst);
        }
        if dst.is_unicast_link_local() {
            let n = self.preferred6().or_else(|| self.preferred()).unwrap_or(0);
            return self.list.get(n).map(|n| n.link_local);
        }
        let on_link = self.list.iter().filter_map(|n| n.slaac.as_ref()).find(|s| s.cidr.contains_addr(&dst));
        on_link.or_else(|| self.preferred6().and_then(|i| self.list[i].slaac.as_ref())).map(|s| s.cidr.address())
    }

    /// Whether UDP may bind `a`: a NIC's address, or the first or last of
    /// its IPv4 prefix (the broadcast ones).
    pub fn bindable(&self, a: IpAddress) -> bool {
        self.list.iter().any(|n| {
            n.iface.has_ip_addr(a)
                || n.iface.ip_addrs().iter().any(|c| match (c, a) {
                    (IpCidr::Ipv4(c), IpAddress::Ipv4(a)) if c.prefix_len() < 31 => {
                        a == c.network().address() || c.broadcast() == Some(a)
                    }
                    _ => false,
                })
        })
    }

    /// The context of the NIC whose address `src` is, else the preferred
    /// one's, else the first's (a TCP connect: its sequence numbers); none
    /// without a NIC.
    pub fn context_for(&mut self, src: Option<IpAddress>) -> Option<&mut Context> {
        let i = src
            .and_then(|a| self.list.iter().position(|n| n.iface.has_ip_addr(a)))
            .or_else(|| self.preferred())
            .unwrap_or(0);
        self.list.get_mut(i).map(|n| n.iface.context())
    }

    /// The DHCP events of every NIC; on a change, the routes, the
    /// loopback addresses and netfs's `/net/ndb` and `/net/ifaddrs`
    /// follow. Whether a NIC has a lease now.
    pub fn dhcp(&mut self, sockets: &mut SocketSet<'_>, lo: &mut Interface, chan: usize) -> bool {
        let mut changed = false;
        for n in self.list.iter_mut() {
            match sockets.get_mut::<dhcpv4::Socket>(n.dhcp).poll() {
                Some(dhcpv4::Event::Configured(cfg)) => {
                    n.lease = Some(Lease { cidr: cfg.address, router: cfg.router, dns: cfg.dns_servers.iter().copied().collect() });
                    n.set_addrs();
                    changed = true;
                }
                Some(dhcpv4::Event::Deconfigured) => {
                    n.lease = None;
                    n.set_addrs();
                    changed = true;
                }
                None => {}
            }
        }
        if changed {
            self.apply(lo, chan);
        }
        self.list.iter().any(|n| n.lease.is_some())
    }

    /// IPv6 autoconfiguration on every NIC: router solicitations until a
    /// router advertised, the advertisements taken, an address dropped
    /// once no longer valid; on a change the routes, the loopback
    /// addresses and netfs's files follow, as for a lease.
    pub fn slaac(&mut self, now: Instant, lo: &mut Interface, chan: usize) {
        let mut changed = false;
        for n in self.list.iter_mut() {
            let due = n.solicits == 0 || now - n.solicited_at >= SOLICIT_EVERY;
            if n.slaac.is_none() && n.solicits < SOLICITS && due {
                n.dev.send_router_solicit(n.link_local);
                n.solicits += 1;
                n.solicited_at = now;
            }
            let mut ours = false;
            if let Some(ra) = n.dev.take_router_advert() {
                ours |= n.advertised(ra, now);
            }
            if n.slaac.as_ref().and_then(|s| s.until).is_some_and(|t| now >= t) {
                n.slaac = None;
                ours = true;
            }
            if ours {
                n.set_addrs();
                changed = true;
            }
        }
        if changed {
            self.apply(lo, chan);
        }
    }

    /// The routes and addresses from the leases and advertisements, and
    /// what netfs shows.
    fn apply(&mut self, lo: &mut Interface, chan: usize) {
        let preferred = self.preferred();
        let preferred6 = self.preferred6();
        let addrs: Vec<(usize, IpAddress)> =
            self.list.iter().enumerate().flat_map(|(i, n)| n.addrs().map(move |a| (i, a))).collect();
        for (i, n) in self.list.iter_mut().enumerate() {
            let router = n.lease.as_ref().and_then(|l| l.router).filter(|_| Some(i) == preferred);
            match router {
                Some(r) => {
                    let _ = n.iface.routes_mut().add_default_ipv4_route(r);
                }
                None => {
                    n.iface.routes_mut().remove_default_ipv4_route();
                }
            }
            let router6 = n.slaac.as_ref().and_then(|s| s.router).filter(|_| Some(i) == preferred6);
            match router6 {
                Some(r) => {
                    let _ = n.iface.routes_mut().add_default_ipv6_route(r);
                }
                None => {
                    n.iface.routes_mut().remove_default_ipv6_route();
                }
            }
            let others: Vec<IpAddress> = addrs.iter().filter(|(j, _)| *j != i).map(|(_, a)| *a).collect();
            n.iface.set_elsewhere_addrs(&others);
        }
        lo.update_ip_addrs(|lo_addrs| {
            lo_addrs.clear();
            let _ = lo_addrs.push(IpCidr::new(IpAddress::v4(127, 0, 0, 1), 8));
            let _ = lo_addrs.push(IpCidr::new(IpAddress::Ipv6(Ipv6Address::LOCALHOST), 128));
            // As many as there is room for, the link-local ones last.
            for (_, a) in &addrs {
                let len = match a {
                    IpAddress::Ipv4(_) => 32,
                    IpAddress::Ipv6(_) => 128,
                };
                let _ = lo_addrs.push(IpCidr::new(*a, len));
            }
        });
        reply(chan, REP_NDB, 0, 0, self.ndb_text(preferred, preferred6).as_bytes());
        reply(chan, REP_IFADDRS, 0, 0, self.ifaddrs_text().as_bytes());
    }

    /// The preferred lease and advertisement the way Plan 9's `/net/ndb`
    /// shows them: the IPv4 address line, the IPv6 one (`ip6=` the address
    /// and its prefix length, `ip6gw=` the router), then one indented
    /// `dns=` line per server, the IPv4 ones first, of each the preferred
    /// NIC's first (libgloss's resolver reads those).
    fn ndb_text(&self, preferred: Option<usize>, preferred6: Option<usize>) -> String {
        let mut s = String::new();
        let p = preferred.and_then(|i| self.list[i].lease.as_ref());
        if let Some(p) = p {
            let _ = write!(s, "ip={} ipmask={}", p.cidr.address(), p.cidr.netmask());
            if let Some(router) = p.router {
                let _ = write!(s, " ipgw={router}");
            }
            s.push('\n');
        }
        let p6 = preferred6.and_then(|i| self.list[i].slaac.as_ref());
        if let Some(p6) = p6 {
            let _ = write!(s, "ip6={}/{}", p6.cidr.address(), p6.cidr.prefix_len());
            if let Some(router) = p6.router {
                let _ = write!(s, " ip6gw={router}");
            }
            s.push('\n');
        }
        let mut dns: Vec<IpAddress> = Vec::new();
        let v4 = p.into_iter().chain(self.list.iter().filter_map(|n| n.lease.as_ref())).flat_map(|l| l.dns.iter());
        let v6 = p6.into_iter().chain(self.list.iter().filter_map(|n| n.slaac.as_ref())).flat_map(|s| s.dns.iter());
        for d in v4.map(|d| IpAddress::Ipv4(*d)).chain(v6.map(|d| IpAddress::Ipv6(*d))) {
            if !dns.contains(&d) {
                dns.push(d);
            }
        }
        for d in dns {
            let _ = writeln!(s, "\tdns={d}");
        }
        s
    }

    /// The interfaces the way `/net/ifaddrs` shows them, in index order
    /// (`lo` is 1, `net0` 2, the virtual NICs after), a line per address:
    /// the name, the address and its netmask (IPv4 "-" for none yet), and
    /// the flags; an interface's IPv4 line first, then its IPv6 ones.
    /// libgloss's `getifaddrs` and `if_nametoindex` read it.
    pub fn ifaddrs_text(&self) -> String {
        let mut s = String::from("lo 127.0.0.1 255.0.0.0 up,loopback\n");
        let _ = writeln!(s, "lo ::1 {} up,loopback", mask6(128));
        for n in &self.list {
            match &n.lease {
                Some(l) => {
                    let _ = writeln!(s, "{} {} {} up,broadcast", n.name, l.cidr.address(), l.cidr.netmask());
                }
                None => {
                    let _ = writeln!(s, "{} - - up,broadcast", n.name);
                }
            }
            let _ = writeln!(s, "{} {} {} up,broadcast", n.name, n.link_local, mask6(64));
            if let Some(a) = &n.slaac {
                let _ = writeln!(s, "{} {} {} up,broadcast", n.name, a.cidr.address(), mask6(a.cidr.prefix_len()));
            }
        }
        s
    }

    /// Once a [`RESCAN`]: take the wired NIC once it is there, and the
    /// virtual NICs that came up; let go of the virtual ones that went
    /// (their program gone, their link down), and of one another program
    /// took over since the last look (its address is that program's: the
    /// NIC is taken again, as the new one it is, with a lease of its own,
    /// not kept with the old program's address and DHCP state).
    pub fn rescan(&mut self, now: Instant, sockets: &mut SocketSet<'_>, lo: &mut Interface, chan: usize) {
        if now - self.scanned_at < RESCAN {
            return;
        }
        self.scanned_at = now;
        let before = self.list.len();
        let mut gone = false;
        self.list.retain(|n| {
            if n.wired || (link_up(&n.dir) && ctl_mac(&n.dir) == Some(n.dev.mac_bytes())) {
                return true;
            }
            sockets.remove(n.dhcp);
            close(n.dev.fd());
            gone = true;
            false
        });
        if !self.list.iter().any(|n| n.wired) {
            if let Some(dev) = Net0Device::open() {
                self.add(String::from("net0"), Vec::from(b"/dev/net0" as &[u8]), dev, true, sockets, now);
            }
        }
        for t in 0..TAPS {
            let mut dir = Vec::from(b"/dev/tap/" as &[u8]);
            dir.push(b'0' + t as u8);
            if self.list.iter().any(|n| n.dir == dir) || !link_up(&dir) {
                continue;
            }
            if let Some(dev) = Net0Device::open_dir(&dir) {
                let mut name = String::from("tap");
                name.push((b'0' + t as u8) as char);
                self.add(name, dir, dev, false, sockets, now);
            }
        }
        if gone || self.list.len() != before {
            self.apply(lo, chan);
        }
    }
}

/// The IPv6 netmask of a `len`-bit prefix.
/// The connect a "destination unreachable" answered fails at once (netd
/// reports the socket closed before it was established: a refused
/// connect), as on Linux; smoltcp would send the SYN again until the
/// caller gave up. QEMU's user network answers so every SYN to an IPv6
/// address on a host without IPv6, where an address list with the IPv6
/// ones first (curl, c-ares's order) then reached IPv4 only through curl's
/// happy-eyeballs timer, and a connect left with no IPv4 address to try
/// waited out curl's five minutes.
fn unreachable(sockets: &mut SocketSet<'_>, u: Unreachable) {
    let (local, remote) = (IpEndpoint::new(u.src, u.src_port), IpEndpoint::new(u.dst, u.dst_port));
    for (_, s) in sockets.iter_mut() {
        if let Socket::Tcp(s) = s {
            if s.state() == tcp::State::SynSent
                && s.local_endpoint() == Some(local)
                && s.remote_endpoint() == Some(remote)
            {
                s.abort();
            }
        }
    }
}

fn mask6(len: u8) -> Ipv6Address {
    Ipv6Address::from(u128::MAX.checked_shl(128 - len as u32).unwrap_or(0))
}
