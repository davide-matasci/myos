//! The commands the driver sends up to a scan, and the notifications and
//! responses it reads (from OpenBSD's if_iwx.c and the structures of
//! if_iwxreg.h; notices in LICENSE).

use alloc::vec::Vec;

use crate::ring::{CmdId, le16, le32, put16, put32};

// Groups.
pub const SYSTEM_GROUP: u8 = 0x2;
pub const PHY_OPS_GROUP: u8 = 0x4;
pub const REGULATORY_AND_NVM_GROUP: u8 = 0xc;

// Commands.
pub const INIT_EXTENDED_CFG: CmdId = CmdId::wide(SYSTEM_GROUP, 0x03);
pub const SOC_CONFIGURATION: CmdId = CmdId::wide(SYSTEM_GROUP, 0x01);
pub const NVM_ACCESS_COMPLETE: CmdId = CmdId::wide(REGULATORY_AND_NVM_GROUP, 0x00);
pub const NVM_GET_INFO: CmdId = CmdId::wide(REGULATORY_AND_NVM_GROUP, 0x02);
pub const TEMP_REPORTING_THRESHOLDS: CmdId = CmdId::wide(PHY_OPS_GROUP, 0x04);
pub const TX_ANT_CONFIGURATION: CmdId = CmdId::legacy(0x98);
pub const BT_CONFIG: CmdId = CmdId::legacy(0x9b);
pub const MCC_UPDATE: CmdId = CmdId::legacy(0xc8);
pub const POWER_TABLE: CmdId = CmdId::legacy(0x77);
pub const SCAN_CFG: CmdId = CmdId::legacy(0x0c);
pub const SCAN_REQ_UMAC: CmdId = CmdId::wide(crate::ring::LONG_GROUP, 0x0d);

// Notifications (legacy group, so their wide code is the opcode).
pub const ALIVE: u16 = 0x01;
pub const REPLY_ERROR: u16 = 0x02;
pub const INIT_COMPLETE: u16 = 0x04;
pub const SCAN_COMPLETE_UMAC: u16 = 0x0f;
pub const SCAN_ITERATION_COMPLETE_UMAC: u16 = 0xb5;
pub const REPLY_RX_MPDU: u16 = 0xc1;
pub const MCC_CHUB_UPDATE: u16 = 0xc9;

/// A command response's flags: the firmware refused it.
pub const CMD_FAILED: u8 = 0x40;

/// INIT_EXTENDED_CFG: NVM access commands follow.
pub fn init_extended_cfg() -> [u8; 4] {
    (1u32 << 1).to_le_bytes()
}

pub fn nvm_access_complete() -> [u8; 4] {
    [0; 4]
}

pub fn nvm_get_info() -> [u8; 4] {
    [0; 4]
}

/// The antennas the firmware may transmit with.
pub fn tx_ant_configuration(valid: u32) -> [u8; 4] {
    valid.to_le_bytes()
}

/// Bluetooth coexistence: Wi-Fi only (BT_COEX_WIFI), no modules.
pub fn bt_config() -> [u8; 8] {
    let mut c = [0; 8];
    put32(&mut c, 0, 0x3);
    c
}

/// The SoC: integrated (no DISCRETE flag), its crystal's latency in µs.
/// OpenBSD's iwx_send_soc_conf sends no LTR delay flags for the AX201
/// (`ltr_delay & mask` of an unshifted 1); Linux would set 1 << 2. This
/// keeps what is known to work on OpenBSD.
pub fn soc_configuration(latency_us: u32) -> [u8; 8] {
    let mut c = [0; 8];
    put32(&mut c, 4, latency_us);
    c
}

/// The device's power: always awake (no POWER_SAVE_ENA, iwx_set_pslevel's
/// level 0).
pub fn power_table() -> [u8; 4] {
    [0; 4]
}

/// An empty threshold list: the firmware handles critical temperature
/// itself (num_temps 0, 8 thresholds).
pub fn temp_reporting_thresholds() -> [u8; 20] {
    [0; 20]
}

/// The regulatory domain `alpha2` ("ZZ": the world), the firmware asked for
/// the current one (GET_CURRENT, with LAR).
pub fn mcc_update(alpha2: [u8; 2], get_current: bool) -> [u8; 32] {
    let mut c = [0; 32];
    put16(&mut c, 0, (alpha2[0] as u16) << 8 | alpha2[1] as u16);
    c[2] = if get_current { 0x10 } else { 0 };
    c
}

