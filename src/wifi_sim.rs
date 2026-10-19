//! The simulated Wi-Fi network of the test boots (issue #495): access
//! points on the host that the guest's simulated radio hears and talks to.
//!
//! The radio is the guest's second virtio-net NIC (`/dev/net1`), on a QEMU
//! `socket` netdev whose UDP datagrams come here. Each Ethernet frame
//! carries one 802.11 frame: broadcast destination, EtherType
//! [`ETHERTYPE`], then [`SimHeader`] (the channel it is sent on, the
//! signal it arrives with) and the 802.11 frame without its FCS. Each
//! access point beacons on its channel every 100 TU and answers the probe
//! requests sent on it, takes open-system authentication and association
//! (refused for a WPA2 network when the request carries no RSN element)
//! and lets a station go at its deauthentication or disassociation; an
//! associated station's data frames reach the network behind it
//! (`src/wifi_net.rs`), whose answers come back as data frames. A WPA2
//! network is the authenticator of the 4-way handshake ([`PASSPHRASE`])
//! and encrypts its data frames with CCMP; its first data frame from a
//! station after the handshake starts a group key handshake (a new GTK),
//! as an access point does when it rekeys. The guest hears only what is
//! on the channel its radio is tuned to (`user/wifi`, `docs/wifi.md`).
//! They beacon only for [`AWAKE`] after the radio last sent something: a
//! boot that never scans is not woken thirty times a second for nothing
//! (under TCG on riscv64 that cost the quick tests their budget).

use std::collections::HashMap;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

use wpa::eapol::{self, INFO_ACK, INFO_ENCRYPTED, INFO_INSTALL, INFO_MIC, INFO_PAIRWISE, INFO_SECURE, INFO_VERSION_AES};
use wpa::{ccmp, Ptk};

use crate::wifi_net;

/// IEEE 802 local experimental EtherType 1.
pub const ETHERTYPE: u16 = 0x88b5;
/// The beacon interval: 100 TU of 1024 µs.
const BEACON_TU: u16 = 100;
/// How long the access points beacon after the radio's last frame.
const AWAKE: Duration = Duration::from_secs(5);
/// The WPA2 networks' passphrase (`user/wifi/test.sh` joins with it).
const PASSPHRASE: &[u8] = b"myos-passphrase";
/// Their RSN element: CCMP, PSK.
const RSN: [u8; 22] = wpa::RSN_PSK_CCMP;

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
    let mut air = Air::new();
    let mut next_beacon = Instant::now();
    let mut heard: Option<Instant> = None;
    let mut buf = [0u8; 2048];
    let send = |h: Heard| {
        let _ = sock.send(&wrap(SimHeader { channel: h.channel, rssi: h.rssi }, &h.frame));
    };
    loop {
        let awake = heard.is_some_and(|t| t.elapsed() < AWAKE);
        if awake && Instant::now() >= next_beacon {
            air.beacons().into_iter().for_each(send);
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
        air.hear(hdr.channel, frame).into_iter().for_each(send);
    }
}

/// A frame of an access point: the channel it is sent on, the signal it
/// arrives with.
pub struct Heard {
    pub channel: u8,
    pub rssi: i8,
    pub frame: Vec<u8>,
}

/// The access points and their stations, whichever radio reaches them:
/// the simulated one's UDP datagrams, or the simulated AX201
/// (`src/ax201_sim.rs`).
pub struct Air {
    start: Instant,
    stations: Stations,
}

impl Air {
    pub fn new() -> Self {
        Air { start: Instant::now(), stations: Stations::default() }
    }

    /// Every access point's beacon, now.
    pub fn beacons(&self) -> Vec<Heard> {
        let ts = self.start.elapsed().as_micros() as u64;
        APS.iter().map(|ap| Heard { channel: ap.channel, rssi: ap.rssi, frame: mgmt(ap, 0x80, [0xff; 6], ts) }).collect()
    }

