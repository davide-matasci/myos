//! The simulated Wi-Fi network of the test boots (issue #495): access
//! points on the host that the guest's simulated radio hears and talks to.
//!
//! The radio is the guest's second virtio-net NIC (`/dev/net1`), on a QEMU
//! `socket` netdev whose UDP datagrams come here. Each Ethernet frame
//! carries one 802.11 frame: broadcast destination, EtherType
//! [`ETHERTYPE`], then [`SimHeader`] (the channel it is sent on, the
//! signal it arrives with) and the 802.11 frame without its FCS. Each
//! access point beacons on its channel every 100 TU and answers the probe
//! requests sent on it; the guest hears only what is on the channel its
//! radio is tuned to (`user/wifi`, `docs/wifi.md`). They beacon only for
//! [`AWAKE`] after the radio last sent something: a boot that never scans
//! is not woken thirty times a second for nothing (under TCG on riscv64
//! that cost the quick tests their budget).

use std::net::UdpSocket;
use std::time::{Duration, Instant};

/// IEEE 802 local experimental EtherType 1.
pub const ETHERTYPE: u16 = 0x88b5;
/// The beacon interval: 100 TU of 1024 µs.
const BEACON_TU: u16 = 100;
/// How long the access points beacon after the radio's last frame.
const AWAKE: Duration = Duration::from_secs(5);

/// One simulated access point.
struct Ap {
    ssid: &'static [u8],
    bssid: [u8; 6],
    channel: u8,
    /// The signal the guest hears it with, in dBm.
    rssi: i8,
    wpa2: bool,
}

/// The networks of every test boot (`user/wifid/test.sh` expects them).
const APS: [Ap; 3] = [
    Ap { ssid: b"myos-open", bssid: [0x02, 0x00, 0x5e, 0x00, 0x01, 0x01], channel: 1, rssi: -40, wpa2: false },
    Ap { ssid: b"myos-wpa2", bssid: [0x02, 0x00, 0x5e, 0x00, 0x01, 0x06], channel: 6, rssi: -55, wpa2: true },
    Ap { ssid: b"myos-far", bssid: [0x02, 0x00, 0x5e, 0x00, 0x01, 0x0b], channel: 11, rssi: -82, wpa2: true },
];

/// What precedes the 802.11 frame in the Ethernet payload.
struct SimHeader {
    channel: u8,
    rssi: i8,
}

const SIM_HEADER: usize = 4;

/// The two UDP ports of the radio's netdev: QEMU's (it receives on it)
/// and ours.
pub struct WifiSim {
    pub qemu_port: u16,
    pub host_port: u16,
}

/// Start the access points on a thread of their own; the ports for the
/// guest's netdev ([`netdev_arg`]). `None` when no socket could be had (the
/// boot then has no radio).
pub fn start() -> Option<WifiSim> {
    let sock = UdpSocket::bind("127.0.0.1:0").ok()?;
    let host_port = sock.local_addr().ok()?.port();
    // A free port for QEMU's end: bound and let go (QEMU binds it next).
    let qemu_port = UdpSocket::bind("127.0.0.1:0").ok()?.local_addr().ok()?.port();
    sock.connect(("127.0.0.1", qemu_port)).ok()?;
    sock.set_read_timeout(Some(Duration::from_millis(20))).ok()?;
    std::thread::spawn(move || run(sock));
    Some(WifiSim { qemu_port, host_port })
}

impl WifiSim {
    /// The `-netdev` argument of the guest's radio.
    pub fn netdev_arg(&self) -> String {
        format!(
            "socket,id=wifi0,udp=127.0.0.1:{},localaddr=127.0.0.1:{}",
            self.host_port, self.qemu_port
        )
    }
}

