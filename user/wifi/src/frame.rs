//! 802.11 frames: the probe request a scan sends and what a beacon or a
//! probe response says about its network; the authentication, association
//! and deauthentication of a station; the data frames of its link, to and
//! from Ethernet (in the clear: `rsn.rs` encrypts and decrypts them).

use alloc::vec::Vec;

pub const BROADCAST: [u8; 6] = [0xff; 6];
/// Frame control byte 0 (type and subtype) of the frames used here.
pub const PROBE_REQ: u8 = 0x40;
pub const PROBE_RESP: u8 = 0x50;
pub const BEACON: u8 = 0x80;
pub const ASSOC_REQ: u8 = 0x00;
pub const ASSOC_RESP: u8 = 0x10;
pub const DISASSOC: u8 = 0xa0;
pub const AUTH: u8 = 0xb0;
pub const DEAUTH: u8 = 0xc0;
pub const ACTION: u8 = 0xd0;
/// The status code of a request granted.
pub const STATUS_OK: u16 = 0;
/// Deauthentication reason: the station is leaving.
pub const REASON_LEAVING: u16 = 3;
/// The management header: frame control, duration, three addresses,
/// sequence control.
const HDR: usize = 24;
/// A beacon's or probe response's fixed fields: timestamp, beacon
/// interval, capabilities.
const FIXED: usize = 12;
const CAP_PRIVACY: u16 = 0x0010;
const CAP_SHORT_PREAMBLE: u16 = 0x0020;
const CAP_SHORT_SLOT: u16 = 0x0400;
/// The rates a station offers on 2.4 GHz: 1, 2, 5.5 and 11 Mb/s, 6, 9,
/// 12, 18 (Supported Rates), then 24, 36, 48 and 54 (Extended Supported
/// Rates); on 5 GHz the OFDM ones, 6, 12 and 24 basic.
const RATES: [u8; 8] = [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24];
const EXT_RATES: [u8; 4] = [0x30, 0x48, 0x60, 0x6c];
const RATES_5G: [u8; 8] = [0x8c, 0x12, 0x98, 0x24, 0xb0, 0x48, 0x60, 0x6c];
/// The WMM Information element of a station: the Wi-Fi Alliance's OUI,
/// type 2, subtype 0, version 1, no U-APSD.
const WMM_INFO: [u8; 9] = [221, 7, 0x00, 0x50, 0xf2, 0x02, 0x00, 0x01, 0x00];

/// How a network is secured, from its capabilities and RSN element.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Security {
    Open,
    /// Privacy without an RSN element: WEP (or WPA1), not supported.
    Wep,
    Wpa2Psk,
    /// RSN with another key management (802.1X, SAE): not supported yet.
    Wpa2Other,
}

impl Security {
    pub fn name(self) -> &'static [u8] {
        match self {
            Security::Open => b"open",
            Security::Wep => b"wep",
            Security::Wpa2Psk => b"wpa2-psk",
            Security::Wpa2Other => b"wpa2",
        }
    }
}

/// What a beacon or probe response says.
#[derive(Clone)]
pub struct Bss {
    pub bssid: [u8; 6],
    pub ssid: Vec<u8>,
    /// The channel it says it is on (its DS parameter set), else the one
    /// it was heard on.
    pub channel: u8,
    pub security: Security,
    /// Its RSN element, whole (empty when it has none): the 4-way
    /// handshake checks message 3's against it.
    pub rsn: Vec<u8>,
    /// Its legacy rates, as its Supported and Extended Supported Rates say
    /// them (500 kb/s units, bit 7 for a basic rate).
    pub rates: Vec<u8>,
    /// An 802.11n access point's RX MCS bitmap, its first two bytes (MCS
    /// 0 to 15, HT Capabilities), and whether it does WMM (QoS): an
    /// 802.11n station needs both.
    pub ht_mcs: Option<u16>,
    pub wmm: bool,
    pub short_preamble: bool,
    pub short_slot: bool,
}

/// A management frame's header: `subtype` from `sa` to `da` of `bssid`.
fn header(subtype: u8, da: [u8; 6], sa: [u8; 6], bssid: [u8; 6]) -> Vec<u8> {
    let mut f = Vec::with_capacity(64);
    f.extend_from_slice(&[subtype, 0, 0, 0]);
    f.extend_from_slice(&da);
    f.extend_from_slice(&sa);
    f.extend_from_slice(&bssid);
    f.extend_from_slice(&[0, 0]);
    f
}

/// A wildcard probe request from `sa` on `channel`: every network there
/// answers.
pub fn probe_request(sa: [u8; 6], channel: u8) -> Vec<u8> {
    let mut f = header(PROBE_REQ, BROADCAST, sa, BROADCAST);
    f.extend_from_slice(&[0, 0]); // SSID: the wildcard
    rates(&mut f, channel);
    f
}

/// The Supported (and Extended Supported) Rates elements of `channel`'s
/// band.
fn rates(f: &mut Vec<u8>, channel: u8) {
    if channel > 14 {
        f.extend_from_slice(&[1, RATES_5G.len() as u8]);
        f.extend_from_slice(&RATES_5G);
        return;
    }
    f.extend_from_slice(&[1, RATES.len() as u8]);
    f.extend_from_slice(&RATES);
    f.extend_from_slice(&[50, EXT_RATES.len() as u8]);
    f.extend_from_slice(&EXT_RATES);
}

