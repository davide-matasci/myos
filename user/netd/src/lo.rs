//! The device under netd's loopback interface (127.0.0.0/8 and the host's
//! own address; smoltcp's `local-pair` patch keeps the NIC's interface off
//! those): every packet sent is received back.
//!
//! An ICMP "port unreachable" going through it answers a datagram sent to
//! a local port nobody has bound. It is noted in [`LoopDevice::refused`]:
//! the connected UDP socket that sent the datagram gets ECONNREFUSED, the
//! way Linux reports it (smoltcp hands ICMP errors to no socket).

use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use myos_net::smoltcp::phy::{self, ChecksumCapabilities, Device, DeviceCapabilities, Medium};
use myos_net::smoltcp::time::Instant;
use myos_net::smoltcp::wire::Ipv4Address;

/// The datagram an ICMP "port unreachable" answered: who sent it, and to
/// which address and port.
pub struct Refused {
    pub src_port: u16,
    pub dst: Ipv4Address,
    pub dst_port: u16,
}

pub struct LoopDevice {
    queue: VecDeque<Vec<u8>>,
    pub refused: Vec<Refused>,
}

impl LoopDevice {
    pub fn new() -> Self {
        Self { queue: VecDeque::new(), refused: Vec::new() }
    }

    /// Packets sent and not yet received back.
    pub fn pending(&self) -> bool {
        !self.queue.is_empty()
    }
}

impl Device for LoopDevice {
    type RxToken<'a> = RxToken;
    type TxToken<'a> = TxToken<'a>;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 65535;
        caps.medium = Medium::Ip;
        caps.checksum = ChecksumCapabilities::ignored();
        caps
    }

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let buffer = self.queue.pop_front()?;
        Some((RxToken { buffer }, TxToken { dev: self }))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(TxToken { dev: self })
    }
}

pub struct RxToken {
    buffer: Vec<u8>,
}

impl phy::RxToken for RxToken {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.buffer)
    }
}

pub struct TxToken<'a> {
    dev: &'a mut LoopDevice,
}

impl phy::TxToken for TxToken<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut buffer = vec![0; len];
        let r = f(&mut buffer);
        if let Some(refused) = port_unreachable(&buffer) {
            self.dev.refused.push(refused);
        }
        self.dev.queue.push_back(buffer);
        r
    }
}

/// The UDP datagram an IPv4 ICMP "port unreachable" quotes: its IP header
/// and the first 8 bytes after it (RFC 792), the UDP header.
fn port_unreachable(pkt: &[u8]) -> Option<Refused> {
    let ihl = (*pkt.first()? as usize & 0xf) * 4;
    if pkt.first()? >> 4 != 4 || pkt.get(9) != Some(&1) {
        return None;
    }
    let icmp = pkt.get(ihl..)?;
    // Type 3 (destination unreachable), code 3 (port unreachable).
    if icmp.get(..2)? != [3, 3] {
        return None;
    }
    let orig = icmp.get(8..)?;
    let oihl = (*orig.first()? as usize & 0xf) * 4;
    if orig.get(9) != Some(&17) {
        return None;
    }
    let udp = orig.get(oihl..oihl + 4)?;
    let dst = orig.get(16..20)?;
    Some(Refused {
        src_port: u16::from_be_bytes([udp[0], udp[1]]),
        dst: Ipv4Address::new(dst[0], dst[1], dst[2], dst[3]),
        dst_port: u16::from_be_bytes([udp[2], udp[3]]),
    })
}