fn run(sock: UdpSocket) {
    let start = Instant::now();
    let mut next_beacon = start;
    let mut heard: Option<Instant> = None;
    let mut buf = [0u8; 2048];
    loop {
        let awake = heard.is_some_and(|t| t.elapsed() < AWAKE);
        if awake && Instant::now() >= next_beacon {
            let ts = start.elapsed().as_micros() as u64;
            for ap in &APS {
                let frame = mgmt(ap, 0x80, [0xff; 6], ts);
                let _ = sock.send(&wrap(SimHeader { channel: ap.channel, rssi: ap.rssi }, &frame));
            }
            next_beacon = Instant::now() + Duration::from_micros(u64::from(BEACON_TU) * 1024);
        }
        // A send to a QEMU not up yet fails (connection refused): the next
        // beacon tries again.
        let Ok(n) = sock.recv(&mut buf) else {
            continue;
        };
        let Some((hdr, frame)) = unwrap(&buf[..n]) else {
            continue;
        };
        heard = Some(Instant::now());
        // A probe request (management, subtype 4): each access point on
        // the channel answers one for its SSID or the wildcard.
        if frame.len() < 24 || frame[0] != 0x40 {
            continue;
        }
        let ta: [u8; 6] = frame[10..16].try_into().unwrap();
        let wanted = ie(&frame[24..], 0);
        let ts = start.elapsed().as_micros() as u64;
        for ap in APS.iter().filter(|ap| ap.channel == hdr.channel) {
            if wanted.is_some_and(|s| !s.is_empty() && s != ap.ssid) {
                continue;
            }
            let resp = mgmt(ap, 0x50, ta, ts);
            let _ = sock.send(&wrap(SimHeader { channel: ap.channel, rssi: ap.rssi }, &resp));
        }
    }
}

/// A beacon (`subtype` 0x80) or probe response (0x50) of `ap` to `da`.
fn mgmt(ap: &Ap, subtype: u8, da: [u8; 6], ts_us: u64) -> Vec<u8> {
    let mut f = vec![subtype, 0, 0, 0];
    f.extend_from_slice(&da);
    f.extend_from_slice(&ap.bssid);
    f.extend_from_slice(&ap.bssid);
    f.extend_from_slice(&[0, 0]); // sequence control
    f.extend_from_slice(&ts_us.to_le_bytes());
    f.extend_from_slice(&BEACON_TU.to_le_bytes());
    // Capabilities: an ESS, with privacy when it has keys.
    let cap: u16 = 0x0001 | if ap.wpa2 { 0x0010 } else { 0 };
    f.extend_from_slice(&cap.to_le_bytes());
    push_ie(&mut f, 0, ap.ssid);
    // 1, 2, 5.5 and 11 Mb/s basic, 6, 9, 12 and 18.
    push_ie(&mut f, 1, &[0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
    push_ie(&mut f, 3, &[ap.channel]);
    if ap.wpa2 {
        // RSN: version 1, CCMP group and pairwise, PSK.
        push_ie(&mut f, 48, &[1, 0, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f, 0xac, 2, 0, 0]);
    }
    f
}

fn push_ie(f: &mut Vec<u8>, id: u8, body: &[u8]) {
    f.push(id);
    f.push(body.len() as u8);
    f.extend_from_slice(body);
}

/// The body of the first element `id` in `ies`.
fn ie(mut ies: &[u8], id: u8) -> Option<&[u8]> {
    while ies.len() >= 2 {
        let len = ies[1] as usize;
        let body = ies.get(2..2 + len)?;
        if ies[0] == id {
            return Some(body);
        }
        ies = &ies[2 + len..];
    }
    None
}

fn wrap(hdr: SimHeader, frame: &[u8]) -> Vec<u8> {
    let mut e = Vec::with_capacity(14 + SIM_HEADER + frame.len());
    e.extend_from_slice(&[0xff; 6]);
    e.extend_from_slice(&APS[0].bssid);
    e.extend_from_slice(&ETHERTYPE.to_be_bytes());
    e.extend_from_slice(&[1, hdr.channel, hdr.rssi as u8, 0]);
    e.extend_from_slice(frame);
    e
}

fn unwrap(e: &[u8]) -> Option<(SimHeader, &[u8])> {
    if e.len() < 14 + SIM_HEADER || u16::from_be_bytes([e[12], e[13]]) != ETHERTYPE || e[14] != 1 {
        return None;
    }
    Some((SimHeader { channel: e[15], rssi: e[16] as i8 }, &e[14 + SIM_HEADER..]))
}
