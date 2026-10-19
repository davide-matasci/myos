//! Joining a network as a station: the PHY, MAC and binding contexts, the
//! station, its transmit queue, the TX command a frame goes out with and
//! what tells it went, and what the firmware needs of the access point's
//! beacon (from OpenBSD's if_iwx.c: iwx_auth, iwx_run, iwx_deauth,
//! iwx_run_stop, iwx_tx and the commands they send; the structures of
//! if_iwxreg.h; notices in LICENSE). A non-HT station: legacy rates, no
//! QoS, no aggregation; the daemon encrypts and decrypts itself, so no key
//! is given to the firmware.

use crate::msg::{header_len, Channel};
use crate::ring::{CmdId, le16, le32, le64, put16, put32, put64};

const MAC_CONF_GROUP: u8 = 0x3;
const DATA_PATH_GROUP: u8 = 0x5;

pub const PHY_CONTEXT: CmdId = CmdId::legacy(0x08);
pub const ADD_STA: CmdId = CmdId::legacy(0x18);
pub const REMOVE_STA: CmdId = CmdId::legacy(0x19);
pub const TXPATH_FLUSH: CmdId = CmdId::legacy(0x1e);
pub const MAC_CONTEXT: CmdId = CmdId::legacy(0x28);
pub const BINDING_CONTEXT: CmdId = CmdId::legacy(0x2b);
pub const MAC_PM_POWER_TABLE: CmdId = CmdId::legacy(0xa9);
pub const MCAST_FILTER: CmdId = CmdId::legacy(0xd0);
pub const SF_CFG: CmdId = CmdId::legacy(0xd1);
pub const SESSION_PROTECTION: CmdId = CmdId::wide(MAC_CONF_GROUP, 0x05);
pub const RLC_CONFIG: CmdId = CmdId { opcode: 0x08, group: DATA_PATH_GROUP, version: 2 };
pub const TLC_MNG_CONFIG: CmdId = CmdId::wide(DATA_PATH_GROUP, 0x0f);
pub const SCD_QUEUE_CONFIG: CmdId = CmdId::wide(DATA_PATH_GROUP, 0x17);
/// TX_CMD: a frame on a data queue, and the response telling it went.
pub const TX_CMD: u8 = 0x1c;

/// FW_CTXT_ACTION_*.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Add = 1,
    Modify = 2,
    Remove = 3,
}

/// The one station (the access point), its MAC context, PHY context and
/// binding: id 0, color 0 (IWX_STATION_ID, an iwx_node's in_id).
const STATION_ID: u8 = 0;
/// The management "TID" the queue of non-QoS frames is for.
const MGMT_TID: u8 = 15;
const FW_CTXT_INVALID: u32 = 0xffff_ffff;
const PHY_BAND_5: u8 = 0;
const PHY_BAND_24: u8 = 1;

/// PHY_CONTEXT version 3/4 with the ultra-high-band channel information
/// (CAPA ULTRA_HB_CHANNELS; iwx_phy_ctxt_cmd_uhb_v3_v4): 20 MHz on
/// `channel`. The receive chains go in RLC_CONFIG (version 2).
pub fn phy_context(action: Action, channel: Channel) -> [u8; 32] {
    let mut c = [0; 32];
    put32(&mut c, 4, action as u32);
    put32(&mut c, 8, u32::from(channel.number));
    c[12] = if channel.ghz5 { PHY_BAND_5 } else { PHY_BAND_24 };
    // width 20 MHz, control position "1 below" (both 0), LMAC 0.
    c
}

/// RLC_CONFIG version 2 (iwx_phy_send_rlc): the PHY context's receive
/// chains, one idle and one active.
pub fn rlc_config(rx_chains: u32) -> [u8; 32] {
    let mut c = [0; 32];
    put32(&mut c, 4, rx_chains << 1 | 1 << 10 | 1 << 12);
    c
}

/// What the firmware keeps of the access point's beacons (from the
/// last one heard).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Beacon {
    /// Its timestamp (the access point's TSF), and the device's clock when
    /// it came.
    pub tsf: u64,
    pub gp2: u32,
    /// Beacon interval (TU), DTIM period and count (its TIM element; 0
    /// without one).
    pub interval: u16,
    pub dtim_period: u8,
    pub dtim_count: u8,
    /// The rates it supports, as TLC_MNG_CONFIG's bits (1, 2, 5.5, 11, 6,
    /// 9, 12, 18, 24, 36, 48, 54 Mb/s).
    pub rates: u16,
}