    /// What the access points on `channel` answer `frame` (sent on it)
    /// with.
    pub fn hear(&mut self, channel: u8, frame: &[u8]) -> Vec<Heard> {
        if frame.len() < 24 {
            return Vec::new();
        }
        let ta: [u8; 6] = frame[10..16].try_into().unwrap();
        if frame[0] != 0x40 {
            // Addressed to one access point, on its channel.
            let ra: [u8; 6] = frame[4..10].try_into().unwrap();
            let Some(ap) = APS.iter().find(|ap| ap.bssid == ra && ap.channel == channel) else {
                return Vec::new();
            };
            let out = self.stations.answer(ap, ta, frame);
            return out.into_iter().map(|frame| Heard { channel: ap.channel, rssi: ap.rssi, frame }).collect();
        }
        // A probe request (management, subtype 4): each access point on
        // the channel answers one for its SSID or the wildcard.
        let wanted = ie(&frame[24..], 0);
        let ts = self.start.elapsed().as_micros() as u64;
        APS.iter()
            .filter(|ap| ap.channel == channel && !wanted.is_some_and(|s| !s.is_empty() && s != ap.ssid))
            .map(|ap| Heard { channel: ap.channel, rssi: ap.rssi, frame: mgmt(ap, 0x50, ta, ts) })
            .collect()
    }
}

/// Where a station stands with an access point.
enum Sta {
    Authenticated,
    Associated(u16, Option<Box<Handshake>>),
}

/// A WPA2 network's keys with one station: the 4-way handshake and what
/// it agreed.
struct Handshake {
    anonce: [u8; 32],
    /// The RSN element of the station's association request.
    rsn: Vec<u8>,
    /// The replay counter of the last EAPOL-Key frame sent.
    replay: u64,
    ptk: Option<Ptk>,
    /// Message 4 came: data frames are encrypted.
    done: bool,
    /// The group key handshake was started (once per association).
    rekeyed: bool,
    /// The packet numbers of the last frame sent and received.
    tx_pn: u64,
    rx_pn: u64,
}

/// A WPA2 network's group key: its index, the key, the last packet number
/// sent with it; and the next one while a group key handshake installs it.
struct Group {
    idx: u8,
    key: [u8; 16],
    pn: u64,
    next: Option<(u8, [u8; 16])>,
}

/// The stations of every access point: `(BSSID, station)`.
#[derive(Default)]
struct Stations {
    sta: HashMap<([u8; 6], [u8; 6]), Sta>,
    next_aid: u16,
    groups: HashMap<[u8; 6], Group>,
    /// Makes the nonces and keys: a test network's, not secret.
    seed: u64,
}

/// IEEE 802.11 status codes the access points answer with.
const STATUS_OK: u16 = 0;
const STATUS_UNSPECIFIED: u16 = 1;
const STATUS_ALG_UNSUPPORTED: u16 = 13;
/// "Invalid AKMP": a WPA2 network associated without an RSN element.
const STATUS_INVALID_AKMP: u16 = 43;
/// Deauthentication reason: "4-way handshake timeout", what an access
/// point says when message 2's MIC is wrong (a wrong passphrase).
const REASON_4WAY_TIMEOUT: u16 = 15;

