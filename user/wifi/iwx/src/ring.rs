//! What the driver and the firmware share in host memory, as bytes: the
//! context information that starts the firmware, the command queue's
//! descriptors and commands, the packets of a receive buffer (from OpenBSD's
//! if_iwx.c: iwx_ctxt_info_init, iwx_send_cmd, iwx_rx_pkt; the structures of
//! if_iwxreg.h; notices in LICENSE).

/// struct iwx_context_info.
pub const CTXT_INFO_LEN: usize = 1792;
/// The DRAM entries the context information has room for, per image.
pub const MAX_DRAM_ENTRY: usize = 64;
/// The receive ring: buffers, each 4 KB.
pub const RX_RING: usize = 512;
pub const RX_BUF: usize = 4096;
/// The command queue: descriptors (TFDs) and their commands.
pub const CMD_RING: usize = 256;
pub const TFD_LEN: usize = 256;
/// A command's slot: the wide header and up to 316 bytes; a bigger one
/// goes in a buffer of its own.
pub const CMD_SLOT: usize = 324;
pub const CMD_HDR: usize = 8;
pub const CMD_SLOT_DATA: usize = CMD_SLOT - CMD_HDR;
pub const CMD_MAX_PAYLOAD: usize = 4096 - CMD_HDR;
/// The device reads a command's first 20 bytes from its first transfer
/// buffer.
const FIRST_TB: usize = 20;
/// Packets in a receive buffer start on 64-byte boundaries.
const FRAME_ALIGN: usize = 0x40;
const FRAME_INVALID: u32 = 0x5555_0000;
const FRAME_SIZE_MASK: u32 = 0x3fff;
/// struct iwx_rb_status: the receive ring's closed buffer count first.
pub const RX_STATUS_LEN: usize = 12;

pub const LEGACY_GROUP: u8 = 0;
pub const LONG_GROUP: u8 = 1;

/// Where the firmware finds what the driver set up for it.
pub struct CtxtAddrs<'a> {
    pub free_rbd: u64,
    pub used_rbd: u64,
    pub rx_status: u64,
    pub cmd_queue: u64,
    pub lmac: &'a [u64],
    pub umac: &'a [u64],
    pub paging: &'a [u64],
}

/// The context information (struct iwx_context_info): `hw_rev` is the raw
/// HW_REV register (its low 16 bits are the MAC id).
pub fn context_info(out: &mut [u8; CTXT_INFO_LEN], hw_rev: u32, a: &CtxtAddrs) -> Result<(), ()> {
    if a.lmac.len() > MAX_DRAM_ENTRY || a.umac.len() > MAX_DRAM_ENTRY || a.paging.len() > MAX_DRAM_ENTRY {
        return Err(());
    }
    out.fill(0);
    put16(out, 0, hw_rev as u16);
    put16(out, 2, 0);
    put16(out, 4, (CTXT_INFO_LEN / 4) as u16);
    // Long TFDs, 2^9 receive buffers of 4 KB.
    let flags: u32 = (1 << 8) | (RX_RING.trailing_zeros() << 4) | (0x4 << 9);
    put32(out, 8, flags);
    put64(out, 24, a.free_rbd);
    put64(out, 32, a.used_rbd);
    put64(out, 40, a.rx_status);
    put64(out, 48, a.cmd_queue);
    // The command queue's size: log2(256) - 3.
    out[56] = (CMD_RING.trailing_zeros() - 3) as u8;
    // The DRAM map: UMAC images, then LMAC, then the paging ("virtual")
    // ones, 64 entries each.
    const DRAM: usize = 192;
    for (i, p) in a.umac.iter().enumerate() {
        put64(out, DRAM + 8 * i, *p);
    }
    for (i, p) in a.lmac.iter().enumerate() {
        put64(out, DRAM + 8 * (MAX_DRAM_ENTRY + i), *p);
    }
    for (i, p) in a.paging.iter().enumerate() {
        put64(out, DRAM + 8 * (2 * MAX_DRAM_ENTRY + i), *p);
    }
    Ok(())
}

/// A command's id: opcode, group and version (iwx_cmd_id).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CmdId {
    pub opcode: u8,
    pub group: u8,
    pub version: u8,
}

impl CmdId {
    pub const fn legacy(opcode: u8) -> Self {
        CmdId { opcode, group: LEGACY_GROUP, version: 0 }
    }

    pub const fn wide(group: u8, opcode: u8) -> Self {
        CmdId { opcode, group, version: 0 }
    }