/// The beacon timing and rates of a beacon or probe response `f`, heard
/// at device time `gp2`; with its BSSID.
pub fn beacon(f: &[u8], gp2: u32) -> Option<([u8; 6], Beacon)> {
    if f.len() < 36 || (f[0] != 0x80 && f[0] != 0x50) {
        return None;
    }
    let mut bssid = [0; 6];
    bssid.copy_from_slice(&f[16..22]);
    let mut b =
        Beacon { tsf: le64(f, 24), gp2, interval: le16(f, 32), dtim_period: 0, dtim_count: 0, rates: 0 };
    let mut ies = &f[36..];
    while ies.len() >= 2 && ies.len() >= 2 + ies[1] as usize {
        let (id, body) = (ies[0], &ies[2..2 + ies[1] as usize]);
        match id {
            1 | 50 => b.rates |= body.iter().filter_map(|&r| rate_bit(r & 0x7f)).fold(0, |a, bit| a | bit),
            5 if body.len() >= 2 => (b.dtim_count, b.dtim_period) = (body[0], body[1]),
            _ => {}
        }
        ies = &ies[2 + ies[1] as usize..];
    }
    Some((bssid, b))
}

/// A rate (in 500 kb/s) as its bit in the 11g rate set (iwx_rs_rval2idx).
fn rate_bit(r: u8) -> Option<u16> {
    const RATES: [u8; 12] = [2, 4, 11, 22, 12, 18, 24, 36, 48, 72, 96, 108];
    RATES.iter().position(|&x| x == r).map(|i| 1 << i)
}

/// The association, for the MAC context.
#[derive(Clone, Copy, Debug)]
pub struct Assoc {
    pub aid: u16,
    pub beacon: Beacon,
}

/// MAC_CONTEXT (iwx_mac_ctx_cmd, 148 bytes; iwx_mac_ctxt_cmd): a BSS
/// station `me` of `bssid`, associated or not. Its ACK rates are the
/// mandatory ones (1 and 2 Mb/s on 2.4 GHz; 6, 12 and 24), its EDCA
/// parameters the defaults of a non-QoS BSS.
pub fn mac_context(action: Action, me: [u8; 6], bssid: [u8; 6], ghz5: bool, assoc: Option<Assoc>) -> [u8; 148] {
    let mut c = [0; 148];
    put32(&mut c, 4, action as u32);
    if action == Action::Remove {
        return c;
    }
    put32(&mut c, 8, 5); // FW_MAC_TYPE_BSS_STA
    c[16..22].copy_from_slice(&me);
    c[24..30].copy_from_slice(&bssid);
    put32(&mut c, 32, if ghz5 { 0 } else { 0b11 });
    put32(&mut c, 36, 0b1_0101);
    // The access categories by their FIFO (iwx_ac_to_tx_fifo: BK 1, BE 2,
    // VI 3, VO 4): CWmin, CWmax, AIFSN, TXOP (32 µs units, ×32).
    for (fifo, ecw_min, ecw_max, aifsn, txop) in [(1, 4, 10, 7, 0), (2, 4, 10, 3, 0), (3, 3, 4, 2, 94), (4, 2, 3, 2, 47)] {
        let a = 60 + 8 * fifo;
        put16(&mut c, a, (1 << ecw_min) - 1);
        put16(&mut c, a + 2, (1 << ecw_max) - 1);
        c[a + 4] = aifsn;
        c[a + 5] = 1 << fifo;
        put16(&mut c, a + 6, txop * 32);
    }
    // Group frames; beacons too until the DTIM period is known.
    const ACCEPT_GRP: u32 = 1 << 2;
    const IN_BEACON: u32 = 1 << 6;
    let dtim_known = assoc.is_some_and(|a| a.aid != 0 && a.beacon.dtim_period != 0);
    put32(&mut c, 52, ACCEPT_GRP | if dtim_known { 0 } else { IN_BEACON });
    // The station data (iwx_mac_ctxt_cmd_fill_sta) at 100.
    let b = assoc.map_or(Beacon { tsf: 0, gp2: 0, interval: 100, dtim_period: 1, dtim_count: 0, rates: 0 }, |a| a.beacon);
    if let Some(a) = assoc {
        let dtim_off = u32::from(b.dtim_count) * u32::from(b.interval) * 1024;
        put32(&mut c, 100, 1);
        put32(&mut c, 104, b.gp2.wrapping_add(dtim_off));
        put64(&mut c, 108, b.tsf.wrapping_add(u64::from(dtim_off)));
        put32(&mut c, 140, b.gp2);
        put32(&mut c, 136, u32::from(a.aid & 0x3fff));
    }
    put32(&mut c, 116, u32::from(b.interval));
    put32(&mut c, 124, u32::from(b.interval) * u32::from(b.dtim_period.max(1)));
    put32(&mut c, 132, 10); // listen interval
    c
}

