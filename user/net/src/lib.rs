#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

//! Userspace smoltcp PHY over `/dev/netN`.
//!
//! Structured so a later `netd` can reuse [`Net0Device`], [`build_interface`],
//! and [`VirtualInstant`]. No `panic_handler` (this is a lib).

extern crate alloc;

use alloc::vec::Vec;

use myos_user::{close, open, open_flags, read, write_fd, O_RDWR};
use smoltcp::iface::{Config, Interface};
use smoltcp::phy::{
    Checksum, ChecksumCapabilities, Device, DeviceCapabilities, Medium, RxToken, TxToken,
};
use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetAddress, EthernetFrame, EthernetProtocol, EthernetRepr, HardwareAddress, Icmpv4Packet,
    Icmpv6Packet, Icmpv6Repr, IpAddress, IpProtocol, Ipv4Address, Ipv4Packet, Ipv6Address,
    Ipv6Packet, Ipv6Repr, NdiscRepr, RawHardwareAddress,
};

/// Re-export so `/ping` (and later netd) can use smoltcp 0.12 types without a
/// second copy of the crate. There is no crate-root `Dhcpv4Socket` alias in
/// 0.12; that type is `smoltcp::socket::dhcpv4::Socket`.
pub use smoltcp;

/// virtio-net DMA buffer is 2048; Ethernet payload max is `2048 - hdr`.
pub const FRAME_BUF: usize = 2048;
/// Ethernet header (14) + 1500 IP MTU.
pub const MTU: usize = 1514;

/// QEMU `virtio-net-pci` default MAC when no `mac=` is passed. Used only
/// when the device's `ctl` names no usable one.
pub const QEMU_DEFAULT_MAC: EthernetAddress =
    EthernetAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);

/// Stack clock in milliseconds. `bump` advances it to the real elapsed time
/// (the kernel's monotonic clock) so smoltcp's retransmit/keepalive timers
/// run at wall speed whether netd is busy or sleeping, and setting the date
/// does not move them (on the wall clock, a date years ahead and back again
/// stopped them until the clock caught up); it never goes backwards and
/// advances by at least the caller's `millis`.
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtualInstant {
    millis: u64,
    /// The monotonic clock (ns) at the first bump; 0 until then.
    epoch_ns: u64,
}

impl VirtualInstant {
    pub const fn new() -> Self {
        Self { millis: 0, epoch_ns: 0 }
    }

    pub fn now(&self) -> Instant {
        Instant::from_millis(self.millis as i64)
    }

    pub fn bump(&mut self, millis: u64) {
        let now_ns = myos_user::monotonic_ns();
        if self.epoch_ns == 0 {
            self.epoch_ns = now_ns;
        }
        let elapsed_ms = now_ns.saturating_sub(self.epoch_ns) / 1_000_000;
        if elapsed_ms > self.millis {
            self.millis = elapsed_ms;
        } else {
            self.millis = self.millis.saturating_add(millis.max(1));
        }
    }

    pub fn millis(&self) -> u64 {
        self.millis
    }
}

/// smoltcp `Device` over a NIC's directory `/dev/netN/`: `data` carries raw
/// Ethernet frames, `ctl` names the MAC and whether the RX interrupt works.
pub struct Net0Device {
    fd: usize,
    rx: [u8; FRAME_BUF],
    rx_len: usize,
    /// Frames handed to the stack so far (lets netd tell an idle poll from
    /// a busy one and sleep when nothing is happening).
    rx_frames: u64,
    /// `mac` of `ctl`, zero when it named none.
    mac: [u8; 6],
    /// `irq on` in `ctl`: a `poll` on `data` wakes when a frame arrives.
    rx_irq: bool,
    /// The last router advertisement received ([`take_router_advert`]),
    /// which smoltcp leaves alone.
    ///
    /// [`take_router_advert`]: Self::take_router_advert
    router_advert: Option<RouterAdvert>,
    /// The TCP segments an ICMP or ICMPv6 "destination unreachable" came
    /// back for since the last [`take_unreachable`] (smoltcp hands ICMP
    /// errors to no socket), at most [`UNREACHABLE_MAX`].
    ///
    /// [`take_unreachable`]: Self::take_unreachable
    unreachable: Vec<Unreachable>,
}

/// The "destination unreachable" errors a NIC keeps between two looks.
const UNREACHABLE_MAX: usize = 16;

