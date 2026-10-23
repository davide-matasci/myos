//! The MT7601U as its driver reaches it over USB: the registers behind the
//! vendor requests (a 64 KiB space, as READ_REGION_1 and WRITE_2 see it),
//! the configuration registers, the eFUSE ROM, the RF and baseband
//! windows (RF_CSR, BBP_CSR), the MCU taking the firmware in blocks and
//! then commands, and the bulk endpoints: frames out to the air on the
//! channel the RF is tuned to, and what is heard there back in, as the
//! MAC aggregates it. A request a real device would not take stops it
//! with the reason.

use std::collections::VecDeque;

use mtw::reg::*;

use super::air::{Air, Heard};

/// The station address in the model's ROM.
pub const MAC: [u8; 6] = [0x00, 0x0e, 0x8e, 0x76, 0x01, 0x42];
/// The interface's bulk endpoints, in its order (an MT7601U's).
pub const BULK: [u8; 8] = [0x84, 0x85, 0x08, 0x04, 0x05, 0x06, 0x07, 0x09];
const EP_RX: u8 = 0x84;
pub const EP_MCU: u8 = 0x08;
pub const EP_DATA: [u8; 5] = [0x04, 0x05, 0x06, 0x07, 0x09];
/// ASIC_VER and MAC_VER_ID.
const ASIC: u32 = 0x7601_0001;
const MAC_VER: u32 = 0x7601_0500;

/// A frame the driver sent.
pub struct Sent {
    pub channel: u8,
    pub endpoint: u8,
    /// The TXWI's PHY word, flags and station entry.
    pub phy: u16,
    pub xflags: u8,
    pub wcid: u8,
    /// An access point there took it (and acknowledged it).
    pub acked: bool,
    pub frame: Vec<u8>,
}

pub struct Device {
    mem: Vec<u8>,
    mcu_dma_addr: u32,
    mcu_dma_len: u32,
    rf: [[u8; 64]; 8],
    bbp: [u8; 256],
    rom: [u8; 512],
    /// The firmware the MCU must run: its vectors and code.
    ivb: Vec<u8>,
    ilm: Vec<u8>,
    mcu: Vec<u8>,
    pub running: bool,
    pub radio_on: bool,
    /// The MCU commands taken: {command, payload}.
    pub commands: Vec<(u8, Vec<u8>)>,
    /// The channel the RF was last tuned to (VCO calibrated after its
    /// registers).
    pub tuned: Option<u8>,
    pub sent: Vec<Sent>,
    air: Air,
    answers: VecDeque<Heard>,
}

impl Device {
    /// A device fresh from reset, whose MCU must be given firmware with
    /// vectors `ivb` and code `ilm`.
    pub fn new(ivb: &[u8], ilm: &[u8]) -> Self {
        let mut rom = [0xff; 512];
        let mut word = |w: usize, v: u16| rom[2 * w..2 * w + 2].copy_from_slice(&v.to_le_bytes());
        word(0x00, 0x7601);
        for i in 0..3 {
            word(0x02 + i, u16::from_le_bytes([MAC[2 * i], MAC[2 * i + 1]]));
        }
        word(0x1a, 0x0011);
        word(0x22, 0x0000);
        word(0x23, 0x0000);
        for i in 0..7 {
            word(0x29 + i, 0x1010);
        }
        for i in 0..10 {
            word(0x6f + i, 0x6666);
        }
        let mut bbp = [0; 256];
        bbp[0] = 0x01;
        let mut d = Device {
            mem: vec![0; 0x1_0000],
            mcu_dma_addr: 0,
            mcu_dma_len: 0,
            rf: [[0; 64]; 8],
            bbp,
            rom,
            ivb: ivb.to_vec(),
            ilm: ilm.to_vec(),
            mcu: vec![0; 0x2_0000],
            running: false,
            radio_on: false,
            commands: Vec::new(),
            tuned: None,
            sent: Vec::new(),
            air: Air::new(),
            answers: VecDeque::new(),
        };
        d.set(ASIC_VER, ASIC);
        d.set(MAC_VER_ID, MAC_VER);
        d
    }

