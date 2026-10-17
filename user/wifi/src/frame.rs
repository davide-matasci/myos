//! 802.11 management frames: the probe request a scan sends, and what a
//! beacon or a probe response says about its network.

use alloc::vec::Vec;

pub const BROADCAST: [u8; 6] = [0xff; 6];
/// Frame control byte 0 (type and subtype) of the frames used here.
pub const PROBE_REQ: u8 = 0x40;
pub const PROBE_RESP: u8 = 0x50;
pub const BEACON: u8 = 0x80;
/// The management header: frame control, duration, three addresses,
/// sequence control.
const HDR: usize = 24;
/// A beacon's or probe response's fixed fields: timestamp, beacon
/// interval, capabilities.
const FIXED: usize = 12;
const CAP_PRIVACY: u16 = 0x0010;
/// The rates a probe request offers: 1, 2, 5.5 and 11 Mb/s, 6, 9, 12, 18.
const RATES: [u8; 8] = [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24];

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
pub struct Bss {
    pub bssid: [u8; 6],
    pub ssid: Vec<u8>,
    /// The channel it says it is on (its DS parameter set), else the one
    /// it was heard on.
    pub channel: u8,
    pub security: Security,
}

/// A wildcard probe request from `sa`: every network on the channel
/// answers.
pub fn probe_request(sa: [u8; 6]) -> Vec<u8> {
    let mut f = Vec::with_capacity(HDR + 2 + 2 + RATES.len());
    f.extend_from_slice(&[PROBE_REQ, 0, 0, 0]);
    f.extend_from_slice(&BROADCAST);
    f.extend_from_slice(&sa);
    f.extend_from_slice(&BROADCAST);
    f.extend_from_slice(&[0, 0]);
    f.extend_from_slice(&[0, 0]); // SSID: the wildcard
    f.push(1);
    f.push(RATES.len() as u8);
    f.extend_from_slice(&RATES);
    f
}

/// The network a beacon or probe response heard on `channel` describes.
pub fn parse_bss(f: &[u8], channel: u8) -> Option<Bss> {
    if f.len() < HDR + FIXED || (f[0] != BEACON && f[0] != PROBE_RESP) {
        return None;
    }
    let bssid: [u8; 6] = f[16..22].try_into().ok()?;
    let cap = u16::from_le_bytes([f[HDR + 10], f[HDR + 11]]);
    let ies = &f[HDR + FIXED..];
    let ssid = ie(ies, 0).unwrap_or(&[]);
    let channel = ie(ies, 3).and_then(|ds| ds.first().copied()).unwrap_or(channel);
    let security = match ie(ies, 48) {
        Some(rsn) if rsn_psk(rsn) => Security::Wpa2Psk,
        Some(_) => Security::Wpa2Other,
        None if cap & CAP_PRIVACY != 0 => Security::Wep,
        None => Security::Open,
    };
    Some(Bss { bssid, ssid: ssid.to_vec(), channel, security })
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