/// BINDING_CONTEXT (iwx_binding_cmd): the MAC context to the PHY context.
pub fn binding(action: Action) -> [u8; 28] {
    let mut c = [0; 28];
    put32(&mut c, 4, action as u32);
    put32(&mut c, 12, FW_CTXT_INVALID);
    put32(&mut c, 16, FW_CTXT_INVALID);
    c
}

/// ADD_STA (iwx_add_sta_cmd, 48 bytes; version 12 in this firmware):
/// add the access point `bssid` as the station, or (`None`) update it.
pub fn add_sta(bssid: Option<[u8; 6]>) -> [u8; 48] {
    let mut c = [0; 48];
    c[0] = u8::from(bssid.is_none());
    if let Some(b) = bssid {
        c[8..14].copy_from_slice(&b);
    }
    c[16] = STATION_ID;
    // No 40 MHz, no MIMO (STA_FLG_FAT_EN_MSK, STA_FLG_MIMO_EN_MSK).
    put32(&mut c, 24, 3 << 26 | 3 << 28);
    c
}

/// ADD_STA modifying the station's DRAIN_FLOW: its frames flushed rather
/// than sent (iwx_drain_sta).
pub fn drain_sta(drain: bool) -> [u8; 48] {
    const DRAIN_FLOW: u32 = 1 << 12;
    let mut c = [0; 48];
    c[0] = 1;
    c[16] = STATION_ID;
    put32(&mut c, 20, if drain { DRAIN_FLOW } else { 0 });
    put32(&mut c, 24, DRAIN_FLOW);
    c
}

/// ADD_STA's answer: ADD_STA_SUCCESS.
pub fn add_sta_ok(r: &[u8]) -> bool {
    r.len() >= 4 && le32(r, 0) & 0xff == 1
}

pub fn remove_sta() -> [u8; 4] {
    [STATION_ID, 0, 0, 0]
}

/// TXPATH_FLUSH (iwx_flush_sta_tids): every TID of the station.
pub fn txpath_flush() -> [u8; 8] {
    let mut c = [0; 8];
    put16(&mut c, 4, 0xffff);
    c
}

/// SCD_QUEUE_CONFIG version 3 (iwx_scd_queue_cfg_cmd, 36 bytes;
/// iwx_enable_txq): add the station's queue of non-QoS frames, its 256
/// descriptors at `tfds` and its byte count table at `byte_counts`.
pub fn queue_add(tfds: u64, byte_counts: u64) -> [u8; 36] {
    let mut c = [0; 36];
    put32(&mut c, 4, 1 << STATION_ID);
    c[8] = MGMT_TID;
    put32(&mut c, 16, 5); // log2(256) - 3
    put64(&mut c, 20, byte_counts);
    put64(&mut c, 28, tfds);
    c
}

/// SCD_QUEUE_CONFIG removing it (iwx_disable_txq).
pub fn queue_remove() -> [u8; 36] {
    let mut c = [0; 36];
    put32(&mut c, 0, 1);
    put32(&mut c, 4, 1 << STATION_ID);
    c[8] = MGMT_TID;
    c
}

/// SCD_QUEUE_CONFIG's answer (iwx_tx_queue_cfg_rsp): the queue the
/// firmware gave, when it starts at index 0 as a new ring must.
pub fn queue_added(r: &[u8]) -> Option<u16> {
    (r.len() == 8 && le16(r, 4) == 0).then(|| le16(r, 0))
}

/// SESSION_PROTECTION (iwx_session_prot_cmd): the firmware stays on the
/// channel for `duration_tu`, or no longer.
pub fn session_protection(add: bool, duration_tu: u32) -> [u8; 24] {
    let mut c = [0; 24];
    put32(&mut c, 4, if add { Action::Add } else { Action::Remove } as u32);
    // conf_id SESSION_PROTECT_CONF_ASSOC (0).
    put32(&mut c, 12, if add { duration_tu } else { 0 });
    c
}