/// SCAN_CFG, the reduced form (API REDUCED_SCAN_CONFIG): the antennas.
pub fn scan_cfg(tx_chains: u32, rx_chains: u32) -> [u8; 12] {
    let mut c = [0; 12];
    put32(&mut c, 4, tx_chains);
    put32(&mut c, 8, rx_chains);
    c
}

/// ALIVE (iwx_alive_resp_v6, 144 bytes): the firmware is up.
#[derive(Debug, PartialEq, Eq)]
pub struct Alive {
    /// ALIVE_OK when the firmware started.
    pub status: u16,
    pub lmac_error_table: [u32; 2],
    pub umac_error_table: u32,
}

pub const ALIVE_V6_LEN: usize = 144;
pub const ALIVE_OK: u16 = 0xcafe;

pub fn alive(data: &[u8]) -> Option<Alive> {
    if data.len() != ALIVE_V6_LEN {
        return None;
    }
    // {u16 status, u16 flags}, two lmac_alive of 48 bytes (their debug
    // pointers at +16, the error table first), umac_alive of 16 (its
    // error table at +8).
    Some(Alive {
        status: le16(data, 0),
        lmac_error_table: [le32(data, 4 + 16), le32(data, 4 + 48 + 16)],
        umac_error_table: le32(data, 4 + 96 + 8),
    })
}

/// The 2.4 and 5 GHz channels in the order of the NVM's channel profile
/// (iwx_nvm_channels_8000).
pub const NVM_CHANNELS: [u8; 51] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 36, 40, 44, 48, 52, 56, 60, 64, 68, 72, 76, 80, 84, 88, 92, 96, 100,
    104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 144, 149, 153, 157, 161, 165, 169, 173, 177, 181,
];
const NUM_2GHZ: usize = 14;
const CHANNEL_VALID: u32 = 1 << 0;
const CHANNEL_ACTIVE: u32 = 1 << 3;

/// A channel the radio may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Channel {
    pub number: u8,
    pub ghz5: bool,
    /// Listen only: no probe requests (no ACTIVE flag).
    pub passive: bool,
}

/// The usable channels of a channel profile (iwx_init_channel_map); 5 GHz
/// only when the SKU has the band.
pub fn channels(profile: impl Iterator<Item = u32>, band_5ghz: bool) -> Vec<Channel> {
    profile
        .zip(NVM_CHANNELS.iter().enumerate())
        .filter_map(|(flags, (i, &number))| {
            let ghz5 = i >= NUM_2GHZ;
            (flags & CHANNEL_VALID != 0 && (!ghz5 || band_5ghz)).then_some(Channel {
                number,
                ghz5,
                passive: flags & CHANNEL_ACTIVE == 0,
            })
        })
        .collect()
}

/// NVM_GET_INFO's response (iwx_nvm_get_info_rsp, version 4: 468 bytes).
#[derive(Debug)]
pub struct NvmInfo {
    pub nvm_version: u16,
    pub band_24ghz: bool,
    pub band_5ghz: bool,
    pub tx_chains: u32,
    pub rx_chains: u32,
    pub lar_enabled: bool,
    pub channels: Vec<Channel>,
}

pub const NVM_INFO_V4_LEN: usize = 468;

pub fn nvm_info(data: &[u8]) -> Option<NvmInfo> {
    if data.len() != NVM_INFO_V4_LEN {
        return None;
    }
    let sku = le32(data, 8);
    let band_5ghz = sku & (1 << 1) != 0;
    // regulatory: u32 lar_enabled, u32 n_channels, u32 profile[110].
    let profile = (0..110).map(|i| le32(data, 28 + 4 * i));
    Some(NvmInfo {
        nvm_version: le16(data, 4),
        band_24ghz: sku & 1 != 0,
        band_5ghz,
        tx_chains: le32(data, 12),
        rx_chains: le32(data, 16),
        lar_enabled: le32(data, 20) != 0,
        channels: channels(profile, band_5ghz),
    })
}

/// MCC_UPDATE's response (iwx_mcc_update_resp_v4): the domain the firmware
/// settled on and its channel profile.
pub fn mcc_response(data: &[u8], band_5ghz: bool) -> Option<([u8; 2], Vec<Channel>)> {
    if data.len() < 20 {
        return None;
    }
    let n = le32(data, 16) as usize;
    if data.len() != 20 + 4 * n {
        return None;
    }
    let mcc = le16(data, 4);
    let profile = (0..n).map(|i| le32(data, 20 + 4 * i));
    Some(([(mcc >> 8) as u8, mcc as u8], channels(profile, band_5ghz)))
}

