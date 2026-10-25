//! The NICs netd drives: the virtio NIC `/dev/net0` (wired) and the virtual
//! NICs of `/dev/tap` (the Wi-Fi link, `docs/wifi.md`), each with its own
//! interface and DHCP lease over the one socket set. There may be none: a
//! board without a network (the Raspberry Pi 3 under QEMU) still has the
//! loopback interface, and a NIC that appears later is taken then.
//!
//! The default route is the preferred NIC's: the wired one when it has a
//! lease, else the first other one with a lease. A connection leaves
//! through the NIC its source address is (the multi-home smoltcp patch:
//! `set_own_source_only`), and a new one takes the address of the NIC whose
//! subnet holds its destination, else the preferred NIC's. The loopback
//! interface carries every NIC's address.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use myos_net::smoltcp::iface::{Context, Interface, SocketHandle, SocketSet};
use myos_net::smoltcp::socket::dhcpv4;
use myos_net::smoltcp::time::{Duration, Instant};
use myos_net::smoltcp::wire::{IpAddress, IpCidr, Ipv4Address, Ipv4Cidr};
use myos_net::{build_interface, link_up, Net0Device};
use myos_user::{close, PollFd, POLLIN};

use crate::{reply, REP_IFADDRS, REP_NDB};

/// The virtual NICs looked for (`/dev/tap/0` to `3`).
const TAPS: usize = 4;
/// How often netd looks for virtual NICs come and gone.
const RESCAN: Duration = Duration::from_secs(1);

/// What a DHCP server gave a NIC.
pub struct Lease {
    pub cidr: Ipv4Cidr,
    pub router: Option<Ipv4Address>,
    pub dns: Vec<Ipv4Address>,
}

