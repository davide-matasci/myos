//! What the bulk IN endpoint brings (mtw_rxeof, mtw_rx_frame; notices in
//! LICENSE): the MAC aggregates the frames it heard into one transfer,
//! each after a 4-byte DMA descriptor (its length) and a receive
//! information block (RXWI: flags, length, signal), with 4 bytes after.

use crate::reg::*;

/// A frame heard.
pub struct Frame<'a> {
    /// The 802.11 frame, from its header on, without the FCS.
    pub frame: &'a [u8],
    /// The signal (RXWI rssi, of the strongest chain) in the MAC's units:
    /// `Dev::rssi_dbm` makes it dBm.
    pub rssi: u8,
}

/// Each good frame in transfer `xfer` to `f`: the damaged ones (a bad
/// FCS, ICV or MIC) are dropped, a header the MAC padded (L2PAD) is put
/// back against its body. A protected frame the MAC decrypted (a key in
/// its table) comes unprotected; the daemon keeps the keys itself, so its
/// frames come as they were sent.
pub fn frames(xfer: &mut [u8], mut f: impl FnMut(Frame)) {
    let mut at = 0;
    while xfer.len() - at > 8 {
        let dmalen = (u32::from_le_bytes(xfer[at..at + 4].try_into().unwrap()) & RXD_LEN) as usize;
        if dmalen == 0 || dmalen & 3 != 0 || dmalen + 8 > xfer.len() - at {
            break;
        }
        if let Some(frame) = frame(&mut xfer[at + 4..at + 4 + dmalen]) {
            f(frame);
        }
        at += dmalen + 8;
    }
}

/// One RXWI and its frame.
fn frame(buf: &mut [u8]) -> Option<Frame<'_>> {
    if buf.len() < RXWI_LEN + 10 {
        return None;
    }
    let flags = u32::from_le_bytes(buf[0..4].try_into().unwrap());
    let len = usize::from(u16::from_le_bytes([buf[8], buf[9]]) & 0xfff);
    if flags & (RX_CRCERR | RX_ICVERR | RX_MICERR) != 0 {
        return None;
    }
    let rssi = buf[12];
    let mut wh = RXWI_LEN;
    if flags & RX_L2PAD != 0 {
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
    if flags & RX_DEC != 0 {
        buf[wh + 1] &= !0x40;
    }
    Some(Frame { frame: &buf[wh..wh + len], rssi })
}

/// The length of the 802.11 header that starts with frame control bytes
/// `fc0`, `fc1` (ieee80211_get_hdrlen).
pub fn hdrlen(fc0: u8, fc1: u8) -> usize {
    let mut n = 24;
    if fc1 & 3 == 3 {
        n += 6;
    }
    // A QoS data frame: its QoS control, and an HT control when ordered.
    if fc0 & 0x0c == 0x08 && fc0 & 0x80 != 0 {
        n += 2;
        if fc1 & 0x80 != 0 {
            n += 4;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    /// One aggregated frame: DMA descriptor, RXWI, the frame (padded
    /// after its header when `pad`), 4 bytes after.
    fn agg(out: &mut Vec<u8>, flags: u32, frame: &[u8], pad: bool) {
        let mut body = alloc::vec![0u8; RXWI_LEN];
        body[0..4].copy_from_slice(&(flags | if pad { RX_L2PAD } else { 0 }).to_le_bytes());
        body[8..10].copy_from_slice(&(frame.len() as u16).to_le_bytes());
        body[12] = 40;
        if pad {
            body.extend_from_slice(&frame[..24]);
            body.extend_from_slice(&[0xee, 0xee]);
            body.extend_from_slice(&frame[24..]);
        } else {
            body.extend_from_slice(frame);
        }
        while body.len() % 4 != 0 {
            body.push(0);
        }
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out.extend_from_slice(&[0; 4]);
    }

    #[test]
    fn aggregated() {
        let beacon: Vec<u8> = (0..40u8).map(|i| if i == 0 { 0x80 } else { i }).collect();
        let data: Vec<u8> = (0..30u8).map(|i| if i == 0 { 0x08 } else { i | 0x40 }).collect();
        let mut xfer = Vec::new();
        agg(&mut xfer, 0, &beacon, false);
        agg(&mut xfer, RX_CRCERR, &beacon, false);
        agg(&mut xfer, 0, &data, true);
        let mut heard = Vec::new();
        frames(&mut xfer, |f| heard.push((f.frame.to_vec(), f.rssi)));
        assert_eq!(heard, [(beacon, 40), (data, 40)]);
    }

    #[test]
    fn short_or_bad() {
        let mut xfer = alloc::vec![0u8; 12];
        xfer[0] = 6;
        let mut n = 0;
        frames(&mut xfer, |_| n += 1);
        frames(&mut [0u8; 4], |_| n += 1);
        assert_eq!(n, 0);
        assert_eq!((hdrlen(0x88, 0x01), hdrlen(0x08, 0x03), hdrlen(0x80, 0)), (26, 30, 24));
    }
}