    /// Another chip's ASIC_VER.
    pub fn set_asic(&mut self, ver: u32) {
        self.set(ASIC_VER, ver);
    }

    pub fn reg(&self, reg: u16) -> u32 {
        let r = usize::from(reg);
        u32::from_le_bytes(self.mem[r..r + 4].try_into().unwrap())
    }

    fn set(&mut self, reg: u16, val: u32) {
        let r = usize::from(reg);
        self.mem[r..r + 4].copy_from_slice(&val.to_le_bytes());
    }

    pub fn bytes(&self, reg: u16, len: usize) -> &[u8] {
        &self.mem[usize::from(reg)..usize::from(reg) + len]
    }

    pub fn bbp(&self, reg: u8) -> u8 {
        self.bbp[usize::from(reg)]
    }

    /// The MAC's own address, as MAC_ADDR_DW0/1 hold it.
    pub fn mac_addr(&self) -> [u8; 6] {
        let (lo, hi) = (self.reg(MAC_ADDR_DW0), self.reg(MAC_ADDR_DW1));
        let mut a = [0; 6];
        a[..4].copy_from_slice(&lo.to_le_bytes());
        a[4..].copy_from_slice(&hi.to_le_bytes()[..2]);
        a
    }

    pub fn bssid(&self) -> [u8; 6] {
        let (lo, hi) = (self.reg(MAC_BSSID_DW0), self.reg(MAC_BSSID_DW1));
        let mut a = [0; 6];
        a[..4].copy_from_slice(&lo.to_le_bytes());
        a[4..].copy_from_slice(&hi.to_le_bytes()[..2]);
        a
    }

    fn tx_on(&self) -> bool {
        self.radio_on
            && self.reg(MAC_SYS_CTRL) & MAC_TX_EN != 0
            && self.reg(WPDMA_GLO_CFG) & TX_DMA_EN != 0
            && self.reg(USB_DMA_CFG) & USB_TX_EN != 0
    }

    fn rx_on(&self) -> bool {
        self.radio_on
            && self.reg(MAC_SYS_CTRL) & MAC_RX_EN != 0
            && self.reg(WPDMA_GLO_CFG) & RX_DMA_EN != 0
            && self.reg(USB_DMA_CFG) & USB_RX_EN != 0
    }

    // ---- Control endpoint ------------------------------------------------