/// REPLY_SF_CFG (iwx_sf_cfg_cmd, 92 bytes; iwx_fill_sf_command): the
/// smart FIFO full on for a legacy association, or off.
pub fn sf_config(full_on: bool) -> [u8; 92] {
    let mut c = [0; 92];
    put32(&mut c, 0, if full_on { 1 } else { 3 });
    put32(&mut c, 4, 4096); // long delay on: SF_W_MARK_SCAN
    put32(&mut c, 8, if full_on { 4096 } else { 8192 }); // LEGACY, else MIMO2
    for i in 0..10 {
        put32(&mut c, 12 + 4 * i, 1_000_000); // long delay aging timer
    }
    // {aging, idle} per scenario: single unicast, aggregated unicast,
    // multicast, block ack, TX re-transmission.
    let full: [(u32, u32); 5] = if full_on {
        [(2016, 320), (2016, 320), (10016, 2016), (2016, 320), (2016, 320)]
    } else {
        [(400, 160); 5]
    };
    for (i, (aging, idle)) in full.iter().enumerate() {
        put32(&mut c, 52 + 8 * i, *aging);
        put32(&mut c, 56 + 8 * i, *idle);
    }
    c
}

/// MCAST_FILTER (iwx_allow_mcast): every group frame of `bssid` up.
pub fn mcast_filter(bssid: [u8; 6]) -> [u8; 12] {
    let mut c = [0; 12];
    c[0] = 1; // filter own
    c[3] = 1; // pass all
    c[4..10].copy_from_slice(&bssid);
    c
}

/// MAC_PM_POWER_TABLE (iwx_mac_power_cmd, 40 bytes): no power saving, a
/// keep-alive every 25 s (iwx_set_pslevel's level 0).
pub fn mac_power() -> [u8; 40] {
    let mut c = [0; 40];
    put16(&mut c, 6, 25);
    c
}

/// TLC_MNG_CONFIG version 4 (iwx_tlc_config_cmd_v4; iwx_rs_init_v4): the
/// firmware's rate scaling over the non-HT `rates`, 20 MHz, antenna A.
pub fn tlc_config(rates: u16) -> [u8; 28] {
    let mut c = [0; 28];
    c[0] = STATION_ID;
    c[6] = 1; // TLC_MNG_CHAIN_A_MSK
    put16(&mut c, 10, rates);
    put16(&mut c, 24, 2312); // IEEE80211_MAX_LEN
    c
}

/// A data queue's TX command (narrow header, then iwx_tx_cmd_gen2 for the
/// 22000 family) with the frame's 802.11 header, padded to 4 bytes:
/// written to `out`, its length returned. The body follows it in a
/// buffer of its own (a third transfer buffer).
pub fn tx_cmd(out: &mut [u8], qid: u8, idx: u8, frame: &[u8], ofdm: bool) -> Option<usize> {
    const CMD_RATE: u32 = 1 << 0;
    const ENCRYPT_DIS: u32 = 1 << 1;
    const HIGH_PRI: u32 = 1 << 2;
    const OFFLD_PAD: u16 = 1 << 13;
    if frame.len() < 24 {
        return None;
    }
    let hdr = header_len(frame).min(frame.len());
    let pad = (4 - hdr % 4) % 4;
    let len = 4 + 20 + hdr + pad;
    if out.len() < len {
        return None;
    }
    out[..len].fill(0);
    out[..4].copy_from_slice(&[TX_CMD, 0, idx, qid]);
    put16(out, 4, frame.len() as u16);
    put16(out, 6, ((hdr / 2) as u16 & 0x1f) << 8 | if pad != 0 { OFFLD_PAD } else { 0 });
    // The daemon encrypts; management frames at the lowest rate, data at
    // the firmware's (rate 0) once TLC_MNG_CONFIG named the rates; an
    // EAPOL frame ahead of the others.
    let data = frame[0] & 0x0c == 0x08;
    let mut flags = ENCRYPT_DIS;
    let mut rate = 0;
    if !data || frame[4] & 1 != 0 {
        flags |= CMD_RATE;
        // rate_n_flags version 2 (TX_CMD notification > 6): antenna A,
        // legacy OFDM (6 Mb/s, index 0) or CCK (1 Mb/s, index 0).
        rate = 1 << 14 | if ofdm { 1 << 8 } else { 0 };
    }
    if data && is_eapol(frame, hdr) {
        flags |= HIGH_PRI;
    }
    put32(out, 8, flags);
    put32(out, 20, rate);
    out[24..24 + hdr].copy_from_slice(&frame[..hdr]);
    Some(len)
}