    /// As sent: firmware API 50 and later refuses group 0, so a legacy
    /// command goes as the long group (the answer comes back under it).
    pub fn sent(self) -> CmdId {
        if self.group == LEGACY_GROUP { CmdId { group: LONG_GROUP, ..self } } else { self }
    }
}

/// The command at `idx` of queue `qid` with `payload`: its wide header and
/// payload into `cmd` (its slot, or a bigger buffer), the length written.
pub fn encode_cmd(cmd: &mut [u8], id: CmdId, qid: u8, idx: u8, payload: &[u8]) -> Option<usize> {
    let len = CMD_HDR + payload.len();
    if payload.len() > CMD_MAX_PAYLOAD || cmd.len() < len {
        return None;
    }
    let id = id.sent();
    cmd[0] = id.opcode;
    cmd[1] = id.group;
    cmd[2] = idx;
    cmd[3] = qid;
    put16(cmd, 4, payload.len() as u16);
    cmd[6] = 0;
    cmd[7] = id.version;
    cmd[CMD_HDR..len].copy_from_slice(payload);
    Some(len)
}

/// The descriptor of a command of `len` bytes at `phys`: the first 20 bytes
/// in one transfer buffer, the rest in a second.
pub fn encode_tfd(tfd: &mut [u8; TFD_LEN], phys: u64, len: usize) {
    tfd.fill(0);
    // struct iwx_tfh_tfd: u16 num_tbs, then packed {u16 len, u64 addr}.
    put16(&mut tfd[..], 2, len.min(FIRST_TB) as u16);
    put64(&mut tfd[..], 4, phys);
    if len > FIRST_TB {
        put16(&mut tfd[..], 12, (len - FIRST_TB) as u16);
        put64(&mut tfd[..], 14, phys + FIRST_TB as u64);
        put16(&mut tfd[..], 0, 2);
    } else {
        put16(&mut tfd[..], 0, 1);
    }
}

/// A free receive buffer descriptor: the buffer's address (4 KB aligned)
/// with its index in the low 12 bits.
pub fn rbd(phys: u64, idx: usize) -> u64 {
    phys | (idx as u64 & 0xfff)
}

/// A packet the firmware put in a receive buffer.
#[derive(Clone, Copy, Debug)]
pub struct Packet<'a> {
    /// Its command or notification, wide (group << 8 | code).
    pub code: u16,
    pub flags: u8,
    pub idx: u8,
    /// The command queue it answers, 0x80 set when the firmware sent it
    /// unasked (a notification).
    pub qid: u8,
    /// What follows the header.
    pub data: &'a [u8],
}

impl Packet<'_> {
    pub fn is_notification(&self) -> bool {
        self.qid & 0x80 != 0
    }

    pub fn group(&self) -> u8 {
        (self.code >> 8) as u8
    }

    pub fn opcode(&self) -> u8 {
        self.code as u8
    }
}

/// The packets of a receive buffer, each on a 64-byte boundary, until an
/// empty or invalid one (iwx_rx_pkt).
pub fn packets(buf: &[u8]) -> Packets<'_> {
    Packets { buf, off: 0 }
}

pub struct Packets<'a> {
    buf: &'a [u8],
    off: usize,
}

impl<'a> Iterator for Packets<'a> {
    type Item = Packet<'a>;

    fn next(&mut self) -> Option<Packet<'a>> {
        // len_n_flags, then the header {code, flags (group), idx, qid}.
        const MIN: usize = 8;
        let p = self.buf.get(self.off..)?;
        if p.len() <= MIN {
            return None;
        }
        let len_n_flags = le32(p, 0);
        let (code, flags, idx, qid) = (p[4], p[5], p[6], p[7]);
        if (qid & !0x80 == 0 && idx == 0 && code == 0 && flags == 0) || len_n_flags == FRAME_INVALID {
            return None;
        }
        // The length counts from the header.
        let len = 4 + (len_n_flags & FRAME_SIZE_MASK) as usize;
        if len < MIN || len > p.len() {
            return None;
        }
        self.off += len.next_multiple_of(FRAME_ALIGN);
        Some(Packet { code: (flags as u16) << 8 | code as u16, flags, idx, qid, data: &p[MIN..len] })
    }
}

pub(crate) fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

pub(crate) fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

pub(crate) fn put64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

pub(crate) fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

pub(crate) fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