/// A TCP segment an ICMP or ICMPv6 "destination unreachable" (RFC 792, RFC
/// 4443) quotes: where it was from and to, as it was sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unreachable {
    pub src: IpAddress,
    pub src_port: u16,
    pub dst: IpAddress,
    pub dst_port: u16,
}

/// What a router advertisement (RFC 4861) says: the router, how long it is
/// a default router (seconds, 0: not one), the prefix whose addresses are
/// on the link and how long they are valid (the first prefix a host may
/// form an address in, RFC 4862), and up to three DNS servers (RDNSS,
/// RFC 8106).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouterAdvert {
    pub router: Ipv6Address,
    pub router_lifetime: u16,
    pub prefix: Option<(Ipv6Address, u8, u32)>,
    pub dns: [Option<Ipv6Address>; 3],
}

/// The link-local address of the NIC with hardware address `mac`, its
/// modified EUI-64 interface identifier (RFC 4291) under fe80::/64.
pub fn link_local(mac: EthernetAddress) -> Ipv6Address {
    in_prefix(Ipv6Address::new(0xfe80, 0, 0, 0, 0, 0, 0, 0), mac)
}

/// The address of the NIC with hardware address `mac` in the /64 `prefix`
/// (SLAAC: the prefix, then the modified EUI-64 interface identifier).
pub fn in_prefix(prefix: Ipv6Address, mac: EthernetAddress) -> Ipv6Address {
    let m = mac.0;
    let mut a = prefix.octets();
    a[8..].copy_from_slice(&[m[0] ^ 2, m[1], m[2], 0xff, 0xfe, m[3], m[4], m[5]]);
    Ipv6Address::from(a)
}

impl Net0Device {
    /// Open `/dev/net0`. Returns `None` if the device is missing.
    pub fn open() -> Option<Self> {
        Self::open_nth(0)
    }

    /// Open `/dev/netN` for `n` in 0..=9.
    pub fn open_nth(n: usize) -> Option<Self> {
        if n > 9 {
            return None;
        }
        let mut dir = *b"/dev/net0";
        dir[8] = b'0' + (n as u8);
        Self::open_dir(&dir)
    }

    /// Open the NIC whose directory is `dir` (`/dev/net0`, `/dev/tap/0`):
    /// its `data` read/write, its `ctl` read once for the MAC and the
    /// interrupt.
    pub fn open_dir(dir: &[u8]) -> Option<Self> {
        let mut data = [0u8; 64];
        let mut ctl = [0u8; 64];
        let data = join(&mut data, dir, b"/data")?;
        let ctl = join(&mut ctl, dir, b"/ctl")?;
        let fd = open_flags(data, O_RDWR)?;
        let (mac, rx_irq) = read_ctl(ctl).unwrap_or(([0; 6], false));
        Some(Self {
            fd,
            rx: [0; FRAME_BUF],
            rx_len: 0,
            rx_frames: 0,
            mac,
            rx_irq,
            router_advert: None,
            unreachable: Vec::new(),
        })
    }

    /// The router advertisement received since the last call, if any.
    pub fn take_router_advert(&mut self) -> Option<RouterAdvert> {
        self.router_advert.take()
    }

    /// The "destination unreachable" errors received for TCP segments
    /// since the last call.
    pub fn take_unreachable(&mut self) -> Vec<Unreachable> {
        core::mem::take(&mut self.unreachable)
    }

    /// Ask the routers of the link to advertise (RFC 4861 router
    /// solicitation), from link-local address `src`: a host would wait up
    /// to minutes for the next unsolicited advertisement.
    pub fn send_router_solicit(&mut self, src: Ipv6Address) {
        let mac = self.mac();
        let dst = Ipv6Address::new(0xff02, 0, 0, 0, 0, 0, 0, 2);
        let icmp = Icmpv6Repr::Ndisc(NdiscRepr::RouterSolicit { lladdr: Some(RawHardwareAddress::from(mac)) });
        let ip = Ipv6Repr {
            src_addr: src,
            dst_addr: dst,
            next_header: IpProtocol::Icmpv6,
            payload_len: icmp.buffer_len(),
            hop_limit: 255,
        };
        let eth = EthernetRepr {
            src_addr: mac,
            // The all-routers group's multicast MAC (RFC 2464).
            dst_addr: EthernetAddress([0x33, 0x33, 0, 0, 0, 2]),
            ethertype: EthernetProtocol::Ipv6,
        };
        let mut buf = [0u8; 128];
        let len = eth.buffer_len() + ip.buffer_len() + icmp.buffer_len();
        let mut frame = EthernetFrame::new_unchecked(&mut buf[..len]);
        eth.emit(&mut frame);
        let mut pkt = Ipv6Packet::new_unchecked(frame.payload_mut());
        ip.emit(&mut pkt);
        let mut msg = Icmpv6Packet::new_unchecked(pkt.payload_mut());
        icmp.emit(&src, &dst, &mut msg, &device_capabilities().checksum);
        let _ = write_fd(self.fd, &buf[..len]);
    }