/// Whether data frame `f` (header `hdr` bytes) is EAPOL's (LLC/SNAP, then
/// EtherType 0x888e).
fn is_eapol(f: &[u8], hdr: usize) -> bool {
    f.get(hdr..hdr + 8) == Some(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e])
}

/// A queue's byte count table entry (iwx_tx_update_byte_tbl, before
/// AX210): the frame's length in 4-byte words, and how many 64-byte
/// chunks of the descriptor to fetch (less one).
pub fn byte_count(frame_len: usize, num_tbs: usize) -> u16 {
    let filled = 2 + 10 * num_tbs;
    (frame_len.div_ceil(4) as u16) | ((filled.div_ceil(64) - 1) as u16) << 12
}

/// TX_CMD's response (iwx_tx_resp, one frame): whether it went
/// (TX_STATUS_SUCCESS or DIRECT_DONE), and the ring index the frames
/// before which are done.
pub fn tx_done(r: &[u8]) -> Option<(bool, u16)> {
    if r.len() < 48 || r[0] != 1 {
        return None;
    }
    let status = le16(r, 40) & 0xff;
    Some((status == 1 || status == 2, le32(r, 44) as u16))
}

/// The duration of the session protection for joining: authentication
/// and association each sent up to three times, 300 ms apart.
pub fn join_protection_tu(beacon_interval: u16) -> u32 {
    (u32::from(beacon_interval) * 9).max(2000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn beacon_frame() -> alloc::vec::Vec<u8> {
        let mut f = vec![0x80, 0, 0, 0];
        f.extend_from_slice(&[0xff; 6]);
        f.extend_from_slice(&[2, 0, 0, 0, 0, 1]);
        f.extend_from_slice(&[2, 0, 0, 0, 0, 1]);
        f.extend_from_slice(&[0, 0]);
        f.extend_from_slice(&0x1122_3344_5566u64.to_le_bytes());
        f.extend_from_slice(&100u16.to_le_bytes());
        f.extend_from_slice(&[0x01, 0x04]);
        f.extend_from_slice(&[0, 3, b'n', b'e', b't']);
        f.extend_from_slice(&[1, 4, 0x82, 0x84, 0x8b, 0x96]);
        f.extend_from_slice(&[5, 4, 1, 3, 0, 0]);
        f.extend_from_slice(&[50, 2, 0x0c, 0x6c]);
        f
    }

    #[test]
    fn beacon_timing() {
        let (bssid, b) = beacon(&beacon_frame(), 777).unwrap();
        assert_eq!(bssid, [2, 0, 0, 0, 0, 1]);
        assert_eq!(
            b,
            Beacon { tsf: 0x1122_3344_5566, gp2: 777, interval: 100, dtim_period: 3, dtim_count: 1, rates: 0x81f }
        );
        assert!(beacon(&[0x08; 40], 0).is_none());
    }

    #[test]
    fn contexts() {
        let p = phy_context(Action::Add, Channel { number: 36, ghz5: true, passive: false });
        assert_eq!((le32(&p, 4), le32(&p, 8), p[12]), (1, 36, PHY_BAND_5));
        assert_eq!(le32(&rlc_config(3), 4), 3 << 1 | 1 << 10 | 1 << 12);
        let me = [2, 0, 0, 0, 0, 9];
        let ap = [2, 0, 0, 0, 0, 1];
        let m = mac_context(Action::Add, me, ap, false, None);
        assert_eq!((le32(&m, 8), &m[16..22], &m[24..30]), (5, &me[..], &ap[..]));
        assert_eq!((le32(&m, 32), le32(&m, 36), le32(&m, 52)), (3, 0x15, 0x44));
        // BE on FIFO 2: CWmin 15, CWmax 1023, AIFSN 3.
        assert_eq!((le16(&m, 76), le16(&m, 78), m[80], m[81]), (15, 1023, 3, 4));
        assert_eq!(le32(&m, 100), 0);
        let (_, b) = beacon(&beacon_frame(), 5000).unwrap();
        let m = mac_context(Action::Modify, me, ap, false, Some(Assoc { aid: 0xc003, beacon: b }));
        assert_eq!((le32(&m, 100), le32(&m, 104), le32(&m, 136)), (1, 5000 + 102_400, 3));
        assert_eq!((le32(&m, 116), le32(&m, 124), le32(&m, 140)), (100, 300, 5000));
        assert_eq!(le32(&m, 52), 0x04); // beacons filtered once the DTIM is known
        assert_eq!(&mac_context(Action::Remove, me, ap, false, None)[8..], &[0; 140][..]);
        let b = binding(Action::Add);
        assert_eq!((le32(&b, 4), le32(&b, 8), le32(&b, 12), le32(&b, 20)), (1, 0, FW_CTXT_INVALID, 0));
    }

    #[test]
    fn station_and_queue() {
        let s = add_sta(Some([2, 0, 0, 0, 0, 1]));
        assert_eq!((s[0], &s[8..14], s[16], le32(&s, 24)), (0, &[2, 0, 0, 0, 0, 1][..], 0, 0x3c00_0000));
        assert_eq!(add_sta(None)[0], 1);
        assert_eq!(le32(&drain_sta(true), 20), 1 << 12);
        assert!(add_sta_ok(&[1, 0, 0, 0]) && !add_sta_ok(&[2, 0, 0, 0]));
        let q = queue_add(0x1000_0000, 0x1001_0000);
        assert_eq!((le32(&q, 0), le32(&q, 4), q[8], le32(&q, 16)), (0, 1, 15, 5));
        assert_eq!((le64(&q, 20), le64(&q, 28)), (0x1001_0000, 0x1000_0000));
        assert_eq!(le32(&queue_remove(), 0), 1);
        assert_eq!(queue_added(&[1, 0, 0, 0, 0, 0, 0, 0]), Some(1));
        assert_eq!(queue_added(&[1, 0, 0, 0, 5, 0, 0, 0]), None);
        assert_eq!(le32(&session_protection(true, 900), 12), 900);
        let sf = sf_config(true);
        assert_eq!((le32(&sf, 0), le32(&sf, 8), le32(&sf, 52), le32(&sf, 56)), (1, 4096, 2016, 320));
        assert_eq!(le32(&sf_config(false), 0), 3);
        assert_eq!(le16(&tlc_config(0xff0), 10), 0xff0);
    }

    #[test]
    fn transmit() {
        let mut out = [0u8; 64];
        // An authentication frame: 24-byte header, lowest rate (CCK).
        let mut auth = vec![0xb0, 0, 0, 0, 2, 0, 0, 0, 0, 1];
        auth.resize(30, 0);
        let n = tx_cmd(&mut out, 1, 7, &auth, false).unwrap();
        assert_eq!((n, &out[..4], le16(&out, 4), le16(&out, 6)), (48, &[0x1c, 0, 7, 1][..], 30, 0x0c00));
        assert_eq!((le32(&out, 8), le32(&out, 20)), (0b11, 1 << 14));
        assert_eq!(&out[24..34], &auth[..10]);
        // A unicast EAPOL data frame: the firmware's rate, high priority.
        let mut eapol = vec![0x08, 0x01, 0, 0, 2, 0, 0, 0, 0, 1];
        eapol.resize(24, 0);
        eapol.extend_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0, 0x88, 0x8e, 1, 2]);
        tx_cmd(&mut out, 1, 0, &eapol, true).unwrap();
        assert_eq!((le32(&out, 8), le32(&out, 20)), (0b110, 0));
        // A QoS data header (26 bytes) is padded to 28.
        let mut qos = vec![0x88, 0x01, 0, 0, 2, 0, 0, 0, 0, 1];
        qos.resize(40, 0);
        let n = tx_cmd(&mut out, 1, 0, &qos, true).unwrap();
        assert_eq!((n, le16(&out, 6)), (4 + 20 + 28, 0x2d00));
        assert_eq!(byte_count(30, 3), 8);
        let mut r = vec![0u8; 48];
        r[0] = 1;
        put16(&mut r, 40, 1);
        put32(&mut r, 44, 5);
        assert_eq!(tx_done(&r), Some((true, 5)));
        put16(&mut r, 40, 0x83);
        assert_eq!(tx_done(&r), Some((false, 5)));
    }
}