/// SCAN_REQ_UMAC version 14 (iwx_scan_req_umac_v14, 1940 bytes): a
/// passive scan of `channels` passing every beacon and probe response up
/// (iwx_umac_scan_v14 without a desired SSID). The firmware sends
/// SCAN_ITERATION_COMPLETE_UMAC and SCAN_COMPLETE_UMAC at the end.
pub const SCAN_V14_LEN: usize = 1940;
const SCAN_PRIORITY_EXT_6: u32 = 6;
const SCAN_CHANNEL_FLAG_ENABLE_CHAN_ORDER: u8 = 1 << 5;
const SCAN_FLAGS_PASS_ALL: u16 = 1 << 1;
const SCAN_FLAGS_NTFY_ITER_COMPLETE: u16 = 1 << 2;
const SCAN_FLAGS_ADAPTIVE_DWELL: u16 = 1 << 7;
const SCAN_FLAGS_FORCE_PASSIVE: u16 = 1 << 11;
const PHY_BAND_5: u8 = 0;
const PHY_BAND_24: u8 = 1;
/// Where its parts start: general, channel, periodic, probe parameters.
const SCAN_GENERAL: usize = 8;
const SCAN_CHANNEL: usize = SCAN_GENERAL + 36;
const SCAN_PERIODIC: usize = SCAN_CHANNEL + 4 + 67 * 8;
const SCAN_PROBE: usize = SCAN_PERIODIC + 12;
const PROBE_REQ_BUF: usize = 512;

pub fn scan_v14(channels: &[Channel], max_channels: usize, mac: [u8; 6], band_5ghz: bool) -> Vec<u8> {
    let mut c = alloc::vec![0u8; SCAN_V14_LEN];
    put32(&mut c, 4, SCAN_PRIORITY_EXT_6);

    // General: dwell times (iwx_scan_umac_dwell_v10, a full scan).
    let g = SCAN_GENERAL;
    put16(
        &mut c,
        g,
        SCAN_FLAGS_FORCE_PASSIVE | SCAN_FLAGS_PASS_ALL | SCAN_FLAGS_NTFY_ITER_COMPLETE | SCAN_FLAGS_ADAPTIVE_DWELL,
    );
    c[g + 4..g + 6].fill(10); // active dwell, both LMACs
    c[g + 6] = 2; // adaptive dwell: APs on 2.4 GHz,
    c[g + 7] = 8; // on 5 GHz,
    c[g + 8] = 10; // on the social channels
    put16(&mut c, g + 10, 300); // adaptive dwell budget (TU)
    put32(&mut c, g + 28, SCAN_PRIORITY_EXT_6);
    c[g + 32..g + 34].fill(110); // passive dwell

    // The channels, in order, each once.
    let ch = SCAN_CHANNEL;
    c[ch] = SCAN_CHANNEL_FLAG_ENABLE_CHAN_ORDER;
    c[ch + 2] = 10; // n_aps_override: P2P-friendly GO channels,
    c[ch + 3] = 2; // social channels
    let n = channels.len().min(max_channels).min(67);
    c[ch + 1] = n as u8;
    for (i, chan) in channels[..n].iter().enumerate() {
        let e = ch + 4 + 8 * i;
        c[e + 4] = chan.number;
        c[e + 5] = if chan.ghz5 { PHY_BAND_5 } else { PHY_BAND_24 };
        c[e + 6] = 1; // iterations
    }

    // One iteration, now.
    c[SCAN_PERIODIC + 2] = 1;

    probe_request(&mut c[SCAN_PROBE..SCAN_PROBE + 20 + PROBE_REQ_BUF], mac, band_5ghz);
    c
}