    pub fn fd(&self) -> usize {
        self.fd
    }

    /// Frames received since open.
    pub fn rx_frames(&self) -> u64 {
        self.rx_frames
    }

    /// The RX queue interrupts, so a `poll` on [`fd`](Self::fd) wakes when a
    /// frame arrives; without it the caller sleeps for a bounded time and
    /// looks again.
    pub fn rx_irq(&self) -> bool {
        self.rx_irq
    }

    /// The `mac` line of `ctl` as read when the NIC was opened (zero when
    /// it named none): what [`ctl_mac`] compares.
    pub fn mac_bytes(&self) -> [u8; 6] {
        self.mac
    }

    /// Hardware MAC from `ctl`; [`QEMU_DEFAULT_MAC`] when it named none or
    /// an unusable address (all-zero / multicast).
    pub fn mac(&self) -> EthernetAddress {
        if mac_usable(&self.mac) {
            EthernetAddress(self.mac)
        } else {
            QEMU_DEFAULT_MAC
        }
    }
}

/// `a` then `b` in `out`.
fn join<'a>(out: &'a mut [u8; 64], a: &[u8], b: &[u8]) -> Option<&'a [u8]> {
    let n = a.len() + b.len();
    if n > out.len() {
        return None;
    }
    out[..a.len()].copy_from_slice(a);
    out[a.len()..n].copy_from_slice(b);
    Some(&out[..n])
}

/// Whether the NIC whose directory is `dir` is there and its link up: its
/// `ctl` reads, and says no `link down` (a virtio NIC's says nothing of
/// the link; a virtual NIC's, `/dev/tap`, does).
pub fn link_up(dir: &[u8]) -> bool {
    let mut ctl = [0u8; 64];
    let Some(ctl) = join(&mut ctl, dir, b"/ctl") else {
        return false;
    };
    let Some(fd) = open(ctl) else {
        return false;
    };
    let mut text = [0u8; 128];
    let n = read(fd, &mut text);
    close(fd);
    n != usize::MAX && !text[..n].split(|&b| b == b'\n').any(|l| l == b"link down")
}

/// The `mac` line of the `ctl` of the NIC whose directory is `dir` (zero
/// when it names none); `None` when there is no such NIC.
pub fn ctl_mac(dir: &[u8]) -> Option<[u8; 6]> {
    let mut ctl = [0u8; 64];
    let ctl = join(&mut ctl, dir, b"/ctl")?;
    read_ctl(ctl).map(|(mac, _)| mac)
}

/// The `mac` and `irq` lines of a NIC's `ctl`.
fn read_ctl(path: &[u8]) -> Option<([u8; 6], bool)> {
    let fd = open(path)?;
    let mut text = [0u8; 128];
    let n = read(fd, &mut text);
    close(fd);
    if n == usize::MAX {
        return None;
    }
    let mut mac = [0u8; 6];
    let mut irq = false;
    for line in text[..n].split(|&b| b == b'\n') {
        if let Some(addr) = line.strip_prefix(b"mac ") {
            mac = parse_mac(addr)?;
        } else if let Some(state) = line.strip_prefix(b"irq ") {
            irq = state == b"on";
        }
    }
    Some((mac, irq))
}

/// `52:54:00:12:34:56` as bytes.
fn parse_mac(text: &[u8]) -> Option<[u8; 6]> {
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut mac = [0u8; 6];
    let mut parts = text.split(|&b| b == b':');
    for byte in mac.iter_mut() {
        let p = parts.next()?;
        if p.len() != 2 {
            return None;
        }
        *byte = hex(p[0])? << 4 | hex(p[1])?;
    }
    parts.next().is_none().then_some(mac)
}

