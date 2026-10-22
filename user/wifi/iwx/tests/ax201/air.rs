//! The access points the model's radio hears: the three networks of the
//! test boots' simulated radio (`src/wifi_sim.rs`, the launcher's),
//! beaconing every 100 TU on their channels, taking open-system
//! authentication and association (each association the next id), and
//! acknowledging what is sent to them. Enough for the driver's frames to
//! be answered; the keys of a WPA2 network are the launcher's.

use std::time::Instant;

/// A frame of an access point: the channel it is sent on, the signal it
/// arrives with.
pub struct Heard {
    pub channel: u8,
    pub rssi: i8,
    pub frame: Vec<u8>,
}

struct Ap {
    ssid: &'static [u8],
    bssid: [u8; 6],
    channel: u8,
    rssi: i8,
    wpa2: bool,
}

const APS: [Ap; 3] = [
    Ap { ssid: b"myos-open", bssid: [0x02, 0x00, 0x5e, 0x00, 0x01, 0x01], channel: 1, rssi: -40, wpa2: false },
    Ap { ssid: b"myos-wpa2", bssid: [0x02, 0x00, 0x5e, 0x00, 0x01, 0x06], channel: 6, rssi: -55, wpa2: true },
    Ap { ssid: b"myos-far", bssid: [0x02, 0x00, 0x5e, 0x00, 0x01, 0x0b], channel: 11, rssi: -82, wpa2: true },
];

/// RSN: CCMP, PSK.
const RSN: [u8; 22] = [48, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0];

pub struct Air {
    start: Instant,
    next_aid: u16,
}

impl Air {
    pub fn new() -> Self {
        Air { start: Instant::now(), next_aid: 1 }
    }

    /// Every access point's beacon, now.
    pub fn beacons(&self) -> Vec<Heard> {
        APS.iter().map(|ap| self.heard(ap, mgmt(ap, 0x80, [0xff; 6], self.ts()))).collect()
    }

    /// Whether `bssid` is an access point on `channel` (it acknowledges
    /// what is sent to it).
    pub fn has_ap(&self, channel: u8, bssid: [u8; 6]) -> bool {
        APS.iter().any(|ap| ap.bssid == bssid && ap.channel == channel)
    }

    /// What the access point `frame` is sent to on `channel` answers:
    /// an authentication (open system), an association.
    pub fn hear(&mut self, channel: u8, frame: &[u8]) -> Vec<Heard> {
        if frame.len() < 24 {
            return Vec::new();
        }
        let (ra, ta): ([u8; 6], [u8; 6]) = (frame[4..10].try_into().unwrap(), frame[10..16].try_into().unwrap());
        let Some(ap) = APS.iter().find(|ap| ap.bssid == ra && ap.channel == channel) else {
            return Vec::new();
        };
        let body = &frame[24..];
        let answer = match frame[0] {
            // Authentication: open system, transaction 1 → 2, success.
            0xb0 if body.len() >= 6 && body[..4] == [0, 0, 1, 0] => {
                let mut f = header(0xb0, ta, ap.bssid);
                f.extend_from_slice(&[0, 0, 2, 0, 0, 0]);
                f
            }
            // Association request: success, the next id.
            0x00 => {
                let mut f = header(0x10, ta, ap.bssid);
                f.extend_from_slice(&[0x01, 0x00, 0, 0]);
                f.extend_from_slice(&(0xc000 | self.next_aid).to_le_bytes());
                self.next_aid += 1;
                f.extend_from_slice(&[1, 4, 0x82, 0x84, 0x8b, 0x96]);
                f
            }
            _ => return Vec::new(),
        };
        vec![self.heard(ap, answer)]
    }

    fn ts(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }

    fn heard(&self, ap: &Ap, frame: Vec<u8>) -> Heard {
        Heard { channel: ap.channel, rssi: ap.rssi, frame }
    }
}

/// A management frame's header: `subtype` from the access point `bssid` to
/// `da`.
fn header(subtype: u8, da: [u8; 6], bssid: [u8; 6]) -> Vec<u8> {
    let mut f = vec![subtype, 0, 0, 0];
    f.extend_from_slice(&da);
    f.extend_from_slice(&bssid);
    f.extend_from_slice(&bssid);
    f.extend_from_slice(&[0, 0]);
    f
}

/// A beacon (0x80) or probe response (0x50): timestamp, 100 TU, an ESS
/// (with privacy for WPA2), its SSID, rates, channel, RSN.
fn mgmt(ap: &Ap, subtype: u8, da: [u8; 6], ts_us: u64) -> Vec<u8> {
    let mut f = header(subtype, da, ap.bssid);
    f.extend_from_slice(&ts_us.to_le_bytes());
    f.extend_from_slice(&100u16.to_le_bytes());
    let cap: u16 = 0x0001 | if ap.wpa2 { 0x0010 } else { 0 };
    f.extend_from_slice(&cap.to_le_bytes());
    f.extend_from_slice(&[0, ap.ssid.len() as u8]);
    f.extend_from_slice(ap.ssid);
    f.extend_from_slice(&[1, 8, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
    f.extend_from_slice(&[3, 1, ap.channel]);
    if ap.wpa2 {
        f.extend_from_slice(&RSN);
    }
    f
}