impl Stations {
    /// What `ap` answers station `sta`'s `frame` with: an authentication
    /// (subtype 0xb0) or association request (0x00), or a data frame to
    /// the distribution system (0x08 or QoS data, 0x88) once associated; a
    /// deauthentication (0xc0) or disassociation (0xa0) takes it off the
    /// list.
    fn answer(&mut self, ap: &Ap, sta: [u8; 6], frame: &[u8]) -> Vec<Vec<u8>> {
        let key = (ap.bssid, sta);
        let body = &frame[24..];
        match frame[0] {
            0xb0 if body.len() >= 6 => {
                let alg = u16::from_le_bytes([body[0], body[1]]);
                let seq = u16::from_le_bytes([body[2], body[3]]);
                if seq != 1 {
                    return Vec::new();
                }
                let status = if alg == 0 { STATUS_OK } else { STATUS_ALG_UNSUPPORTED };
                if status == STATUS_OK {
                    self.sta.insert(key, Sta::Authenticated);
                }
                let mut f = header(0xb0, sta, ap.bssid);
                f.extend_from_slice(&alg.to_le_bytes());
                f.extend_from_slice(&2u16.to_le_bytes());
                f.extend_from_slice(&status.to_le_bytes());
                vec![f]
            }
            0x00 if body.len() >= 4 => {
                let rsn = ie(&body[4..], 48);
                let status = match self.sta.get(&key) {
                    None => STATUS_UNSPECIFIED,
                    Some(_) if ap.wpa2 && rsn.is_none() => STATUS_INVALID_AKMP,
                    Some(_) => STATUS_OK,
                };
                let mut aid = 0;
                let mut out = Vec::new();
                if status == STATUS_OK {
                    self.next_aid = self.next_aid % 2007 + 1;
                    aid = self.next_aid;
                    let hs = ap.wpa2.then(|| {
                        let mut rsn_ie = vec![48, rsn.unwrap().len() as u8];
                        rsn_ie.extend_from_slice(rsn.unwrap());
                        Box::new(Handshake {
                            anonce: self.random(),
                            rsn: rsn_ie,
                            replay: 1,
                            ptk: None,
                            done: false,
                            rekeyed: false,
                            tx_pn: 0,
                            rx_pn: 0,
                        })
                    });
                    // Message 1: the ANonce.
                    if let Some(hs) = &hs {
                        let info = INFO_VERSION_AES | INFO_PAIRWISE | INFO_ACK;
                        let m1 = eapol::build(info, 16, hs.replay, Some(&hs.anonce), 0, &[], None);
                        out.push(data_frame(ap.bssid, sta, eapol::ETHERTYPE, &m1));
                    }
                    self.sta.insert(key, Sta::Associated(aid, hs));
                }
                let mut f = header(0x10, sta, ap.bssid);
                let cap: u16 = 0x0001 | if ap.wpa2 { 0x0010 } else { 0 };
                f.extend_from_slice(&cap.to_le_bytes());
                f.extend_from_slice(&status.to_le_bytes());
                // The association id has its two top bits set.
                f.extend_from_slice(&(aid | 0xc000).to_le_bytes());
                push_ie(&mut f, 1, &[0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
                out.insert(0, f);
                out
            }
            0xc0 | 0xa0 => {
                self.sta.remove(&key);
                Vec::new()
            }
            0x08 | 0x88 if frame[1] & 3 == 1 => self.data(ap, sta, frame).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// A data frame from associated station `sta`: an EAPOL-Key frame of a
    /// handshake, or traffic for the router (decrypted first on a WPA2
    /// network, its answer encrypted).
    fn data(&mut self, ap: &Ap, sta: [u8; 6], frame: &[u8]) -> Option<Vec<Vec<u8>>> {
        let key = (ap.bssid, sta);
        let Some(Sta::Associated(aid, hs)) = self.sta.get_mut(&key) else {
            return None;
        };
        let aid = *aid;
        let protected = frame[1] & 0x40 != 0;
        // In the clear.
        let frame = match hs.as_deref_mut() {
            Some(hs) if protected => {
                let (f, pn) = ccmp::decrypt(&hs.ptk.as_ref().filter(|_| hs.done)?.tk, frame)?;
                if pn <= hs.rx_pn {
                    return None;
                }
                hs.rx_pn = pn;
                f
            }
            Some(_) => frame.to_vec(),
            None if protected => return None,
            None => frame.to_vec(),
        };
        // On a WPA2 network: whether the 4-way handshake is done.
        let keyed = hs.as_ref().map(|hs| hs.done);
        // The LLC/SNAP header, then the EtherType.
        let body = &frame[ccmp::header_len(&frame)?..];
        if body.len() < 8 || body[..6] != [0xaa, 0xaa, 0x03, 0, 0, 0] {
            return None;
        }
        let ethertype = u16::from_be_bytes([body[6], body[7]]);
        if keyed.is_some() && ethertype == eapol::ETHERTYPE {
            return self.eapol(ap, sta, &body[8..]);
        }
        // Nothing but the handshake until it is done, and only encrypted
        // frames after.
        if keyed.is_some_and(|done| !done || !protected) {
            return None;
        }
        let out = wifi_net::answer(ap.bssid, sta, aid, ethertype, &body[8..]);
        let mut sent = Vec::new();
        // The station's first traffic after the handshake: a new group key
        // (sent before the answer, which still goes out with the old one).
        if keyed == Some(true) && let Some(Sta::Associated(_, Some(hs))) = self.sta.get(&key) && !hs.rekeyed {
            let new = self.random_key();
            let group = self.group(ap.bssid);
            let new = (if group.idx == 1 { 2 } else { 1 }, new);
            group.next = Some(new);
            let Some(Sta::Associated(_, Some(hs))) = self.sta.get_mut(&key) else {
                return None;
            };
            hs.rekeyed = true;
            hs.replay += 1;
            let ptk = hs.ptk.as_ref()?;
            let info = INFO_VERSION_AES | INFO_ACK | INFO_MIC | INFO_SECURE | INFO_ENCRYPTED;
            let data = eapol::wrap(&ptk.kek, &gtk_kde(new.0, &new.1));
            let g1 = eapol::build(info, 0, hs.replay, None, 0, &data, Some(&ptk.kck));
            sent.push(self.protect(ap, sta, data_frame(ap.bssid, sta, eapol::ETHERTYPE, &g1))?);
        }
        if let Some(o) = out {
            sent.push(self.protect(ap, sta, data_frame(ap.bssid, o.dst, o.ethertype, &o.payload))?);
        }
        Some(sent)
    }

    /// An EAPOL-Key frame from station `sta`: message 2 or 4 of the 4-way
    /// handshake, or the group key handshake's message 2.
    fn eapol(&mut self, ap: &Ap, sta: [u8; 6], f: &[u8]) -> Option<Vec<Vec<u8>>> {
        let group = self.group(ap.bssid);
        let (gidx, gkey, gpn) = (group.idx, group.key, group.pn);
        let Some(Sta::Associated(_, Some(hs))) = self.sta.get_mut(&(ap.bssid, sta)) else {
            return None;
        };
        let k = eapol::parse(f)?;
        if k.replay != hs.replay || k.info & INFO_MIC == 0 {
            return None;
        }
        let pairwise = k.info & INFO_PAIRWISE != 0;
        match (&hs.ptk, pairwise, k.info & INFO_SECURE != 0) {
            // Message 2: the SNonce; the PTK. A wrong MIC is a wrong
            // passphrase: the station is let go.
            (None, true, false) => {
                let pmk = wpa::psk(PASSPHRASE, ap.ssid)?;
                let ptk = Ptk::derive(&pmk, ap.bssid, sta, &hs.anonce, k.nonce);
                if !eapol::mic_ok(&ptk.kck, f) || k.data != hs.rsn {
                    self.sta.remove(&(ap.bssid, sta));
                    let mut d = header(0xc0, sta, ap.bssid);
                    d.extend_from_slice(&REASON_4WAY_TIMEOUT.to_le_bytes());
                    return Some(vec![d]);
                }
                // Message 3: the beacon's RSN element and the GTK, wrapped.
                hs.replay += 1;
                let mut data = RSN.to_vec();
                data.extend_from_slice(&gtk_kde(gidx, &gkey));
                let info =
                    INFO_VERSION_AES | INFO_PAIRWISE | INFO_ACK | INFO_MIC | INFO_INSTALL | INFO_SECURE | INFO_ENCRYPTED;
                let data = eapol::wrap(&ptk.kek, &data);
                let m3 = eapol::build(info, 16, hs.replay, Some(&hs.anonce), gpn, &data, Some(&ptk.kck));
                hs.ptk = Some(ptk);
                Some(vec![data_frame(ap.bssid, sta, eapol::ETHERTYPE, &m3)])
            }
            // Message 4: the keys are in.
            (Some(ptk), true, true) if !hs.done => {
                hs.done = eapol::mic_ok(&ptk.kck, f);
                Some(Vec::new())
            }
            // The group key handshake's message 2: the new GTK is the one
            // group frames go out with.
            (Some(ptk), false, true) if hs.done => {
                if eapol::mic_ok(&ptk.kck, f) {
                    let group = self.group(ap.bssid);
                    if let Some((idx, key)) = group.next.take() {
                        (group.idx, group.key, group.pn) = (idx, key, 0);
                    }
                }
                Some(Vec::new())
            }
            _ => None,
        }
    }

    /// Data frame `f` for station `sta` (or a group), encrypted when `ap`
    /// is a WPA2 network: with the station's TK, or the group key.
    fn protect(&mut self, ap: &Ap, sta: [u8; 6], f: Vec<u8>) -> Option<Vec<u8>> {
        if !ap.wpa2 {
            return Some(f);
        }
        if f[4] & 1 != 0 {
            let group = self.group(ap.bssid);
            group.pn += 1;
            return ccmp::encrypt(&group.key, group.pn, group.idx, &f);
        }
        let Some(Sta::Associated(_, Some(hs))) = self.sta.get_mut(&(ap.bssid, sta)) else {
            return None;
        };
        hs.tx_pn += 1;
        ccmp::encrypt(&hs.ptk.as_ref()?.tk, hs.tx_pn, 0, &f)
    }

    /// The group key of access point `bssid`, made at its first use.
    fn group(&mut self, bssid: [u8; 6]) -> &mut Group {
        if !self.groups.contains_key(&bssid) {
            let key = self.random_key();
            self.groups.insert(bssid, Group { idx: 1, key, pn: 0, next: None });
        }
        self.groups.get_mut(&bssid).unwrap()
    }

    /// 32 bytes no two calls repeat: from the process's random hash keys
    /// and a counter. A test network's nonces and keys need no more.
    fn random(&mut self) -> [u8; 32] {
        use std::hash::{BuildHasher, RandomState};
        let state = RandomState::new();
        let mut out = [0u8; 32];
        for chunk in out.chunks_mut(8) {
            self.seed += 1;
            chunk.copy_from_slice(&state.hash_one(self.seed).to_le_bytes());
        }
        out
    }

    fn random_key(&mut self) -> [u8; 16] {
        self.random()[..16].try_into().unwrap()
    }
}

/// A data frame from the distribution system: to `dst`, from access point
/// `bssid` (the router behind it has the same address), carrying `payload`
/// of `ethertype` after its LLC/SNAP header.
fn data_frame(bssid: [u8; 6], dst: [u8; 6], ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = vec![0x08, 0x02, 0, 0];
    f.extend_from_slice(&dst);
    f.extend_from_slice(&bssid);
    f.extend_from_slice(&bssid);
    f.extend_from_slice(&[0, 0, 0xaa, 0xaa, 0x03, 0, 0, 0]);
    f.extend_from_slice(&ethertype.to_be_bytes());
    f.extend_from_slice(payload);
    f
}

/// The GTK key data encapsulation of group key `idx`.
fn gtk_kde(idx: u8, key: &[u8; 16]) -> Vec<u8> {
    let mut d = vec![0xdd, 22, 0x00, 0x0f, 0xac, 1, idx, 0];
    d.extend_from_slice(key);
    d
}

/// A management frame's header from `ap` to `da`.
fn header(fc: u8, da: [u8; 6], bssid: [u8; 6]) -> Vec<u8> {
    let mut f = vec![fc, 0, 0, 0];
    f.extend_from_slice(&da);
    f.extend_from_slice(&bssid);
    f.extend_from_slice(&bssid);
    f.extend_from_slice(&[0, 0]);
    f
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
        f.extend_from_slice(&RSN);
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