/// Unicast, non-zero MAC. All-zero or multicast breaks smoltcp RX filtering.
fn mac_usable(mac: &[u8; 6]) -> bool {
    if mac.iter().all(|&b| b == 0) {
        return false;
    }
    mac[0] & 1 == 0
}

/// Build an Ethernet `Interface` with the device MAC (its `ctl`), software
/// checksums, and the given timestamp.
pub fn build_interface(device: &mut Net0Device, now: Instant) -> Interface {
    let mac = device.mac();
    let mut config = Config::new(HardwareAddress::Ethernet(mac));
    // Stable seed derived from MAC; not cryptographic.
    let mut seed = 0u64;
    for (i, b) in mac.0.iter().enumerate() {
        seed |= (*b as u64) << (8 * i);
    }
    config.random_seed = seed;
    Interface::new(config, device, now)
}

pub fn device_capabilities() -> DeviceCapabilities {
    let mut caps = DeviceCapabilities::default();
    caps.medium = Medium::Ethernet;
    caps.max_transmission_unit = MTU;
    // No burst limit: smoltcp caps every advertised TCP window at
    // `max_burst_size` segments, and a limit of 1 let a download move one
    // segment per round trip through the guest (~130 KB/s). The virtio RX
    // ring holds 16 frames, and QEMU's user network holds back what does not
    // fit instead of dropping it.
    caps.max_burst_size = None;
    // Driver did not negotiate virtio checksum offload. In smoltcp 0.12,
    // `Checksum::Both` (default) means the *stack* verifies RX and computes TX
    // in software. `Checksum::None` / `ignored()` skips checksums entirely.
    // Struct is `#[non_exhaustive]`; assign fields on Default.
    let mut csum = ChecksumCapabilities::default();
    csum.ipv4 = Checksum::Both;
    csum.udp = Checksum::Both;
    csum.tcp = Checksum::Both;
    csum.icmpv4 = Checksum::Both;
    caps.checksum = csum;
    caps
}

/// The router advertisement Ethernet frame `frame` carries, if it is one
/// a host takes (RFC 4861 6.1.2: from a link-local address, hop limit 255,
/// a valid checksum).
fn router_advert(frame: &[u8]) -> Option<RouterAdvert> {
    // Ethernet (14), IPv6 (40, ICMPv6 next), ICMPv6 type 134: most frames
    // are not one, and go no further than this.
    if frame.len() < 14 + 40 + 16 || frame[12..14] != [0x86, 0xdd] || frame[20] != 58 || frame[54] != 134 {
        return None;
    }
    let ip = Ipv6Packet::new_checked(&frame[14..]).ok()?;
    let src = ip.src_addr();
    if !src.is_unicast_link_local() || ip.hop_limit() != 255 {
        return None;
    }
    let icmp = Icmpv6Packet::new_checked(ip.payload()).ok()?;
    if !icmp.verify_checksum(&src, &ip.dst_addr()) {
        return None;
    }
    let body = ip.payload();
    let mut ra = RouterAdvert {
        router: src,
        router_lifetime: u16::from_be_bytes([body[6], body[7]]),
        prefix: None,
        dns: [None; 3],
    };
    // The options after the 16-byte header: type, length in 8-byte units.
    let mut opts = &body[16..];
    while opts.len() >= 8 {
        let len = opts[1] as usize * 8;
        if len == 0 || len > opts.len() {
            break;
        }
        let opt = &opts[..len];
        match opt[0] {
            // Prefix information: on-link and autonomous flags set.
            3 if len == 32 && ra.prefix.is_none() && opt[3] & 0xc0 == 0xc0 => {
                let valid = u32::from_be_bytes([opt[4], opt[5], opt[6], opt[7]]);
                let prefix = Ipv6Address::from(<[u8; 16]>::try_from(&opt[16..32]).ok()?);
                ra.prefix = Some((prefix, opt[2], valid));
            }
            // Recursive DNS servers: 16 bytes each after the 8 of the header.
            25 => {
                for (slot, a) in ra.dns.iter_mut().filter(|d| d.is_none()).zip(opt[8..].chunks_exact(16)) {
                    *slot = Some(Ipv6Address::from(<[u8; 16]>::try_from(a).ok()?));
                }
            }
            _ => {}
        }
        opts = &opts[len..];
    }
    Some(ra)
}

