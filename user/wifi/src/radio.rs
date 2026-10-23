//! The radio a scan and an association go through: tuned to a channel, it
//! sends and hears 802.11 frames. The simulated one is the test boots';
//! Intel's (the AX201, the AX200) is `intel.rs`.

use alloc::vec::Vec;

use crate::frame::Bss;
use myos_user::{close, open_flags, read, write_fd, O_RDONLY, O_RDWR};

/// A frame heard: its length in the buffer, the channel, the signal.
pub struct Rx {
    pub len: usize,
    pub channel: u8,
    pub rssi: i8,
}

pub trait Radio {
    /// The station's own address.
    fn mac(&self) -> [u8; 6];
    /// Hear and send on `channel` from now on.
    fn set_channel(&mut self, channel: u8);
    fn send(&mut self, frame: &[u8]) -> bool;
    /// A frame heard on the channel tuned to, if one is waiting.
    fn recv(&mut self, buf: &mut [u8]) -> Option<Rx>;
    /// The fd a `poll` for [`Radio::recv`] waits on.
    fn fd(&self) -> usize;
    /// Frames are waiting that a `poll` of [`Radio::fd`] would not show
    /// (a driver that queued them while it waited for something else).
    fn pending(&self) -> bool {
        false
    }
    /// Before authenticating with `bss`: a radio whose firmware keeps the
    /// network's state sets it up and stays on its channel; whether it
    /// could.
    fn joining(&mut self, _bss: &Bss) -> bool {
        true
    }
    /// Associated with the network being joined, as `aid`, over 802.11n
    /// when `ht` (the association offered the radio's [`Radio::ht`]).
    fn associated(&mut self, _aid: u16, _ht: bool) -> bool {
        true
    }
    /// The body of the HT Capabilities element of a radio that does
    /// 802.11n itself (its rates, aggregation off): the association
    /// offers it to an 802.11n access point.
    fn ht(&self) -> Option<[u8; 26]> {
        None
    }
    /// Once a second while associated: a radio's periodic work (choosing
    /// the transmit rate from how the last second's frames went).
    fn tick(&mut self) {}
    /// The network left, or joining it given up.
    fn left(&mut self) {}
    /// A scan the radio does by itself (its firmware's): what it heard;
    /// `None` when the daemon visits the channels itself.
    fn scan(&mut self) -> Option<Vec<Heard>> {
        None
    }
}

/// A frame a radio's own scan heard.
pub struct Heard {
    pub frame: Vec<u8>,
    pub channel: u8,
    pub rssi: i8,
}

/// The simulated radio: a NIC whose Ethernet frames carry 802.11 frames to
/// and from the launcher's access points (`src/wifi_sim.rs`): EtherType
/// [`ETHERTYPE`], then version, channel, signal and a pad byte.
pub struct Sim {
    fd: usize,
    mac: [u8; 6],
    channel: u8,
    frame: [u8; 2048],
}

const ETHERTYPE: u16 = 0x88b5;
const HDR: usize = 14 + 4;

impl Sim {
    /// The radio on NIC `dev` (`/dev/net1`).
    pub fn open(dev: &[u8]) -> Option<Self> {
        let mut path = [0u8; 64];
        let data = join(&mut path, dev, b"/data")?;
        let fd = open_flags(data, O_RDWR)?;
        let mut path = [0u8; 64];
        let Some(mac) = join(&mut path, dev, b"/ctl").and_then(read_mac) else {
            close(fd);
            return None;
        };
        Some(Sim { fd, mac, channel: 1, frame: [0; 2048] })
    }
}

impl Radio for Sim {
    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn set_channel(&mut self, channel: u8) {
        self.channel = channel;
    }

    /// An 802.11n radio of one stream, as the MT7601U is: the test boots'
    /// `myos-open` takes it over 802.11n.
    fn ht(&self) -> Option<[u8; 26]> {
        Some(mtw::chip::HT_CAP_1X1)
    }

    fn send(&mut self, frame: &[u8]) -> bool {
        let n = HDR + frame.len();
        if n > self.frame.len() {
            return false;
        }
        let f = &mut self.frame;
        f[..6].copy_from_slice(&[0xff; 6]);
        f[6..12].copy_from_slice(&self.mac);
        f[12..14].copy_from_slice(&ETHERTYPE.to_be_bytes());
        f[14..18].copy_from_slice(&[1, self.channel, 0, 0]);
        f[HDR..n].copy_from_slice(frame);
        write_fd(self.fd, &self.frame[..n]) == n
    }

    fn recv(&mut self, buf: &mut [u8]) -> Option<Rx> {
        loop {
            let n = read(self.fd, &mut self.frame);
            if n == 0 || n == usize::MAX {
                return None;
            }
            let f = &self.frame[..n];
            // Another channel's frame is not heard; nor anything else on
            // the NIC.
            if n < HDR || u16::from_be_bytes([f[12], f[13]]) != ETHERTYPE || f[14] != 1 || f[15] != self.channel {
                continue;
            }
            let len = (n - HDR).min(buf.len());
            buf[..len].copy_from_slice(&f[HDR..HDR + len]);
            return Some(Rx { len, channel: f[15], rssi: f[16] as i8 });
        }
    }

    fn fd(&self) -> usize {
        self.fd
    }
}

fn join<'a>(out: &'a mut [u8], a: &[u8], b: &[u8]) -> Option<&'a [u8]> {
    let n = a.len() + b.len();
    if n > out.len() {
        return None;
    }
    out[..a.len()].copy_from_slice(a);
    out[a.len()..n].copy_from_slice(b);
    Some(&out[..n])
}

/// The `mac aa:bb:cc:dd:ee:ff` line of a NIC's `ctl`.
fn read_mac(ctl: &[u8]) -> Option<[u8; 6]> {
    let fd = open_flags(ctl, O_RDONLY)?;
    let mut buf = [0u8; 64];
    let n = read(fd, &mut buf);
    close(fd);
    let text = buf.get(..n)?.strip_prefix(b"mac ")?;
    let mut mac = [0u8; 6];
    for (i, b) in mac.iter_mut().enumerate() {
        let hex = text.get(3 * i..3 * i + 2)?;
        *b = u8::from_str_radix(core::str::from_utf8(hex).ok()?, 16).ok()?;
    }
    Some(mac)
}