/// The network a beacon or probe response heard on `channel` describes.
pub fn parse_bss(f: &[u8], channel: u8) -> Option<Bss> {
    if f.len() < HDR + FIXED || (f[0] != BEACON && f[0] != PROBE_RESP) {
        return None;
    }
    let bssid: [u8; 6] = f[16..22].try_into().ok()?;
    let cap = u16::from_le_bytes([f[HDR + 10], f[HDR + 11]]);
    Some(bss_of(bssid, cap, &f[HDR + FIXED..], channel))
}

/// The network `bssid` with capabilities `cap` and elements `ies`, heard
/// on `channel` (a beacon's, or what a radio's own scan reports).
pub fn bss_of(bssid: [u8; 6], cap: u16, ies: &[u8], channel: u8) -> Bss {
    let ssid = ie(ies, 0).unwrap_or(&[]);
    let channel = ie(ies, 3).and_then(|ds| ds.first().copied()).unwrap_or(channel);
    let rsn = ie(ies, 48);
    let security = match rsn {
        Some(rsn) if rsn_psk(rsn) => Security::Wpa2Psk,
        Some(_) => Security::Wpa2Other,
        None if cap & CAP_PRIVACY != 0 => Security::Wep,
        None => Security::Open,
    };
    let rsn = rsn.map(|b| [&[48, b.len() as u8][..], b].concat()).unwrap_or_default();
    let rates = [ie(ies, 1).unwrap_or(&[]), ie(ies, 50).unwrap_or(&[])].concat();
    let ht_mcs = ie(ies, 45).and_then(|ht| Some(u16::from_le_bytes([*ht.get(3)?, *ht.get(4)?])));
    // WMM's Parameter element (subtype 1) or Information element (0).
    let wmm = ies_of(ies, 221).any(|v| v.len() >= 5 && v[..4] == [0x00, 0x50, 0xf2, 0x02] && v[4] <= 1);
    Bss {
        bssid,
        ssid: ssid.to_vec(),
        channel,
        security,
        rsn,
        rates,
        ht_mcs,
        wmm,
        short_preamble: cap & CAP_SHORT_PREAMBLE != 0,
        short_slot: cap & CAP_SHORT_SLOT != 0,
    }
}

/// Whether an RSN element offers PSK key management (suite 00-0f-ac:2).
fn rsn_psk(rsn: &[u8]) -> bool {
    // Version, group cipher, then the pairwise ciphers' count and list.
    let Some(n) = rsn.get(6..8).map(|c| u16::from_le_bytes([c[0], c[1]]) as usize) else {
        return false;
    };
    let akm = 8 + 4 * n;
    let Some(count) = rsn.get(akm..akm + 2).map(|c| u16::from_le_bytes([c[0], c[1]]) as usize) else {
        return false;
    };
    (0..count).any(|i| rsn.get(akm + 2 + 4 * i..akm + 6 + 4 * i) == Some(&[0x00, 0x0f, 0xac, 2][..]))
}

/// The body of the first information element `id` in `ies`.
fn ie(ies: &[u8], id: u8) -> Option<&[u8]> {
    ies_of(ies, id).next()
}

/// The bodies of the information elements `id` in `ies` (a vendor's,
/// 221, may come more than once).
fn ies_of(mut ies: &[u8], id: u8) -> impl Iterator<Item = &[u8]> {
    core::iter::from_fn(move || {
        while ies.len() >= 2 {
            let len = ies[1] as usize;
            let body = ies.get(2..2 + len)?;
            let this = ies[0];
            ies = &ies[2 + len..];
            if this == id {
                return Some(body);
            }
        }
        None
    })
}

/// The first frame of open-system authentication, from `sa` to `bssid`.
pub fn auth_request(sa: [u8; 6], bssid: [u8; 6]) -> Vec<u8> {
    let mut f = header(AUTH, bssid, sa, bssid);
    f.extend_from_slice(&[0, 0, 1, 0, 0, 0]); // open system, sequence 1, status 0
    f
}

/// An association request to `bssid` on `channel` for network `ssid`,
/// with RSN element `rsn` (whole; empty for an open network), and as an
/// 802.11n station with HT Capabilities `ht` (the element's body: the
/// radio's) when given, a QoS (WMM) one then too.
pub fn assoc_request(sa: [u8; 6], bssid: [u8; 6], channel: u8, ssid: &[u8], rsn: &[u8], ht: Option<&[u8; 26]>) -> Vec<u8> {
    let mut f = header(ASSOC_REQ, bssid, sa, bssid);
    f.extend_from_slice(&0x0001u16.to_le_bytes()); // capabilities: ESS
    f.extend_from_slice(&10u16.to_le_bytes()); // listen interval, in beacons
    f.push(0);
    f.push(ssid.len() as u8);
    f.extend_from_slice(ssid);
    rates(&mut f, channel);
    f.extend_from_slice(rsn);
    if let Some(ht) = ht {
        f.extend_from_slice(&[45, ht.len() as u8]);
        f.extend_from_slice(ht);
        f.extend_from_slice(&WMM_INFO);
    }
    f
}