/// The probe request template (iwx_fill_probe_req, without HT): its
/// segments {offset, length} of the MAC header, the 2.4 GHz, 5 GHz and
/// common elements, then the frame. The firmware inserts the SSID.
fn probe_request(preq: &mut [u8], mac: [u8; 6], band_5ghz: bool) {
    const RATES_11G: [u8; 12] = [0x82, 0x84, 0x8b, 0x96, 12, 18, 24, 36, 48, 72, 96, 108];
    const RATES_11A: [u8; 8] = [0x8c, 18, 0x98, 36, 0xb0, 72, 96, 108];
    let mut f: Vec<u8> = Vec::with_capacity(PROBE_REQ_BUF);
    f.extend_from_slice(&[0x40, 0, 0, 0]); // probe request, duration (by the hardware)
    f.extend_from_slice(&[0xff; 6]);
    f.extend_from_slice(&mac);
    f.extend_from_slice(&[0xff; 6]);
    f.extend_from_slice(&[0, 0]); // sequence (by the hardware)
    f.extend_from_slice(&[0, 0]); // SSID: wildcard
    let mut segments = [(0usize, f.len()), (0, 0), (0, 0), (0, 0), (0, 0)];

    let start = f.len();
    f.extend_from_slice(&[1, 8]);
    f.extend_from_slice(&RATES_11G[..8]);
    f.extend_from_slice(&[50, 4]); // extended supported rates
    f.extend_from_slice(&RATES_11G[8..]);
    f.extend_from_slice(&[3, 1, 0]); // DS parameter set (capability DS_PARAM_SET_IE)
    segments[1] = (start, f.len() - start);

    if band_5ghz {
        let start = f.len();
        f.extend_from_slice(&[1, 8]);
        f.extend_from_slice(&RATES_11A);
        segments[2] = (start, f.len() - start);
    }
    segments[4] = (f.len(), 0);

    for (i, (off, len)) in segments.iter().enumerate() {
        put16(preq, 4 * i, *off as u16);
        put16(preq, 4 * i + 2, *len as u16);
    }
    preq[20..20 + f.len()].copy_from_slice(&f);
}

/// SCAN_COMPLETE_UMAC (iwx_umac_scan_complete): the scan's status, 0 when
/// it ran to the end.
pub fn scan_complete(data: &[u8]) -> Option<u8> {
    (data.len() >= 16).then(|| data[6])
}

/// A frame received (REPLY_RX_MPDU, iwx_rx_mpdu_desc in its 48-byte v1
/// form), CRC good.
#[derive(Debug, PartialEq, Eq)]
pub struct Mpdu {
    /// The 802.11 frame, its hardware padding taken out.
    pub frame: Vec<u8>,
    pub channel: u8,
    pub dbm: i16,
}

const MPDU_DESC_LEN: usize = 48;
const MPDU_PAD: u8 = 0x20;
const MPDU_CRC_OK: u32 = 1 << 0;
const MPDU_OVERRUN_OK: u32 = 1 << 1;

