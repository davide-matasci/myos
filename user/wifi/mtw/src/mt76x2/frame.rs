//! The MT76x2U's frames on the bulk endpoints (Linux's
//! mt76u_process_rx_entry, mt76x02_mac_process_rx,
//! mt76x02u_tx_prepare_skb, mt76x02_mac_write_txwi; notices in LICENSE).
//! In: a DMA header (the length), the RXWI (flags, length, rate, each
//! chain's signal), the frame, 4 bytes of FCE info; one frame a transfer
//! (receive aggregation is off), parsed as aggregated anyway. Out: the
//! TXINFO, the TXWI (rate, ACK, station entry, length), the frame with
//! its header padded to 4 bytes, zeros to a 4-byte boundary and 4 more.

use alloc::vec::Vec;

use super::reg::*;
use crate::rate::TxRate;
use crate::reg::RATES;
use crate::rx::hdrlen;

/// A frame heard.
pub struct Frame<'a> {
    /// The 802.11 frame, from its header on, without the FCS.
    pub frame: &'a [u8],
    /// The station entry it came from (0xff: none).
    pub wcid: u8,
    /// Each chain's signal as the RXWI says it: `Dev::rssi_dbm` makes it
    /// dBm.
    pub rssi: [i8; 2],
}

/// Each good frame in transfer `xfer` to `f`: the damaged ones (a bad
/// FCS) are dropped, a header the MAC padded (L2PAD) is put back against
/// its body. The daemon keeps the keys, so a protected frame comes as it
/// was sent; the ICV and MIC flags of a MAC that has no key are not
/// looked at (mt76x02_mac_process_rx does not).
pub fn frames(xfer: &mut [u8], mut f: impl FnMut(Frame)) {
    let mut at = 0;
    while xfer.len() - at >= DMA_HDR_LEN + RXWI_LEN + FCE_INFO_LEN {
        let dmalen = usize::from(u16::from_le_bytes([xfer[at], xfer[at + 1]]));
        if dmalen == 0 || dmalen & 3 != 0 || DMA_HDR_LEN + dmalen + FCE_INFO_LEN > xfer.len() - at {
            break;
        }
        if let Some(frame) = frame(&mut xfer[at + DMA_HDR_LEN..at + DMA_HDR_LEN + dmalen]) {
            f(frame);
        }
        at += DMA_HDR_LEN + dmalen + FCE_INFO_LEN;
    }
}

/// One RXWI and its frame.
fn frame(buf: &mut [u8]) -> Option<Frame<'_>> {
    if buf.len() < RXWI_LEN + 10 {
        return None;
    }
    let word = |at: usize| u32::from_le_bytes(buf[at..at + 4].try_into().unwrap());
    let (rxinfo, ctl) = (word(0), word(4));
    if rxinfo & RXINFO_CRCERR != 0 {
        return None;
    }
    let len = ((ctl >> RXWI_CTL_MPDU_LEN_SHIFT) & RXWI_CTL_MPDU_LEN) as usize;
    let rssi = [buf[12] as i8, buf[13] as i8];
    let mut wh = RXWI_LEN;
    if rxinfo & RXINFO_L2PAD != 0 {
        let hdr = hdrlen(buf[wh], buf[wh + 1]);
        if wh + hdr + 2 > buf.len() {
            return None;
        }
        buf.copy_within(wh..wh + hdr, wh + 2);
        wh += 2;
    }
    if wh + len > buf.len() || len < 10 {
        return None;
    }
    if rxinfo & RXINFO_DECRYPT != 0 {
        buf[wh + 1] &= !0x40;
    }
    Some(Frame { frame: &buf[wh..wh + len], wcid: (ctl & RXWI_CTL_WCID) as u8, rssi })
}

/// How a frame goes out: the link's rate for data to the access point,
/// the band's slowest otherwise.
pub struct Tx {
    pub rate: TxRate,
    /// The station entry of the access point (0xff: none).
    pub wcid: u8,
    pub short_preamble: bool,
    pub ghz5: bool,
    /// ASIC_VERSION's low half (the TXWI's streams field depends on it).
    pub rev: u16,
}