/// The TCP segment the ICMP or ICMPv6 "destination unreachable" Ethernet
/// frame `frame` carries quotes (its IP header, then at least the 8 bytes
/// after it: the ports), if it is one with a valid checksum. Any code: a
/// connect it answers fails, as on Linux (QEMU's user network sends one to
/// a SYN it could not connect on, "no route" from a host without IPv6).
fn tcp_unreachable(frame: &[u8]) -> Option<Unreachable> {
    match frame.get(12..14)? {
        [0x08, 0x00] => {
            let ip = Ipv4Packet::new_checked(frame.get(14..)?).ok()?;
            if ip.next_header() != IpProtocol::Icmp {
                return None;
            }
            let icmp = Icmpv4Packet::new_checked(ip.payload()).ok()?;
            // Type 3: destination unreachable.
            if u8::from(icmp.msg_type()) != 3 || !icmp.verify_checksum() {
                return None;
            }
            let orig = ip.payload().get(8..)?;
            let ihl = (*orig.first()? as usize & 0xf) * 4;
            if orig.get(9) != Some(&6) {
                return None;
            }
            let (src, dst) = (orig.get(12..16)?, orig.get(16..20)?);
            let tcp = orig.get(ihl..ihl + 4)?;
            Some(Unreachable {
                src: IpAddress::Ipv4(Ipv4Address::new(src[0], src[1], src[2], src[3])),
                src_port: u16::from_be_bytes([tcp[0], tcp[1]]),
                dst: IpAddress::Ipv4(Ipv4Address::new(dst[0], dst[1], dst[2], dst[3])),
                dst_port: u16::from_be_bytes([tcp[2], tcp[3]]),
            })
        }
        [0x86, 0xdd] => {
            let ip = Ipv6Packet::new_checked(frame.get(14..)?).ok()?;
            if ip.next_header() != IpProtocol::Icmpv6 {
                return None;
            }
            let icmp = Icmpv6Packet::new_checked(ip.payload()).ok()?;
            // Type 1: destination unreachable.
            if u8::from(icmp.msg_type()) != 1 || !icmp.verify_checksum(&ip.src_addr(), &ip.dst_addr()) {
                return None;
            }
            // The quoted packet as sent: no extension headers before TCP.
            let orig = ip.payload().get(8..)?;
            if orig.get(6) != Some(&6) {
                return None;
            }
            let src = <[u8; 16]>::try_from(orig.get(8..24)?).ok()?;
            let dst = <[u8; 16]>::try_from(orig.get(24..40)?).ok()?;
            let tcp = orig.get(40..44)?;
            Some(Unreachable {
                src: IpAddress::Ipv6(Ipv6Address::from(src)),
                src_port: u16::from_be_bytes([tcp[0], tcp[1]]),
                dst: IpAddress::Ipv6(Ipv6Address::from(dst)),
                dst_port: u16::from_be_bytes([tcp[2], tcp[3]]),
            })
        }
        _ => None,
    }
}

pub struct Net0RxToken<'a> {
    buf: &'a [u8],
}

pub struct Net0TxToken {
    fd: usize,
}

impl Device for Net0Device {
    type RxToken<'a> = Net0RxToken<'a>;
    type TxToken<'a> = Net0TxToken;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let n = read(self.fd, &mut self.rx);
        // 0 = no frame this poll (busy-poll). usize::MAX = error.
        if n == 0 || n == usize::MAX {
            return None;
        }
        self.rx_len = n.min(FRAME_BUF);
        self.rx_frames += 1;
        if let Some(ra) = router_advert(&self.rx[..self.rx_len]) {
            self.router_advert = Some(ra);
        }
        if let Some(u) = tcp_unreachable(&self.rx[..self.rx_len]) {
            if self.unreachable.len() < UNREACHABLE_MAX {
                self.unreachable.push(u);
            }
        }
        Some((
            Net0RxToken {
                buf: &self.rx[..self.rx_len],
            },
            Net0TxToken { fd: self.fd },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(Net0TxToken { fd: self.fd })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        device_capabilities()
    }
}

impl RxToken for Net0RxToken<'_> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(self.buf)
    }
}

impl TxToken for Net0TxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = [0u8; FRAME_BUF];
        let n = len.min(FRAME_BUF);
        let r = f(&mut buf[..n]);
        if n != 0 {
            let _ = write_fd(self.fd, &buf[..n]);
        }
        r
    }
}