pub fn mpdu(data: &[u8]) -> Option<Mpdu> {
    if data.len() < MPDU_DESC_LEN {
        return None;
    }
    let status = le32(data, 12);
    if status & (MPDU_CRC_OK | MPDU_OVERRUN_OK) != MPDU_CRC_OK | MPDU_OVERRUN_OK {
        return None;
    }
    let len = le16(data, 0) as usize;
    let body = data.get(MPDU_DESC_LEN..MPDU_DESC_LEN + len)?;
    if len < 24 {
        return None;
    }
    let mut frame = body.to_vec();
    if data[3] & MPDU_PAD != 0 {
        // Two bytes follow the header; only management frames are taken
        // so far, whose header is 24 bytes.
        if frame[0] & 0x0c != 0 {
            return None;
        }
        frame.drain(24..26.min(frame.len()));
    }
    let energy = |e: u8| if e == 0 { -256 } else { -(e as i16) };
    Some(Mpdu { frame, channel: data[34], dbm: energy(data[32]).max(energy(data[33])) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn alive_v6() {
        let mut d = vec![0u8; ALIVE_V6_LEN];
        put16(&mut d, 0, 0xcafe);
        put32(&mut d, 20, 0x0040_1234);
        put32(&mut d, 68, 0x0040_5678);
        put32(&mut d, 108, 0x0080_0000);
        let a = alive(&d).unwrap();
        assert_eq!(a, Alive { status: ALIVE_OK, lmac_error_table: [0x0040_1234, 0x0040_5678], umac_error_table: 0x0080_0000 });
        put16(&mut d, 0, 0xdead);
        assert_eq!(alive(&d).unwrap().status, 0xdead);
        assert!(alive(&d[..100]).is_none());
    }

    #[test]
    fn nvm_info_v4() {
        let mut d = vec![0u8; NVM_INFO_V4_LEN];
        put16(&mut d, 4, 0x0a1d);
        put32(&mut d, 8, 0b11); // 2.4 and 5.2 GHz
        put32(&mut d, 12, 3);
        put32(&mut d, 16, 3);
        put32(&mut d, 20, 1);
        // Channel 1 active, 13 passive, 14 not valid, 36 passive.
        put32(&mut d, 28, CHANNEL_VALID | CHANNEL_ACTIVE);
        put32(&mut d, 28 + 4 * 12, CHANNEL_VALID);
        put32(&mut d, 28 + 4 * 14, CHANNEL_VALID);
        let n = nvm_info(&d).unwrap();
        assert_eq!((n.nvm_version, n.band_5ghz, n.tx_chains, n.lar_enabled), (0x0a1d, true, 3, true));
        assert_eq!(
            n.channels,
            vec![
                Channel { number: 1, ghz5: false, passive: false },
                Channel { number: 13, ghz5: false, passive: true },
                Channel { number: 36, ghz5: true, passive: true },
            ]
        );
        // Without the 5 GHz band its channels go.
        put32(&mut d, 8, 0b01);
        assert_eq!(nvm_info(&d).unwrap().channels.len(), 2);
        assert!(nvm_info(&d[..467]).is_none());
    }

    #[test]
    fn mcc() {
        assert_eq!(&mcc_update(*b"ZZ", true)[..4], &[b'Z', b'Z', 0x10, 0]);
        let mut d = vec![0u8; 20 + 4 * 2];
        put16(&mut d, 4, (b'C' as u16) << 8 | b'H' as u16);
        put32(&mut d, 16, 2);
        put32(&mut d, 20, CHANNEL_VALID | CHANNEL_ACTIVE);
        let (cc, ch) = mcc_response(&d, true).unwrap();
        assert_eq!((&cc, ch.len()), (b"CH", 1));
        assert!(mcc_response(&d[..24], true).is_none());
    }

    #[test]
    fn small_commands() {
        assert_eq!(init_extended_cfg(), [2, 0, 0, 0]);
        assert_eq!(bt_config(), [3, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(soc_configuration(500), [0, 0, 0, 0, 0xf4, 1, 0, 0]);
        assert_eq!(scan_cfg(3, 3), [0, 0, 0, 0, 3, 0, 0, 0, 3, 0, 0, 0]);
        assert_eq!((SCAN_REQ_UMAC.sent().group, MCC_UPDATE.sent().group), (1, 1));
    }
    #[test]
    fn scan_request() {
        let ch = [Channel { number: 6, ghz5: false, passive: false }, Channel { number: 36, ghz5: true, passive: true }];
        let c = scan_v14(&ch, 67, [2, 0, 0, 0, 0, 1], true);
        assert_eq!(c.len(), SCAN_V14_LEN);
        assert_eq!((SCAN_CHANNEL, SCAN_PERIODIC, SCAN_PROBE), (44, 584, 596));
        assert_eq!(le16(&c, 8), 0x886);
        assert_eq!((c[44], c[45]), (1 << 5, 2));
        assert_eq!(&c[48 + 4..48 + 8], &[6, PHY_BAND_24, 1, 0]);
        assert_eq!(&c[56 + 4..56 + 8], &[36, PHY_BAND_5, 1, 0]);
        assert_eq!(c[586], 1);
        // The probe request: header 26 bytes, 2.4 GHz rates and DS, 5 GHz rates.
        let p = SCAN_PROBE;
        assert_eq!((le16(&c, p), le16(&c, p + 2)), (0, 26));
        assert_eq!((le16(&c, p + 4), le16(&c, p + 6)), (26, 10 + 6 + 3));
        assert_eq!((le16(&c, p + 8), le16(&c, p + 10)), (45, 10));
        assert_eq!((le16(&c, p + 16), le16(&c, p + 18)), (55, 0));
        assert_eq!(&c[p + 20..p + 24], &[0x40, 0, 0, 0]);
        assert_eq!(&c[p + 30..p + 36], &[2, 0, 0, 0, 0, 1]);
        // The firmware's limit on channels holds.
        assert_eq!(scan_v14(&ch, 1, [0; 6], true)[45], 1);
    }

    #[test]
    fn received_frame() {
        let mut d = vec![0u8; MPDU_DESC_LEN + 30];
        put16(&mut d, 0, 30);
        put32(&mut d, 12, MPDU_CRC_OK | MPDU_OVERRUN_OK);
        d[32] = 60;
        d[33] = 0;
        d[34] = 11;
        d[48] = 0x80; // beacon
        d[48 + 26] = 0xaa;
        let m = mpdu(&d).unwrap();
        assert_eq!((m.frame.len(), m.channel, m.dbm), (30, 11, -60));
        // Padded: the two bytes after the header go.
        d[3] = MPDU_PAD;
        let m = mpdu(&d).unwrap();
        assert_eq!((m.frame.len(), m.frame[24]), (28, 0xaa));
        // A bad CRC, a short frame are dropped.
        put32(&mut d, 12, MPDU_OVERRUN_OK);
        assert!(mpdu(&d).is_none());
        put32(&mut d, 12, MPDU_CRC_OK | MPDU_OVERRUN_OK);
        put16(&mut d, 0, 40);
        assert!(mpdu(&d).is_none());
    }
}
