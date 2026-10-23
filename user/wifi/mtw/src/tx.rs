//! A frame to send as the bulk OUT endpoints take it (mtw_tx; notices in
//! LICENSE): a DMA descriptor (TXD), the transmit information block
//! (TXWI: rate, ACK, length), the frame, zeros to a 4-byte boundary and 4
//! more.

use alloc::vec::Vec;

use crate::reg::*;

/// Where and how frame `frame` goes: the endpoint (of `Endpoints`: the
/// MCU's for a management frame, else the data one of its access
/// category), and the transfer. A data frame to the access point goes at
/// rate `ridx` as the station's table entry `wcid`, the rest at 1 Mb/s
/// (a management or broadcast frame, which every station hears); a frame
/// that wants an ACK gets the ACK's duration. The frame carries its own
/// IV, if any (`TXD_WIV`): the daemon encrypts.
pub fn transfer(frame: &[u8], ridx: usize, wcid: u8) -> (Queue, Vec<u8>) {
    let (fc0, fc1) = (frame[0], frame[1]);
    let data = fc0 & 0x0c == 0x08;
    let mgmt = fc0 & 0x0c == 0x00;
    let qos = data && fc0 & 0x80 != 0;
    let qos_ctl = if qos { frame.get(crate::rx::hdrlen(fc0, fc1) - 2).copied().unwrap_or(0) } else { 0 };
    let queue = if mgmt { Queue::Mcu } else { Queue::Data(ac(qos_ctl & 7)) };
    let multicast = frame[4] & 1 != 0;
    let ridx = if multicast || !data { RIDX_CCK1 } else { ridx };
    let rate = &RATES[ridx];

    let body = TXWI_LEN + frame.len();
    let padded = (body + 3) & !3;
    let mut out = Vec::with_capacity(TXD_LEN + padded + DMA_PAD);
    out.extend_from_slice(&(padded as u16).to_le_bytes());
    out.extend_from_slice(&(TXD_DATA | TXD_80211 | TXD_WLAN | TXD_QSEL_EDCA | TXD_WIV).to_le_bytes());

    let phy = (if rate.ofdm { PHY_OFDM } else { PHY_CCK }) << PHY_SHIFT | u16::from(rate.mcs);
    // No ACK for a group frame or a QoS one that says so.
    let ack = !multicast && (qos_ctl >> 5) & 3 != 1;
    let mut xflags = if qos { 0 } else { TX_NSEQ };
    if ack {
        xflags |= TX_ACK;
    }
    out.extend_from_slice(&[0, TX_TXOP_BACKOFF]);
    out.extend_from_slice(&phy.to_le_bytes());
    out.extend_from_slice(&[xflags, if data { wcid } else { 0xff }]);
    out.extend_from_slice(&(frame.len() as u16).to_le_bytes());
    out.extend_from_slice(&[0; 12]);

    let at = out.len();
    out.extend_from_slice(frame);
    if ack {
        let dur = RATES[rate.ctl_ridx].lp_ack_dur;
        out[at + 2..at + 4].copy_from_slice(&dur.to_le_bytes());
    }
    out.resize(TXD_LEN + padded + DMA_PAD, 0);
    (queue, out)
}

/// The bulk OUT endpoint a frame goes to.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Queue {
    Mcu,
    /// The data endpoint of an access category (0 best effort, 1
    /// background, 2 video, 3 voice: EDCA_AC_*).
    Data(usize),
}

/// The access category of user priority `up` (ieee80211_up_to_ac).
fn ac(up: u8) -> usize {
    match up {
        1 | 2 => 1,
        4 | 5 => 2,
        6 | 7 => 3,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(fc0: u8, da: u8) -> Vec<u8> {
        let mut f = alloc::vec![fc0, 0, 0, 0, da, 2, 3, 4, 5, 6];
        f.extend_from_slice(&[0x10; 14]);
        f
    }

    #[test]
    fn data_to_the_ap() {
        let mut frame = header(0x08, 0x02);
        frame.extend_from_slice(b"payload");
        let (q, x) = transfer(&frame, RIDX_OFDM24, aid_to_wcid(1));
        assert_eq!(q, Queue::Data(0));
        // TXD: the TXWI and frame's length, rounded up; WIV.
        let body = TXWI_LEN + frame.len();
        assert_eq!(u16::from_le_bytes([x[0], x[1]]) as usize, (body + 3) & !3);
        assert_eq!(x.len(), TXD_LEN + ((body + 3) & !3) + DMA_PAD);
        assert_ne!(u16::from_le_bytes([x[2], x[3]]) & TXD_WIV, 0);
        // TXWI: OFDM, 24 Mb/s's code, ACK, no sequence of its own, entry 2.
        let w = &x[TXD_LEN..];
        assert_eq!(u16::from_le_bytes([w[2], w[3]]), PHY_OFDM << PHY_SHIFT | 2);
        assert_eq!((w[4], w[5]), (TX_NSEQ | TX_ACK, 2));
        assert_eq!(u16::from_le_bytes([w[6], w[7]]) as usize, frame.len());
        // The frame, with the ACK's duration at 24 Mb/s (its control rate's).
        let f = &x[TXD_LEN + TXWI_LEN..];
        assert_eq!(u16::from_le_bytes([f[2], f[3]]), 48);
        assert_eq!(&f[4..frame.len()], &frame[4..]);
    }

    #[test]
    fn management_and_broadcast() {
        let (q, x) = transfer(&header(0x40, 0xff), RIDX_OFDM24, 1);
        assert_eq!(q, Queue::Mcu);
        let w = &x[TXD_LEN..];
        // 1 Mb/s CCK, no ACK, no station entry.
        assert_eq!(u16::from_le_bytes([w[2], w[3]]), 0);
        assert_eq!((w[4], w[5]), (TX_NSEQ, 0xff));
        // A QoS data frame of priority 6 goes to the voice endpoint.
        let mut qos = header(0x88, 0x02);
        qos.extend_from_slice(&[6, 0]);
        assert_eq!(transfer(&qos, RIDX_OFDM24, 1).0, Queue::Data(3));
    }
}