#[cfg(test)]
pub(crate) fn le64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_info_layout() {
        let mut c = [0xaau8; CTXT_INFO_LEN];
        let a = CtxtAddrs {
            free_rbd: 0x1000,
            used_rbd: 0x2000,
            rx_status: 0x3000,
            cmd_queue: 0x4000,
            lmac: &[0x10_0000, 0x10_1000],
            umac: &[0x20_0000],
            paging: &[0x30_0000],
        };
        context_info(&mut c, 0x0000_0354, &a).unwrap();
        assert_eq!(le16(&c, 0), 0x354);
        assert_eq!(le16(&c, 4), 448);
        assert_eq!(le32(&c, 8), 0x990);
        assert_eq!((le64(&c, 24), le64(&c, 32), le64(&c, 40)), (0x1000, 0x2000, 0x3000));
        assert_eq!(le64(&c, 48), 0x4000);
        assert_eq!(c[56], 5);
        assert_eq!(le64(&c, 192), 0x20_0000);
        assert_eq!(le64(&c, 192 + 512), 0x10_0000);
        assert_eq!(le64(&c, 192 + 512 + 8), 0x10_1000);
        assert_eq!(le64(&c, 192 + 1024), 0x30_0000);
        assert!(c[1728..].iter().all(|&b| b == 0));
        let too_many = [0u64; 65];
        assert!(context_info(&mut c, 0, &CtxtAddrs { lmac: &too_many, ..a }).is_err());
    }

    #[test]
    fn command_and_descriptor() {
        let mut slot = [0u8; CMD_SLOT];
        // A legacy command goes as the long group.
        let n = encode_cmd(&mut slot, CmdId::legacy(0x98), 0, 7, &[1, 2, 3, 4]).unwrap();
        assert_eq!(n, 12);
        assert_eq!(&slot[..12], &[0x98, 1, 7, 0, 4, 0, 0, 0, 1, 2, 3, 4]);
        let mut big = [0u8; 4096];
        let n = encode_cmd(&mut big, CmdId::wide(1, 0x0d), 0, 255, &[0; 1940]).unwrap();
        assert_eq!((n, big[1], le16(&big, 4)), (1948, 1, 1940));
        assert!(encode_cmd(&mut slot, CmdId::legacy(1), 0, 0, &[0; CMD_SLOT_DATA + 1]).is_none());

        let mut tfd = [0xffu8; TFD_LEN];
        encode_tfd(&mut tfd, 0x1234_5000, 12);
        assert_eq!((le16(&tfd, 0), le16(&tfd, 2), le64(&tfd, 4)), (1, 12, 0x1234_5000));
        assert!(tfd[12..].iter().all(|&b| b == 0));
        encode_tfd(&mut tfd, 0x1234_5000, 1948);
        assert_eq!((le16(&tfd, 0), le16(&tfd, 2)), (2, 20));
        assert_eq!((le16(&tfd, 12), le64(&tfd, 14)), (1928, 0x1234_5014));
        assert_eq!(rbd(0xabcd_e000, 513), 0xabcd_e201);
    }

    #[test]
    fn packets_of_a_buffer() {
        let mut buf = [0u8; RX_BUF];
        // An ALIVE notification (code 1, legacy, qid 0x80) of 4 data bytes,
        // then a command response at the next 64-byte boundary.
        put32(&mut buf, 0, 4 + 4);
        buf[4..8].copy_from_slice(&[0x01, 0, 0, 0x80]);
        buf[8..12].copy_from_slice(&[0xfe, 0xca, 0, 0]);
        put32(&mut buf, 64, 4 + 2);
        buf[68..72].copy_from_slice(&[0x0d, 1, 9, 0]);
        buf[72..74].copy_from_slice(&[0xaa, 0xbb]);
        let p: [Packet; 2] = {
            let mut it = packets(&buf);
            [it.next().unwrap(), it.next().unwrap()]
        };
        assert_eq!((p[0].code, p[0].is_notification(), p[0].data), (0x0001, true, &[0xfe, 0xca, 0, 0][..]));
        assert_eq!((p[1].code, p[1].idx, p[1].group(), p[1].data), (0x010d, 9, 1, &[0xaa, 0xbb][..]));
        assert_eq!(packets(&buf).count(), 2);
        // The invalid marker ends the buffer, as does a length past it.
        put32(&mut buf, 0, FRAME_INVALID);
        assert_eq!(packets(&buf).count(), 0);
        put32(&mut buf, 0, 5000);
        assert_eq!(packets(&buf).count(), 0);
    }
}
