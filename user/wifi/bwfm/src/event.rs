//! The firmware's events (Ethernet frames of Broadcom's EtherType on the
//! event channel) and what they carry: a scan's results, the link's state;
//! and the channel specifications the firmware speaks.

use alloc::vec::Vec;

use crate::reg::*;

/// An event message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event<'a> {
    pub kind: u32,
    pub status: u32,
    pub reason: u32,
    pub flags: u16,
    pub addr: [u8; 6],
    pub ifidx: u8,
    pub data: &'a [u8],
}

fn be16(b: &[u8], i: usize) -> u16 {
    u16::from_be_bytes([b[i], b[i + 1]])
}
fn be32(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
fn le16(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn le32(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// The event in `frame` (an Ethernet frame), if it is one.
pub fn parse(frame: &[u8]) -> Option<Event<'_>> {
    if frame.len() < EVENT_LEN || be16(frame, 12) != ETHERTYPE_LINK_CTL {
        return None;
    }
    let h = &frame[14..14 + EVENT_ETHHDR_LEN];
    if h[5..8] != BRCM_OUI || be16(h, 8) != BRCM_SUBTYPE_EVENT {
        return None;
    }
    let m = &frame[14 + EVENT_ETHHDR_LEN..];
    let datalen = be32(m, 20) as usize;
    let data = frame.get(EVENT_LEN..EVENT_LEN + datalen)?;
    let mut addr = [0u8; 6];
    addr.copy_from_slice(&m[24..30]);
    Some(Event { kind: be32(m, 4), status: be32(m, 8), reason: be32(m, 12), flags: be16(m, 2), addr, ifidx: m[46], data })
}

/// An event frame, as the firmware sends one (for the model, and the
/// tests): `src` the station, `kind`, `status`, `reason`, `flags`, `addr`
/// and `data`.
pub fn build(src: [u8; 6], kind: u32, status: u32, reason: u32, flags: u16, addr: [u8; 6], data: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(EVENT_LEN + data.len());
    f.extend_from_slice(&[0xff; 6]);
    f.extend_from_slice(&src);
    f.extend_from_slice(&ETHERTYPE_LINK_CTL.to_be_bytes());
    // Broadcom header: subtype, length, version, OUI, user subtype.
    f.extend_from_slice(&0x8001u16.to_be_bytes());
    f.extend_from_slice(&((EVENT_MSG_LEN + data.len()) as u16).to_be_bytes());
    f.push(0);
    f.extend_from_slice(&BRCM_OUI);
    f.extend_from_slice(&BRCM_SUBTYPE_EVENT.to_be_bytes());
    // The message.
    f.extend_from_slice(&1u16.to_be_bytes());
    f.extend_from_slice(&flags.to_be_bytes());
    f.extend_from_slice(&kind.to_be_bytes());
    f.extend_from_slice(&status.to_be_bytes());
    f.extend_from_slice(&reason.to_be_bytes());
    f.extend_from_slice(&0u32.to_be_bytes());
    f.extend_from_slice(&(data.len() as u32).to_be_bytes());
    f.extend_from_slice(&addr);
    f.extend_from_slice(&[0u8; 16]);
    f.extend_from_slice(&[0, 0]);
    f.extend_from_slice(data);
    f
}

/// The mask of the events wanted (the `event_msgs` variable).
pub fn mask(events: &[u32]) -> [u8; EVENT_MASK_LEN] {
    let mut m = [0u8; EVENT_MASK_LEN];
    for &e in events {
        m[e as usize / 8] |= 1 << (e % 8);
    }
    m
}

/// A network a scan found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scanned {
    pub bssid: [u8; 6],
    pub ssid: Vec<u8>,
    pub channel: u8,
    pub rssi: i16,
    pub beacon_period: u16,
    pub capability: u16,
    /// The beacon's information elements.
    pub ies: Vec<u8>,
}

/// The networks in an ESCAN_RESULT event's data (`escan_results` with its
/// `bss_info` records), `io_type` the firmware's channel encoding.
pub fn escan_results(data: &[u8], io_type: u32) -> Vec<Scanned> {
    let mut out = Vec::new();
    if data.len() < ESCAN_RESULTS_LEN {
        return out;
    }
    let count = usize::from(le16(data, 10));
    let mut at = ESCAN_RESULTS_LEN;
    for _ in 0..count {
        let Some(rec) = data.get(at..) else {
            break;
        };
        if rec.len() < BSS_INFO_LEN {
            break;
        }
        let len = le32(rec, BSS_INFO_LENGTH) as usize;
        if len < BSS_INFO_LEN || len > rec.len() {
            break;
        }
        let rec = &rec[..len];
        if let Some(s) = bss_info(rec, io_type) {
            out.push(s);
        }
        at += len;
    }
    out
}

/// One `bss_info` record.
pub fn bss_info(rec: &[u8], io_type: u32) -> Option<Scanned> {
    if rec.len() < BSS_INFO_LEN {
        return None;
    }
    let mut bssid = [0u8; 6];
    bssid.copy_from_slice(&rec[BSS_INFO_BSSID..BSS_INFO_BSSID + 6]);
    let ssid_len = usize::from(rec[BSS_INFO_SSID_LEN]).min(MAX_SSID_LEN);
    let ie_off = usize::from(le16(rec, BSS_INFO_IE_OFFSET));
    let ie_len = le32(rec, BSS_INFO_IE_LENGTH) as usize;
    let ies = rec.get(ie_off..ie_off.checked_add(ie_len)?)?;
    let chanspec = u32::from(le16(rec, BSS_INFO_CHANSPEC));
    let ctl_ch = rec[BSS_INFO_CTL_CH];
    Some(Scanned {
        bssid,
        ssid: rec[BSS_INFO_SSID..BSS_INFO_SSID + ssid_len].to_vec(),
        channel: if ctl_ch != 0 { ctl_ch } else { spec2chan(chanspec, io_type) },
        rssi: le16(rec, BSS_INFO_RSSI) as i16,
        beacon_period: le16(rec, BSS_INFO_BEACON_PERIOD),
        capability: le16(rec, BSS_INFO_CAPABILITY),
        ies: ies.to_vec(),
    })
}

/// A `bss_info` record, as the firmware writes one (for the model).
pub fn build_bss_info(bssid: [u8; 6], ssid: &[u8], chanspec: u16, rssi: i16, capability: u16, ies: &[u8]) -> Vec<u8> {
    let len = BSS_INFO_LEN + ies.len();
    let mut r = Vec::from([0u8; BSS_INFO_LEN]);
    r[0..4].copy_from_slice(&109u32.to_le_bytes());
    r[BSS_INFO_LENGTH..BSS_INFO_LENGTH + 4].copy_from_slice(&(len as u32).to_le_bytes());
    r[BSS_INFO_BSSID..BSS_INFO_BSSID + 6].copy_from_slice(&bssid);
    r[BSS_INFO_BEACON_PERIOD..BSS_INFO_BEACON_PERIOD + 2].copy_from_slice(&100u16.to_le_bytes());
    r[BSS_INFO_CAPABILITY..BSS_INFO_CAPABILITY + 2].copy_from_slice(&capability.to_le_bytes());
    let n = ssid.len().min(MAX_SSID_LEN);
    r[BSS_INFO_SSID_LEN] = n as u8;
    r[BSS_INFO_SSID..BSS_INFO_SSID + n].copy_from_slice(&ssid[..n]);
    r[BSS_INFO_CHANSPEC..BSS_INFO_CHANSPEC + 2].copy_from_slice(&chanspec.to_le_bytes());
    r[BSS_INFO_RSSI..BSS_INFO_RSSI + 2].copy_from_slice(&rssi.to_le_bytes());
    r[BSS_INFO_CTL_CH] = (chanspec & 0xff) as u8;
    r[BSS_INFO_IE_OFFSET..BSS_INFO_IE_OFFSET + 2].copy_from_slice(&(BSS_INFO_LEN as u16).to_le_bytes());
    r[BSS_INFO_IE_LENGTH..BSS_INFO_IE_LENGTH + 4].copy_from_slice(&(ies.len() as u32).to_le_bytes());
    r.extend_from_slice(ies);
    r
}

/// An `escan_results` header over `records` (for the model).
pub fn build_escan_results(records: &[Vec<u8>]) -> Vec<u8> {
    let total: usize = records.iter().map(Vec::len).sum();
    let mut out = Vec::with_capacity(ESCAN_RESULTS_LEN + total);
    out.extend_from_slice(&((ESCAN_RESULTS_LEN + total) as u32).to_le_bytes());
    out.extend_from_slice(&109u32.to_le_bytes());
    out.extend_from_slice(&0x1234u16.to_le_bytes());
    out.extend_from_slice(&(records.len() as u16).to_le_bytes());
    for r in records {
        out.extend_from_slice(r);
    }
    out
}

/// The channel a specification names (its control channel).
pub fn spec2chan(spec: u32, io_type: u32) -> u8 {
    let ch = (spec & CHANSPEC_CHAN_MASK) as i32;
    let ch = if io_type == IO_TYPE_D11N {
        match (spec & CHANSPEC_D11N_BW_MASK, spec & CHANSPEC_D11N_SB_MASK) {
            (CHANSPEC_D11N_BW_40, CHANSPEC_D11N_SB_L) => ch - 2,
            (CHANSPEC_D11N_BW_40, CHANSPEC_D11N_SB_U) => ch + 2,
            _ => ch,
        }
    } else {
        match (spec & CHANSPEC_D11AC_BW_MASK, spec & CHANSPEC_D11AC_SB_MASK) {
            (CHANSPEC_D11AC_BW_40, CHANSPEC_D11AC_SB_LLL) => ch - 2,
            (CHANSPEC_D11AC_BW_40, CHANSPEC_D11AC_SB_LLU) => ch + 2,
            (CHANSPEC_D11AC_BW_80, CHANSPEC_D11AC_SB_LLL) => ch - 6,
            (CHANSPEC_D11AC_BW_80, CHANSPEC_D11AC_SB_LLU) => ch - 2,
            (CHANSPEC_D11AC_BW_80, CHANSPEC_D11AC_SB_LUL) => ch + 2,
            (CHANSPEC_D11AC_BW_80, CHANSPEC_D11AC_SB_LUU) => ch + 6,
            _ => ch,
        }
    };
    ch.clamp(0, 255) as u8
}

/// The specification of 2.4 GHz `channel`, 20 MHz wide.
pub fn chan2spec(channel: u8, io_type: u32) -> u16 {
    let ch = u32::from(channel) & CHANSPEC_CHAN_MASK;
    (if io_type == IO_TYPE_D11N {
        ch | CHANSPEC_D11N_SB_N | CHANSPEC_D11N_BW_20 | CHANSPEC_D11N_BND_2G
    } else {
        ch | CHANSPEC_D11AC_SB_LLL | CHANSPEC_D11AC_BW_20 | CHANSPEC_D11AC_BND_2G
    }) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip() {
        let f = build([2; 6], E_LINK, E_STATUS_SUCCESS, 0, EVENT_FLAG_LINK_UP, [5; 6], b"xy");
        let e = parse(&f).unwrap();
        assert_eq!((e.kind, e.status, e.reason, e.flags, e.addr, e.data), (E_LINK, 0, 0, 1, [5; 6], &b"xy"[..]));
        assert!(parse(&f[..40]).is_none());
        let mut plain = f.clone();
        plain[12] = 0x08;
        assert!(parse(&plain).is_none());
        let m = mask(&[E_LINK, E_ESCAN_RESULT]);
        assert_eq!((m[2], m[8]), (1, 1 << 5));
    }

    #[test]
    fn scan_results() {
        let a = build_bss_info([1; 6], b"net-a", chan2spec(6, IO_TYPE_D11AC), -40, 0x0411, &[0, 5, b'n', b'e', b't', b'-', b'a']);
        let b = build_bss_info([2; 6], b"b", chan2spec(11, IO_TYPE_D11AC), -70, 0x0401, &[]);
        let data = build_escan_results(&[a, b]);
        let found = escan_results(&data, IO_TYPE_D11AC);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].channel, found[0].rssi, &found[0].ssid[..], found[0].ies.len()), (6, -40, &b"net-a"[..], 7));
        assert_eq!((found[1].channel, found[1].bssid), (11, [2; 6]));
        assert!(escan_results(&data[..20], IO_TYPE_D11AC).is_empty());
    }

    #[test]
    fn channel_specs() {
        assert_eq!(spec2chan(u32::from(chan2spec(3, IO_TYPE_D11N)), IO_TYPE_D11N), 3);
        assert_eq!(spec2chan(u32::from(chan2spec(13, IO_TYPE_D11AC)), IO_TYPE_D11AC), 13);
        assert_eq!(spec2chan(5 | CHANSPEC_D11N_BW_40 | CHANSPEC_D11N_SB_U, IO_TYPE_D11N), 7);
        assert_eq!(spec2chan(42 | CHANSPEC_D11AC_BW_80 | CHANSPEC_D11AC_SB_LUU, IO_TYPE_D11AC), 48);
    }
}
