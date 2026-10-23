//! The BCDC protocol: the firmware's control messages (a command or a
//! variable, set or queried, answered with a status) and the header on
//! every data frame, as the SDIO bus carries them.

use alloc::vec::Vec;

use crate::reg::*;

/// A control message's header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dcmd {
    pub cmd: u32,
    pub len: u32,
    pub flags: u32,
    pub status: u32,
}

impl Dcmd {
    pub fn reqid(&self) -> u16 {
        (self.flags >> BCDC_DCMD_ID_SHIFT) as u16
    }
    pub fn error(&self) -> bool {
        self.flags & BCDC_DCMD_ERROR != 0
    }
}

/// A control message: command `cmd` with `data`, a `set` or a query, as
/// request `reqid` on interface `ifidx`.
pub fn encode(cmd: u32, set: bool, reqid: u16, ifidx: u8, data: &[u8]) -> Vec<u8> {
    let flags = if set { BCDC_DCMD_SET } else { 0 } | u32::from(reqid) << BCDC_DCMD_ID_SHIFT | u32::from(ifidx & 0xf) << BCDC_DCMD_IF_SHIFT;
    let mut out = Vec::with_capacity(BCDC_DCMD_HDR_LEN + data.len());
    out.extend_from_slice(&cmd.to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(data);
    out
}

/// A control message as received: its header and the data after it.
pub fn decode(buf: &[u8]) -> Option<(Dcmd, &[u8])> {
    if buf.len() < BCDC_DCMD_HDR_LEN {
        return None;
    }
    let w = |i: usize| u32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
    Some((Dcmd { cmd: w(0), len: w(4), flags: w(8), status: w(12) }, &buf[BCDC_DCMD_HDR_LEN..]))
}

/// A variable's buffer: its name, a NUL, its value.
pub fn var(name: &str, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(name.len() + 1 + data.len());
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    out.extend_from_slice(data);
    out
}

/// A data frame's header: protocol version 2, the frame's `priority`, no
/// extra offset.
pub fn data_header(priority: u8) -> [u8; BCDC_HDR_LEN] {
    [BCDC_FLAG_PROTO_VER << 4, priority & 7, 0, 0]
}

/// The payload of a received frame, past its header and the data offset
/// the header gives (in words).
pub fn payload(frame: &[u8]) -> Option<&[u8]> {
    if frame.len() < BCDC_HDR_LEN {
        return None;
    }
    let off = BCDC_HDR_LEN + usize::from(frame[3]) * 4;
    frame.get(off..)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages() {
        let m = encode(C_GET_VAR, false, 0x1234, 0, b"cur_etheraddr\0\0\0\0\0\0\0");
        assert_eq!(&m[..16], &[0x06, 0x01, 0, 0, 20, 0, 0, 0, 0, 0, 0x34, 0x12, 0, 0, 0, 0]);
        let (d, data) = decode(&m).unwrap();
        assert_eq!((d.cmd, d.len, d.reqid(), d.error()), (262, 20, 0x1234, false));
        assert_eq!(data.len(), 20);
        let m = encode(C_SET_VAR, true, 1, 2, &[]);
        let (d, _) = decode(&m).unwrap();
        assert_eq!(d.flags, BCDC_DCMD_SET | 1 << 16 | 2 << 12);
        assert_eq!(var("mpc", &[1, 0, 0, 0]), b"mpc\0\x01\0\0\0");
        assert_eq!(data_header(5), [0x20, 5, 0, 0]);
        assert_eq!(payload(&[0x20, 0, 0, 1, 9, 9, 9, 9, 7, 8]).unwrap(), &[7, 8]);
        assert_eq!(payload(&[0x20, 0]), None);
    }
}