    pub fn vendor_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) {
        match request {
            REQ_WRITE_2 => {
                assert!(data.is_empty() && index & 1 == 0, "model: WRITE_2 of {index:#x} with data");
                let r = usize::from(index);
                self.mem[r..r + 2].copy_from_slice(&value.to_le_bytes());
                // A 32-bit register's side effects when its high half lands.
                if index & 2 != 0 {
                    self.written(index & !3);
                }
            }
            REQ_WRITE_REGION_1 => {
                let r = usize::from(index);
                self.mem[r..r + data.len()].copy_from_slice(data);
            }
            REQ_WRITE_CFG => {
                assert_eq!(data.len(), 4, "model: WRITE_CFG of {index:#x} not 4 bytes");
                let v = u32::from_le_bytes(data.try_into().unwrap());
                match index {
                    MCU_DMA_ADDR => self.mcu_dma_addr = v,
                    MCU_DMA_LEN => self.mcu_dma_len = v,
                    _ => panic!("model: WRITE_CFG of {index:#x}"),
                }
            }
            // The interrupt vectors: the MCU starts on the code loaded.
            REQ_RESET if value == 0x12 => {
                assert!(!self.running, "model: vectors written to a running MCU");
                assert_eq!(data, &self.ivb[..], "model: not the firmware's vectors");
                let at = 0x40;
                assert_eq!(&self.mcu[at..at + self.ilm.len()], &self.ilm[..], "model: the MCU would start on other code");
                self.running = true;
            }
            _ => panic!("model: vendor request {request:#x} out"),
        }
    }

    pub fn vendor_in(&mut self, request: u8, _value: u16, index: u16, data: &mut [u8]) {
        match request {
            REQ_READ_REGION_1 => {
                assert!(data.len() == 4 && index & 3 == 0, "model: READ_REGION_1 of {} at {index:#x}", data.len());
                data.copy_from_slice(self.bytes(index, 4));
                // The TX statistics clear when read.
                if matches!(index, TX_STA_CNT0 | TX_STA_CNT1 | TX_STA_CNT2) {
                    self.set(index, 0);
                }
            }
            REQ_READ_CFG if index == MCU_DMA_ADDR => {
                let v = if self.running { MCU_READY } else { self.mcu_dma_addr & !MCU_READY };
                data.copy_from_slice(&v.to_le_bytes());
            }
            _ => panic!("model: vendor request {request:#x} in of {index:#x}"),
        }
    }

    /// Register `reg` was written whole.
    fn written(&mut self, reg: u16) {
        let v = self.reg(reg);
        match reg {
            EFUSE_CTRL if v & EFSROM_KICK != 0 => {
                assert_eq!(v & EFSROM_MODE_MASK, 0, "model: eFUSE read in another mode");
                let block = ((v & EFSROM_AIN_MASK) >> EFSROM_AIN_SHIFT) as usize;
                assert_eq!(block & 0xf, 0, "model: eFUSE address not a block's");
                for n in 0..4 {
                    let w = u32::from_le_bytes(self.rom[block + 4 * n..block + 4 * n + 4].try_into().unwrap());
                    self.set(EFUSE_DATA0 + 4 * n as u16, w);
                }
                self.set(EFUSE_CTRL, v & !EFSROM_KICK & !EFUSE_AOUT_MASK);
            }
            RF_CSR if v & RF_CSR_KICK != 0 => {
                let (bank, r) = (((v >> BANK_SHIFT) & 0xf) as usize, ((v >> 8) & 0x3f) as usize);
                assert!(matches!(bank, 0 | 4 | 5), "model: RF bank {bank}");
                let mut out = v & !RF_CSR_KICK;
                if v & RF_CSR_WRITE != 0 {
                    self.rf[bank][r] = v as u8;
                    // The VCO calibrated: tuned to what banks 0's 17 to 20 say.
                    if (bank, r) == (0, 4) && v & 0x80 != 0 {
                        let regs = (self.rf[0][17], self.rf[0][18], self.rf[0][19], self.rf[0][20]);
                        self.tuned = RF_CHAN.iter().find(|c| (c.1, c.2, c.3, c.4) == regs).map(|c| c.0);
                    }
                } else {
                    out = (out & !0xff) | u32::from(self.rf[bank][r]);
                }
                self.set(RF_CSR, out);
            }
            BBP_CSR if v & BBP_CSR_KICK != 0 => {
                let r = ((v >> BBP_ADDR_SHIFT) & 0xff) as usize;
                let mut out = v & !BBP_CSR_KICK;
                if v & BBP_CSR_READ != 0 {
                    out = (out & !0xff) | u32::from(self.bbp[r]);
                } else {
                    let val = v as u8;
                    self.bbp[r] = val;
                    match (r, val) {
                        // A temperature reading done.
                        (22, 0x40) => self.bbp[47] &= !0x10,
                        // The RX DC calibration done.
                        (159, 0x4c) if self.bbp[158] == 0x8c => self.bbp[159] = 0x0c,
                        _ => {}
                    }
                }
                self.set(BBP_CSR, out);
            }
            _ => {}
        }
    }

    // ---- Bulk endpoints --------------------------------------------------

    pub fn bulk_out(&mut self, ep: u8, data: &[u8]) {
        assert!(ep == EP_MCU || EP_DATA.contains(&ep), "model: bulk OUT on {ep:#x}");
        assert!(data.len() >= TXD_LEN + DMA_PAD && data.len() % 4 == 0, "model: transfer of {} bytes", data.len());
        let len = usize::from(u16::from_le_bytes([data[0], data[1]]));
        let flags = u16::from_le_bytes([data[2], data[3]]);
        assert_eq!(data.len(), TXD_LEN + len + DMA_PAD, "model: TXD length {len} of a {}-byte transfer", data.len());
        assert!(data[TXD_LEN + len..].iter().all(|&b| b == 0), "model: no zeros after the descriptor");
        let body = &data[TXD_LEN..TXD_LEN + len];
        if flags & TXD_MCU != 0 {
            assert_eq!(ep, EP_MCU, "model: MCU transfer on {ep:#x}");
            if flags & TXD_CMD != 0 {
                assert!(self.running, "model: MCU command before its firmware runs");
                let cmd = ((flags >> TXD_CMD_SHIFT) & 0x7f) as u8;
                if cmd == MCU_CMD_RADIO {
                    let func = u32::from_le_bytes(body[..4].try_into().unwrap());
                    self.radio_on = func == 0x31;
                }
                self.commands.push((cmd, body.to_vec()));
            } else {
                self.firmware_block(body);
            }
            return;
        }
        self.frame(ep, flags, body);
    }

    /// A block of firmware, where MCU_DMA_ADDR says, of MCU_DMA_LEN.
    fn firmware_block(&mut self, block: &[u8]) {
        assert!(!self.running, "model: firmware written to a running MCU");
        assert!(
            self.reg(FCE_PSE_CTRL) == 1
                && self.reg(FCE_PDMA) == 0x44
                && self.reg(FCE_SKIP_FS) == 3
                && self.reg(TX_CPU_FCE_BASE) == 0x0040_0230
                && self.reg(USB_DMA_CFG) & USB_TX_EN != 0,
            "model: firmware sent before the MCU's DMA was set up"
        );
        assert!(block.len() <= 0x2000, "model: firmware block of {} bytes", block.len());
        assert_eq!(self.mcu_dma_len >> 16, block.len() as u32, "model: MCU_DMA_LEN not the block's");
        let at = self.mcu_dma_addr as usize;
        self.mcu[at..at + block.len()].copy_from_slice(block);
    }

    /// A frame after its TXWI, to the air.
    fn frame(&mut self, ep: u8, flags: u16, body: &[u8]) {
        assert!(self.tx_on(), "model: frame sent with transmit off");
        assert_eq!(flags & (TXD_80211 | TXD_QSEL_EDCA), TXD_80211 | TXD_QSEL_EDCA, "model: TXD flags {flags:#x}");
        let txwi = &body[..TXWI_LEN];
        let len = usize::from(u16::from_le_bytes([txwi[6], txwi[7]]));
        assert_eq!(body.len(), (TXWI_LEN + len + 3) & !3, "model: TXWI length {len} in a {}-byte body", body.len());
        let frame = body[TXWI_LEN..TXWI_LEN + len].to_vec();
        let (phy, xflags, wcid) = (u16::from_le_bytes([txwi[2], txwi[3]]), txwi[4], txwi[5]);
        let mgmt = frame[0] & 0x0c == 0;
        assert_eq!(mgmt, ep == EP_MCU, "model: a {} frame on {ep:#x}", if mgmt { "management" } else { "data" });
        let ra: [u8; 6] = frame[4..10].try_into().unwrap();
        if !mgmt && ra[0] & 1 == 0 {
            // Unicast data: to the entry of the access point it goes to.
            assert_eq!(self.bytes(WCID_ENTRY0 + 8 * u16::from(wcid), 6), &ra, "model: station entry {wcid} is not {ra:02x?}");
        }
        assert_eq!(xflags & TX_ACK != 0, ra[0] & 1 == 0, "model: ACK wanted of a group frame, or not of another");
        let channel = self.tuned.expect("model: frame sent before the RF was tuned");
        let kbps = rate_kbps(phy);
        // A frame faster than the access point's signal carries is lost:
        // the MAC retries it 7 times and counts a failure.
        let heard = self.air.rssi(channel, ra).is_some_and(|rssi| kbps <= max_kbps(rssi));
        let acked = xflags & TX_ACK != 0 && heard;
        if xflags & TX_ACK != 0 && self.air.has_ap(channel, ra) {
            let (cnt0, cnt1) = (self.reg(TX_STA_CNT0), self.reg(TX_STA_CNT1));
            if acked {
                self.set(TX_STA_CNT1, cnt1 + 1);
            } else {
                self.set(TX_STA_CNT0, cnt0 + 1);
                self.set(TX_STA_CNT1, cnt1 + (7 << 16));
            }
        }
        if heard || ra[0] & 1 != 0 {
            self.answers.extend(self.air.hear(channel, &frame));
        }
        self.sent.push(Sent { channel, endpoint: ep, phy, xflags, wcid, acked, frame });
    }

    /// The TX statistics as the MAC counts them: {failures, successes,
    /// retries}.
    pub fn tx_counts(&self) -> (u32, u32, u32) {
        let cnt1 = self.reg(TX_STA_CNT1);
        (self.reg(TX_STA_CNT0) & 0xffff, cnt1 & 0xffff, cnt1 >> 16)
    }

    /// One bulk IN transfer on the frames endpoint: what was heard on the
    /// channel tuned to since (each access point's beacon, the answers),
    /// past the receive filter, aggregated up to a page.
    pub fn bulk_in(&mut self, ep: u8) -> Vec<u8> {
        assert_eq!(ep, EP_RX, "model: frames read from {ep:#x}");
        let mut out = Vec::new();
        if !self.rx_on() {
            return out;
        }
        let Some(channel) = self.tuned else {
            return out;
        };
        let me = self.mac_addr();
        let filter = self.reg(RX_FILTR_CFG);
        let mut heard: Vec<Heard> = self.answers.drain(..).collect();
        heard.extend(self.air.beacons());
        for h in heard.into_iter().filter(|h| h.channel == channel) {
            let to: [u8; 6] = h.frame[4..10].try_into().unwrap();
            if filter & DROP_UC_NOME != 0 && to[0] & 1 == 0 && to != me {
                continue;
            }
            let mut body = vec![0u8; RXWI_LEN];
            body[8..10].copy_from_slice(&(h.frame.len() as u16).to_le_bytes());
            // The signal in the MAC's units, for a ROM without offsets.
            body[12] = (-12 - i32::from(h.rssi)) as u8;
            body.extend_from_slice(&h.frame);
            body.resize((body.len() + 3) & !3, 0);
            if out.len() + body.len() + 8 > MAX_RXSZ {
                break;
            }
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&body);
            out.extend_from_slice(&[0; 4]);
        }
        out
    }
}

/// The rate of TXWI PHY word `phy` in kb/s (one stream, 20 MHz, a long
/// guard interval); a mode or code the MT7601U has not stops the model.
fn rate_kbps(phy: u16) -> u32 {
    let (mode, mcs) = (phy >> PHY_SHIFT, usize::from(phy & 0x3f & !PHY_SHPRE));
    let table: &[u32] = match mode {
        PHY_CCK => &[1000, 2000, 5500, 11_000],
        PHY_OFDM => &[6000, 9000, 12_000, 18_000, 24_000, 36_000, 48_000, 54_000],
        PHY_HT => &[6500, 13_000, 19_500, 26_000, 39_000, 52_000, 58_500, 65_000],
        _ => panic!("model: PHY mode {mode}"),
    };
    *table.get(mcs).unwrap_or_else(|| panic!("model: MCS {mcs} in PHY mode {mode}"))
}

/// The fastest rate a signal of `rssi` dBm carries, roughly.
fn max_kbps(rssi: i8) -> u32 {
    match rssi {
        -50.. => 65_000,
        -60.. => 36_000,
        -70.. => 18_000,
        _ => 9000,
    }
}