/// The answer to an ADDBA request (a block ack session the access point
/// offers, IEEE 802.11-2020 9.6.4.2) in action frame body `body` from
/// `bssid`: declined (status 37), so the access point sends frames one at
/// a time; `None` for another action frame.
pub fn addba_declined(sa: [u8; 6], bssid: [u8; 6], body: &[u8]) -> Option<Vec<u8>> {
    // Category 3 (block ack), action 0 (ADDBA request): its dialog token,
    // parameters and timeout.
    let (token, params, timeout) = match body {
        [3, 0, token, p0, p1, t0, t1, ..] => (*token, [*p0, *p1], [*t0, *t1]),
        _ => return None,
    };
    let mut f = header(ACTION, bssid, sa, bssid);
    f.extend_from_slice(&[3, 1, token]);
    f.extend_from_slice(&37u16.to_le_bytes());
    f.extend_from_slice(&params);
    f.extend_from_slice(&timeout);
    Some(f)
}

/// A deauthentication of `sa` from `bssid`, for `reason`.
pub fn deauth(sa: [u8; 6], bssid: [u8; 6], reason: u16) -> Vec<u8> {
    let mut f = header(DEAUTH, bssid, sa, bssid);
    f.extend_from_slice(&reason.to_le_bytes());
    f
}

/// What the access point `bssid` said to station `me`: the frame's
/// subtype and body, for a management frame from it to `me`.
pub fn from_ap<'a>(f: &'a [u8], me: [u8; 6], bssid: [u8; 6]) -> Option<(u8, &'a [u8])> {
    if f.len() < HDR || f[4..10] != me || f[16..22] != bssid {
        return None;
    }
    Some((f[0], &f[HDR..]))
}

/// An authentication answer's status (the second frame of open system).
pub fn auth_status(body: &[u8]) -> Option<u16> {
    let b = body.get(..6)?;
    (u16::from_le_bytes([b[0], b[1]]) == 0 && u16::from_le_bytes([b[2], b[3]]) == 2)
        .then(|| u16::from_le_bytes([b[4], b[5]]))
}

/// An association response's status and association id.
pub fn assoc_status(body: &[u8]) -> Option<(u16, u16)> {
    let b = body.get(..6)?;
    Some((u16::from_le_bytes([b[2], b[3]]), u16::from_le_bytes([b[4], b[5]]) & 0x3fff))
}

/// Frame control byte 0 of a data frame, and of a QoS data frame (whose
/// header carries two more bytes, the QoS control).
const DATA: u8 = 0x08;
const QOS_DATA: u8 = 0x88;
/// Frame control byte 1: to the distribution system (station to access
/// point), from it.
const TO_DS: u8 = 0x01;
const FROM_DS: u8 = 0x02;
/// The LLC/SNAP header before a data frame's EtherType (RFC 1042).
const SNAP: [u8; 6] = [0xaa, 0xaa, 0x03, 0, 0, 0];

/// The data frame station `me` sends to `bssid` for Ethernet frame `eth`
/// (destination, source, EtherType, payload): a QoS data frame of best
/// effort on a link with QoS (`qos`).
pub fn data_to_ds(me: [u8; 6], bssid: [u8; 6], eth: &[u8], qos: bool) -> Option<Vec<u8>> {
    if eth.len() < 14 {
        return None;
    }
    let mut f = Vec::with_capacity(HDR + 2 + SNAP.len() + eth.len() - 12);
    f.extend_from_slice(&[if qos { QOS_DATA } else { DATA }, TO_DS, 0, 0]);
    f.extend_from_slice(&bssid);
    f.extend_from_slice(&me);
    f.extend_from_slice(&eth[..6]);
    f.extend_from_slice(&[0, 0]);
    if qos {
        f.extend_from_slice(&[0, 0]);
    }
    f.extend_from_slice(&SNAP);
    f.extend_from_slice(&eth[12..]);
    Some(f)
}

/// The Ethernet frame a data frame from access point `bssid` carries, when
/// it is to station `me` or to a group.
pub fn data_from_ds(f: &[u8], me: [u8; 6], bssid: [u8; 6]) -> Option<Vec<u8>> {
    let hdr = match f.first()? {
        &DATA => HDR,
        &QOS_DATA => HDR + 2,
        _ => return None,
    };
    if f.len() < hdr + SNAP.len() + 2 || f[1] & (TO_DS | FROM_DS) != FROM_DS || f[10..16] != bssid {
        return None;
    }
    let (da, sa) = (&f[4..10], &f[16..22]);
    if da != me && da[0] & 1 == 0 {
        return None;
    }
    let body = &f[hdr..];
    if body[..SNAP.len()] != SNAP {
        return None;
    }
    let mut eth = Vec::with_capacity(12 + body.len() - SNAP.len());
    eth.extend_from_slice(da);
    eth.extend_from_slice(sa);
    eth.extend_from_slice(&body[SNAP.len()..]);
    Some(eth)
}
