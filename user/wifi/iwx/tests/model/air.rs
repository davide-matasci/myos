//! The access points the model's radio hears: the three networks of the
//! test boots' simulated radio (`src/wifi_sim.rs`, the launcher's),
//! beaconing every 100 TU on their channels, taking open-system
//! authentication and association (each association the next id), and
//! acknowledging what is sent to them. `myos-open` is an 802.11n access
//! point (HT and WMM elements), the others 802.11g ones. A dual-band
//! radio's air (`Air::dual_band`) has a fourth, `myos-5g`, an 802.11n
//! access point on channel 36. Enough for the driver's frames to be
//! answered; the keys of a WPA2 network are the launcher's.

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
    ht: bool,
}

const APS: [Ap; 4] = [
    Ap { ssid: b"myos-open", bssid: [0x02, 0x00, 0x5e, 0x00, 0x01, 0x01], channel: 1, rssi: -40, wpa2: false, ht: true },
    Ap { ssid: b"myos-wpa2", bssid: [0x02, 0x00, 0x5e, 0x00, 0x01, 0x06], channel: 6, rssi: -55, wpa2: true, ht: false },
    Ap { ssid: b"myos-far", bssid: [0x02, 0x00, 0x5e, 0x00, 0x01, 0x0b], channel: 11, rssi: -82, wpa2: true, ht: false },
    Ap { ssid: b"myos-5g", bssid: [0x02, 0x00, 0x5e, 0x00, 0x05, 0x24], channel: 36, rssi: -45, wpa2: false, ht: true },
];

/// RSN: CCMP, PSK.
const RSN: [u8; 22] = [48, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0];

pub struct Air {
    start: Instant,
    next_aid: u16,
    aps: &'static [Ap],
}

impl Air {
    /// The 2.4 GHz access points.
    pub fn new() -> Self {
        Air { start: Instant::now(), next_aid: 1, aps: &APS[..3] }
    }

    /// The 2.4 GHz and 5 GHz ones.
    #[allow(dead_code)]
    pub fn dual_band() -> Self {
        Air { aps: &APS, ..Air::new() }
    }

    /// Every access point's beacon, now.
    pub fn beacons(&self) -> Vec<Heard> {
        self.aps.iter().map(|ap| self.heard(ap, mgmt(ap, 0x80, [0xff; 6], self.ts()))).collect()
    }

    /// Whether `bssid` is an access point on `channel` (it acknowledges
    /// what is sent to it).
    pub fn has_ap(&self, channel: u8, bssid: [u8; 6]) -> bool {
        self.aps.iter().any(|ap| ap.bssid == bssid && ap.channel == channel)
    }

    /// The signal of access point `bssid` on `channel`, when it is there.
    #[allow(dead_code)]
    pub fn rssi(&self, channel: u8, bssid: [u8; 6]) -> Option<i8> {
        self.aps.iter().find(|ap| ap.bssid == bssid && ap.channel == channel).map(|ap| ap.rssi)
    }

    /// What the access point `frame` is sent to on `channel` answers:
    /// an authentication (open system), an association.
    pub fn hear(&mut self, channel: u8, frame: &[u8]) -> Vec<Heard> {
        if frame.len() < 24 {
            return Vec::new();
        }
        let (ra, ta): ([u8; 6], [u8; 6]) = (frame[4..10].try_into().unwrap(), frame[10..16].try_into().unwrap());
        let Some(ap) = self.aps.iter().find(|ap| ap.bssid == ra && ap.channel == channel) else {
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
                f.extend_from_slice(if ap.channel > 14 { &[1, 3, 0x8c, 0x98, 0xb0] } else { &[1, 4, 0x82, 0x84, 0x8b, 0x96] });
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
/// (with privacy for WPA2), its SSID, rates (OFDM only on 5 GHz),
/// channel, RSN.
fn mgmt(ap: &Ap, subtype: u8, da: [u8; 6], ts_us: u64) -> Vec<u8> {
    let mut f = header(subtype, da, ap.bssid);
    f.extend_from_slice(&ts_us.to_le_bytes());
    f.extend_from_slice(&100u16.to_le_bytes());
    let cap: u16 = 0x0001 | if ap.wpa2 { 0x0010 } else { 0 };
    f.extend_from_slice(&cap.to_le_bytes());
    f.extend_from_slice(&[0, ap.ssid.len() as u8]);
    f.extend_from_slice(ap.ssid);
    if ap.channel > 14 {
        f.extend_from_slice(&[1, 8, 0x8c, 0x12, 0x98, 0x24, 0xb0, 0x48, 0x60, 0x6c]);
    } else {
        f.extend_from_slice(&[1, 8, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
    }
    f.extend_from_slice(&[3, 1, ap.channel]);
    if ap.wpa2 {
        f.extend_from_slice(&RSN);
    }
    if ap.ht {
        // HT Capabilities: 20 MHz, short GI; MCS 0 to 15 received.
        f.extend_from_slice(&[45, 26, 0x2c, 0x01, 0x03]);
        f.extend_from_slice(&[0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01, 0, 0, 0]);
        f.extend_from_slice(&[0; 7]);
        // HT Operation: the primary channel, no secondary.
        f.extend_from_slice(&[61, 22, ap.channel]);
        f.extend_from_slice(&[0; 21]);
        // WMM Parameter element (the Wi-Fi Alliance's OUI, type 2, subtype 1).
        f.extend_from_slice(&[221, 24, 0x00, 0x50, 0xf2, 0x02, 0x01, 0x01, 0x00, 0x00]);
        f.extend_from_slice(&[0x03, 0xa4, 0x00, 0x00, 0x27, 0xa4, 0x00, 0x00]);
        f.extend_from_slice(&[0x42, 0x43, 0x5e, 0x00, 0x62, 0x32, 0x2f, 0x00]);
    }
    f
}