/// Where and how frame `frame` goes: the data endpoint (of `Endpoints`)
/// of its access category (a management frame voice's, as mac80211
/// queues it), and the transfer. A data frame to the access point goes at
/// `tx.rate` as its table entry, the rest at 1 Mb/s, or 6 on 5 GHz (a
/// management or group frame, which every station hears); a frame that
/// wants an ACK gets the ACK's duration. The frame carries its own IV,
/// if any (TXD_WIV): the daemon encrypts.
pub fn transfer(frame: &[u8], tx: &Tx) -> (usize, Vec<u8>) {
    let (fc0, fc1) = (frame[0], frame[1]);
    let data = fc0 & 0x0c == 0x08;
    let qos = data && fc0 & 0x80 != 0;
    let hdr = hdrlen(fc0, fc1).min(frame.len());
    let qos_ctl = if qos { frame.get(hdr - 2).copied().unwrap_or(0) } else { 0 };
    let ac = if data { crate::tx::ac(qos_ctl & 7) } else { 3 };
    let multicast = frame[4] & 1 != 0;
    let slowest = TxRate::Legacy(if tx.ghz5 { 4 } else { crate::reg::RIDX_CCK1 });
    let rate = if multicast || !data { slowest } else { tx.rate };
    let short = tx.short_preamble && !tx.ghz5 && rate != TxRate::Legacy(crate::reg::RIDX_CCK1);
    let word = match rate {
        TxRate::Legacy(r) if RATES[r].ofdm => PHY_OFDM << RATE_PHY_SHIFT | u16::from(RATES[r].mcs),
        TxRate::Legacy(r) => PHY_CCK << RATE_PHY_SHIFT | u16::from(RATES[r].mcs) | if short { CCK_SHORT } else { 0 },
        TxRate::Ht(mcs) => PHY_HT << RATE_PHY_SHIFT | u16::from(mcs),
    };
    let txstream = if tx.rev >= REV_E4 {
        TXSTREAM_E4
    } else if tx.rev >= REV_E3 && !matches!(rate, TxRate::Ht(_)) {
        TXSTREAM_E3_LEGACY
    } else {
        0
    };
    // No ACK for a group frame or a QoS one that says so; the MAC's
    // sequence number for a frame without QoS.
    let ack = !multicast && (qos_ctl >> 5) & 3 != 1;
    let mut ack_ctl = if qos { 0 } else { TXWI_ACK_NSEQ };
    if ack {
        ack_ctl |= TXWI_ACK_REQ;
    }

    let pad = if hdr % 4 != 0 { 2 } else { 0 };
    let body = TXWI_LEN + frame.len() + pad;
    let padded = (body + 3) & !3;
    let mut out = Vec::with_capacity(4 + padded + 4);
    let info = padded as u32 | TXD_80211 | TXD_WIV | QSEL_EDCA << TXD_QSEL_SHIFT | WLAN_PORT << MSG_PORT_SHIFT;
    out.extend_from_slice(&info.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&word.to_le_bytes());
    out.extend_from_slice(&[ack_ctl, if data { tx.wcid } else { 0xff }]);
    out.extend_from_slice(&(frame.len() as u16).to_le_bytes());
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&[0, txstream, 0, 0]);

    let at = out.len();
    out.extend_from_slice(&frame[..hdr]);
    out.extend_from_slice(&[0; 2][..pad]);
    out.extend_from_slice(&frame[hdr..]);
    if ack {
        let ctl = &RATES[rate.ctl_ridx()];
        let dur = if short { ctl.sp_ack_dur } else { ctl.lp_ack_dur };
        out[at + 2..at + 4].copy_from_slice(&dur.to_le_bytes());
    }
    out.resize(4 + padded + 4, 0);
    (ac, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One transfer: DMA header, RXWI, the frame (padded after its header
    /// when `pad`), the FCE info.
    fn xfer(out: &mut Vec<u8>, rxinfo: u32, frame: &[u8], pad: bool) {
        let mut body = alloc::vec![0u8; RXWI_LEN];
        body[0..4].copy_from_slice(&(rxinfo | if pad { RXINFO_L2PAD } else { 0 }).to_le_bytes());
        let ctl = 7 | (frame.len() as u32) << RXWI_CTL_MPDU_LEN_SHIFT;
        body[4..8].copy_from_slice(&ctl.to_le_bytes());
        body[12] = -40i8 as u8;
        body[13] = -44i8 as u8;
        if pad {
            body.extend_from_slice(&frame[..26]);
            body.extend_from_slice(&[0xee, 0xee]);
            body.extend_from_slice(&frame[26..]);
        } else {
            body.extend_from_slice(frame);
        }
        body.resize((body.len() + 3) & !3, 0);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out.extend_from_slice(&[0; FCE_INFO_LEN]);
    }

    #[test]
    fn heard() {
        let beacon: Vec<u8> = (0..40u8).map(|i| if i == 0 { 0x80 } else { i }).collect();
        let qos: Vec<u8> = (0..34u8).map(|i| if i == 0 { 0x88 } else { i & 0x3f }).collect();
        let mut x = Vec::new();
        xfer(&mut x, 0, &beacon, false);
        xfer(&mut x, RXINFO_CRCERR, &beacon, false);
        xfer(&mut x, 0, &qos, true);
        let mut heard = Vec::new();
        frames(&mut x, |f| heard.push((f.frame.to_vec(), f.wcid, f.rssi)));
        assert_eq!(heard, [(beacon, 7, [-40, -44]), (qos, 7, [-40, -44])]);
        let mut n = 0;
        frames(&mut [0u8; 12], |_| n += 1);
        assert_eq!(n, 0);
    }

    fn header(fc0: u8, da: u8) -> Vec<u8> {
        let mut f = alloc::vec![fc0, 0, 0, 0, da, 2, 3, 4, 5, 6];
        f.extend_from_slice(&[0x10; 14]);
        f
    }

    #[test]
    fn data_and_management() {
        let tx = Tx { rate: TxRate::Ht(13), wcid: 1, short_preamble: false, ghz5: true, rev: 0x0044 };
        // QoS data of priority 0: best effort, the header padded (26
        // bytes), MCS 13, two streams, an ACK and the frame's own sequence.
        let mut qos = header(0x88, 0x02);
        qos.extend_from_slice(&[0, 0]);
        qos.extend_from_slice(b"payload");
        let (ac, x) = transfer(&qos, &tx);
        assert_eq!(ac, 0);
        let info = u32::from_le_bytes(x[..4].try_into().unwrap());
        let body = TXWI_LEN + qos.len() + 2;
        assert_eq!(info & 0xffff, ((body + 3) & !3) as u32);
        assert_eq!(info & !0xffff, TXD_80211 | TXD_WIV | QSEL_EDCA << TXD_QSEL_SHIFT);
        assert_eq!(x.len(), 4 + ((body + 3) & !3) + 4);
        let w = &x[4..];
        assert_eq!(u16::from_le_bytes([w[2], w[3]]), PHY_HT << RATE_PHY_SHIFT | 13);
        assert_eq!((w[4], w[5], usize::from(u16::from_le_bytes([w[6], w[7]]))), (TXWI_ACK_REQ, 1, qos.len()));
        assert_eq!(w[17], TXSTREAM_E4);
        let f = &x[4 + TXWI_LEN..];
        assert_eq!((&f[4..26], &f[26..28], &f[28..35]), (&qos[4..26], &[0u8, 0][..], &b"payload"[..]));
        // A probe request on 5 GHz: voice, 6 Mb/s, no ACK, no entry.
        let (ac, x) = transfer(&header(0x40, 0xff), &tx);
        let w = &x[4..];
        assert_eq!((ac, u16::from_le_bytes([w[2], w[3]]), w[4], w[5]), (3, PHY_OFDM << RATE_PHY_SHIFT, TXWI_ACK_NSEQ, 0xff));
        // On 2.4 GHz, 1 Mb/s.
        let tx = Tx { ghz5: false, rev: 0x0022, ..tx };
        let (_, x) = transfer(&header(0xb0, 0x02), &tx);
        assert_eq!((u16::from_le_bytes([x[6], x[7]]), x[4 + 17]), (PHY_CCK << RATE_PHY_SHIFT, TXSTREAM_E3_LEGACY));
    }
}