pub struct Nic {
    /// `net0`, `tap0`...: its line in `/net/ifaddrs`.
    pub name: String,
    /// Its directory (`/dev/net0`, `/dev/tap/0`).
    dir: Vec<u8>,
    pub dev: Net0Device,
    pub iface: Interface,
    dhcp: SocketHandle,
    pub lease: Option<Lease>,
    wired: bool,
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
        self.list.push(Nic { name, dir, dev, iface, dhcp, lease: None, wired });
    }

    pub fn poll(&mut self, now: Instant, sockets: &mut SocketSet<'_>) {
        for n in self.list.iter_mut() {
            n.iface.poll(now, &mut n.dev, sockets);
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

    /// The NIC that carries the default route: the wired one with a
    /// lease, else the first other one with a lease.
    fn preferred(&self) -> Option<usize> {
        let leased = |n: &Nic| n.lease.is_some();
        self.list.iter().position(|n| n.wired && leased(n)).or_else(|| self.list.iter().position(leased))
    }

    /// An address of a NIC's.
    pub fn has_ip_addr(&self, a: Ipv4Address) -> bool {
        self.list.iter().any(|n| n.iface.has_ip_addr(a))
    }

    /// The source address of what goes to `dst`, as on Linux: 127.0.0.1 to
    /// 127.0.0.0/8, an address of this host's to itself, the address of
    /// the NIC whose subnet holds `dst`, else the preferred NIC's (none
    /// before a lease).
    pub fn source_for(&self, dst: Ipv4Address) -> Option<Ipv4Address> {
        if dst.is_loopback() || dst.is_unspecified() {
            return Some(Ipv4Address::LOCALHOST);
        }
        if self.has_ip_addr(dst) {
            return Some(dst);
        }
        let on_link = self.list.iter().filter_map(|n| n.lease.as_ref()).find(|l| l.cidr.contains_addr(&dst));
        on_link.or_else(|| self.preferred().and_then(|i| self.list[i].lease.as_ref())).map(|l| l.cidr.address())
    }

    /// Whether UDP may bind `a`: a NIC's address, or the first or last of
    /// its prefix (the broadcast ones).
    pub fn bindable(&self, a: Ipv4Address) -> bool {
        self.list.iter().any(|n| {
            n.iface.has_ip_addr(a)
                || n.iface.ip_addrs().iter().any(|c| match c {
                    IpCidr::Ipv4(c) if c.prefix_len() < 31 => a == c.network().address() || c.broadcast() == Some(a),
                    _ => false,
                })
        })
    }

    /// The context of the NIC whose address `src` is, else the preferred
    /// one's, else the first's (a TCP connect: its sequence numbers); none
    /// without a NIC.
    pub fn context_for(&mut self, src: Option<Ipv4Address>) -> Option<&mut Context> {
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
                    n.iface.update_ip_addrs(|addrs| {
                        addrs.clear();
                        let _ = addrs.push(IpCidr::Ipv4(cfg.address));
                    });
                    n.lease = Some(Lease { cidr: cfg.address, router: cfg.router, dns: cfg.dns_servers.iter().copied().collect() });
                    changed = true;
                }
                Some(dhcpv4::Event::Deconfigured) => {
                    n.iface.update_ip_addrs(|addrs| addrs.clear());
                    n.lease = None;
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

    /// The routes and addresses from the leases, and what netfs shows.
    fn apply(&mut self, lo: &mut Interface, chan: usize) {
        let preferred = self.preferred();
        let addrs: Vec<IpAddress> =
            self.list.iter().filter_map(|n| n.lease.as_ref()).map(|l| IpAddress::Ipv4(l.cidr.address())).collect();
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
            let own = n.lease.as_ref().map(|l| IpAddress::Ipv4(l.cidr.address()));
            let others: Vec<IpAddress> = addrs.iter().copied().filter(|a| Some(*a) != own).collect();
            n.iface.set_elsewhere_addrs(&others);
        }
        lo.update_ip_addrs(|lo_addrs| {
            lo_addrs.clear();
            let _ = lo_addrs.push(IpCidr::new(IpAddress::v4(127, 0, 0, 1), 8));
            for a in &addrs {
                let _ = lo_addrs.push(IpCidr::new(*a, 32));
            }
        });
        reply(chan, REP_NDB, 0, 0, self.ndb_text(preferred).as_bytes());
        reply(chan, REP_IFADDRS, 0, 0, self.ifaddrs_text().as_bytes());
    }

    /// The preferred lease the way Plan 9's `/net/ndb` shows it: the
    /// address line, then one indented `dns=` line per server, the
    /// preferred NIC's first, then the others' (libgloss's resolver reads
    /// those).
    fn ndb_text(&self, preferred: Option<usize>) -> String {
        let mut s = String::new();
        let Some(p) = preferred.and_then(|i| self.list[i].lease.as_ref()) else {
            return s;
        };
        let _ = write!(s, "ip={} ipmask={}", p.cidr.address(), p.cidr.netmask());
        if let Some(router) = p.router {
            let _ = write!(s, " ipgw={router}");
        }
        s.push('\n');
        let mut dns: Vec<Ipv4Address> = p.dns.clone();
        for l in self.list.iter().filter_map(|n| n.lease.as_ref()) {
            for d in &l.dns {
                if !dns.contains(d) {
                    dns.push(*d);
                }
            }
        }
        for d in dns {
            let _ = writeln!(s, "\tdns={d}");
        }
        s
    }

    /// The interfaces the way `/net/ifaddrs` shows them, one per line in
    /// index order (`lo` is 1, `net0` 2, the virtual NICs after): the
    /// name, the IPv4 address and netmask ("-" for none) and the flags.
    /// libgloss's `getifaddrs` and `if_nametoindex` read it.
    pub fn ifaddrs_text(&self) -> String {
        let mut s = String::from("lo 127.0.0.1 255.0.0.0 up,loopback\n");
        for n in &self.list {
            match &n.lease {
                Some(l) => {
                    let _ = writeln!(s, "{} {} {} up,broadcast", n.name, l.cidr.address(), l.cidr.netmask());
                }
                None => {
                    let _ = writeln!(s, "{} - - up,broadcast", n.name);
                }
            }
        }
        s
    }

    /// Once a [`RESCAN`]: take the wired NIC once it is there, and the
    /// virtual NICs that came up; let go of the virtual ones that went
    /// (their program gone, their link down).
    pub fn rescan(&mut self, now: Instant, sockets: &mut SocketSet<'_>, lo: &mut Interface, chan: usize) {
        if now - self.scanned_at < RESCAN {
            return;
        }
        self.scanned_at = now;
        let before = self.list.len();
        let mut gone = false;
        self.list.retain(|n| {
            if n.wired || link_up(&n.dir) {
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
