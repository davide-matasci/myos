//! The MT7612U as its driver (`mtw::mt76x2`) reaches it over USB: the MAC's
//! registers behind the multi requests, the USB configuration space, the
//! EEPROM, the FCE taking the ROM patch and the firmware in blocks, the
//! MCU applying the patch, starting on its vectors and then taking
//! commands, answering those that want it on the second bulk IN endpoint,
//! the channel it tunes to, and the frames: out to the air on that
//! channel, and what is heard there back in, a frame a transfer, at two
//! chains' signal. Every request and descriptor is checked against the
//! layout Linux's mt76x02 headers describe, and what a real device would
//! not take stops it with the reason. myos's own code, not taken from
//! Linux or MediaTek.

use std::collections::{HashMap, VecDeque};

use mtw::mt76x2::reg::*;

use super::air::{Air, Heard};

/// The station address in the model's EEPROM.
pub const MAC: [u8; 6] = [0x00, 0x0e, 0x8e, 0x76, 0x12, 0x42];
/// The interface's bulk endpoints, in its order (an MT7612U's: IN 0x84
/// frames, 0x85 answers; OUT 0x08 commands, then BE, BK, VI, VO, HCCA).
pub const BULK: [u8; 8] = [0x84, 0x85, 0x08, 0x04, 0x05, 0x06, 0x07, 0x09];
const EP_RX: u8 = 0x84;
const EP_RESP: u8 = 0x85;
pub const EP_MCU: u8 = 0x08;
pub const EP_DATA: [u8; 5] = [0x04, 0x05, 0x06, 0x07, 0x09];
/// ASIC_VERSION: an MT7612U of revision E4.
pub const ASIC: u32 = 0x7612_0044;
/// The LNA gain the EEPROM gives (+4 dB, sign and magnitude), which the
/// RXWI's signal carries.
const LNA: i32 = 4;
/// AGC 8 and 9's gain after reset.
pub const AGC_GAIN_RESET: u32 = 0x50;

/// A frame the driver sent.
pub struct Sent {
    pub channel: u8,
    pub endpoint: u8,
    /// The TXWI's rate word, ACK flags, station entry and streams.
    pub rate: u16,
    pub ack_ctl: u8,
    pub wcid: u8,
    pub txstream: u8,
    /// An access point there took it (and acknowledged it).
    pub acked: bool,
    pub frame: Vec<u8>,
}

pub struct Device {
    mem: Vec<u8>,
    cfg: HashMap<u16, u32>,
    pub ee: [u8; EEPROM_SIZE],
    fce: HashMap<u16, u32>,
    mcu: Vec<u8>,
    /// The ROM patch and the firmware the MCU must run.
    patch: Vec<u8>,
    ilm: Vec<u8>,
    dlm: Vec<u8>,
    patch_enabled: bool,
    pub patched: bool,
    pub running: bool,
    pub radio_on: bool,
    /// Blocks of code taken, for the descriptor index the driver moves.
    blocks: u32,
    /// The MCU commands taken: {command, payload}.
    pub commands: Vec<(u8, Vec<u8>)>,
    answers: VecDeque<Vec<u8>>,
    /// The channel the MCU last tuned to, and whether for a scan.
    pub tuned: Option<(u8, bool)>,
    pub sent: Vec<Sent>,
    air: Air,
    heard: VecDeque<Heard>,
}

impl Device {
    /// A device fresh from reset, whose MCU must be given ROM patch `patch`
    /// and firmware parts `ilm` and `dlm`.
    pub fn new(patch: &[u8], ilm: &[u8], dlm: &[u8]) -> Self {
        let mut ee = [0xff; EEPROM_SIZE];
        word(&mut ee, EE_CHIP_ID, 0x7612);
        for i in 0..3 {
            word(&mut ee, EE_MAC_ADDR + 2 * i, u16::from_le_bytes([MAC[2 * i], MAC[2 * i + 1]]));
        }
        // Both bands, internal PAs, two chains; TSSI on; external LNAs off.
        word(&mut ee, EE_NIC_CONF_0, NIC_CONF_0_PA_INT_2G | NIC_CONF_0_PA_INT_5G | 0x22);
        word(&mut ee, EE_NIC_CONF_1, NIC_CONF_1_TX_ALC_EN);
        word(&mut ee, EE_NIC_CONF_2, 0);
        // An LNA gain of +4 dB on both bands; no signal offsets.
        word(&mut ee, EE_LNA_GAIN, 0x8484);
        // The chains' power on 2.4 GHz: slope, offset, a target of 16 and
        // 14 dBm (in 0.5 dB), no deltas; the same on 5 GHz's groups.
        ee[EE_TX_POWER_0_START_2G..EE_TX_POWER_0_START_2G + 6].copy_from_slice(&[0xc8, 0x10, 0x20, 0, 0, 0]);
        ee[EE_TX_POWER_1_START_2G..EE_TX_POWER_1_START_2G + 6].copy_from_slice(&[0xc9, 0x11, 0x1c, 0, 0, 0]);
        for g in 0..6 {
            let at = TX_POWER_GROUP_SIZE_5G * g;
            ee[EE_TX_POWER_0_START_5G + at..EE_TX_POWER_0_START_5G + at + 5].copy_from_slice(&[0xc8, 0x10, 0x18, 0, 0]);
            ee[EE_TX_POWER_1_START_5G + at..EE_TX_POWER_1_START_5G + at + 5].copy_from_slice(&[0xc9, 0x11, 0x16, 0, 0]);
        }
        // No per-rate offsets; the R calibration's result is there.
        for at in (EE_TX_POWER_CCK..=EE_TX_POWER_VHT_MCS8).step_by(2) {
            word(&mut ee, at, 0);
        }
        word(&mut ee, EE_BT_RCAL_RESULT, 0x0010);
        let mut d = Device {
            mem: vec![0; 0x1_0000],
            cfg: HashMap::new(),
            ee,
            fce: HashMap::new(),
            mcu: vec![0; 0x12_0000],
            patch: patch.to_vec(),
            ilm: ilm.to_vec(),
            dlm: dlm.to_vec(),
            patch_enabled: false,
            patched: false,
            running: false,
            radio_on: false,
            blocks: 0,
            commands: Vec::new(),
            answers: VecDeque::new(),
            tuned: None,
            sent: Vec::new(),
            air: Air::dual_band(),
            heard: VecDeque::new(),
        };
        d.set(ASIC_VERSION, ASIC);
        d.set(MAC_CSR0, ASIC);
        for n in [8, 9] {
            d.set(bbp(BBP_AGC, n), 0x1836_00f8 | AGC_GAIN_RESET << 8);
        }
        d
    }

    /// Another chip's ASIC_VERSION.
    pub fn set_asic(&mut self, ver: u32) {
        self.set(ASIC_VERSION, ver);
    }

    pub fn reg(&self, reg: u32) -> u32 {
        let r = reg as usize;
        u32::from_le_bytes(self.mem[r..r + 4].try_into().unwrap())
    }

    fn set(&mut self, reg: u32, val: u32) {
        let r = reg as usize;
        self.mem[r..r + 4].copy_from_slice(&val.to_le_bytes());
    }

    pub fn bytes(&self, reg: u32, len: usize) -> &[u8] {
        &self.mem[reg as usize..reg as usize + len]
    }

    pub fn cfg(&self, reg: u32) -> u32 {
        self.cfg.get(&((reg & !TYPE_MASK) as u16)).copied().unwrap_or(0)
    }

    /// The MAC's own address, as MAC_ADDR_DW0/1 hold it.
    pub fn mac_addr(&self) -> [u8; 6] {
        let mut a = [0; 6];
        a[..4].copy_from_slice(&self.reg(MAC_ADDR_DW0).to_le_bytes());
        a[4..].copy_from_slice(&self.reg(MAC_ADDR_DW1).to_le_bytes()[..2]);
        a
    }

    /// The station interface's BSSID (AP-client slot 0).
    pub fn bssid(&self) -> [u8; 6] {
        let mut a = [0; 6];
        a[..4].copy_from_slice(&self.reg(APC_BSSID_L0).to_le_bytes());
        a[4..].copy_from_slice(&self.reg(APC_BSSID_H0).to_le_bytes()[..2]);
        a
    }

    /// The calibrations the MCU ran, in order: {kind, parameter}.
    pub fn calibrations(&self) -> Vec<(u32, u32)> {
        let w = |p: &[u8], at: usize| u32::from_le_bytes(p[at..at + 4].try_into().unwrap());
        self.commands.iter().filter(|(c, _)| *c == CMD_CALIBRATION_OP).map(|(_, p)| (w(p, 0), w(p, 4))).collect()
    }

    fn tx_on(&self) -> bool {
        self.radio_on && self.reg(MAC_SYS_CTRL) & ENABLE_TX != 0 && self.cfg(CFG_U3DMA) & DMA_TX_BULK_EN != 0
    }

    fn rx_on(&self) -> bool {
        self.radio_on && self.reg(MAC_SYS_CTRL) & ENABLE_RX != 0 && self.cfg(CFG_U3DMA) & DMA_RX_BULK_EN != 0
    }

    // ---- Control endpoint ------------------------------------------------

    pub fn vendor_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) {
        match request {
            REQ_MULTI_WRITE if data.len() == 4 => {
                assert_eq!(value, 0, "model: MULTI_WRITE above 64 KiB");
                self.set(u32::from(index), u32::from_le_bytes(data.try_into().unwrap()));
                self.written(u32::from(index));
            }
            REQ_MULTI_WRITE => {
                assert!(value == 0 && data.len() % 4 == 0, "model: a copy of {} bytes to {index:#x}", data.len());
                let r = usize::from(index);
                self.mem[r..r + data.len()].copy_from_slice(data);
            }
            REQ_WRITE_CFG => {
                assert_eq!(data.len(), 4, "model: WRITE_CFG of {index:#x} not 4 bytes");
                let mut v = u32::from_le_bytes(data.try_into().unwrap());
                // The power domain comes up when asked.
                if index == (CFG_MTC_CTRL & 0xffff) as u16 && v & MTC_MTCMOS_PWR_UP != 0 {
                    v |= MTC_STATE_UP | MTC_PWR_ACK | MTC_PWR_ACK_S;
                }
                self.cfg.insert(index, v);
            }
            REQ_WRITE_FCE => {
                assert!(data.is_empty(), "model: WRITE_FCE with data");
                let (reg, high) = (index & !2, index & 2 != 0);
                assert!(reg == FCE_DMA_ADDR || reg == FCE_DMA_LEN, "model: WRITE_FCE of {index:#x}");
                let v = self.fce.entry(reg).or_default();
                *v = if high { (*v & 0xffff) | u32::from(value) << 16 } else { (*v & !0xffff) | u32::from(value) };
            }
            REQ_DEV_MODE if value == DEV_MODE_RESET => {
                assert!(data.is_empty());
                self.running = false;
                self.set(MCU_COM_REG0, 0);
            }
            // The vectors: the MCU starts on the code loaded, once patched.
            REQ_DEV_MODE if value == DEV_MODE_IVB => {
                assert!(data.is_empty(), "model: vectors given as data");
                assert!(self.patched, "model: the firmware started on an unpatched ROM");
                let dlm = (DLM_OFFSET + DLM_OFFSET_E3) as usize;
                assert_eq!(&self.mcu[ILM_OFFSET as usize..][..self.ilm.len()], &self.ilm[..], "model: not the firmware's ILM");
                assert_eq!(&self.mcu[dlm..dlm + self.dlm.len()], &self.dlm[..], "model: not the firmware's DLM");
                self.running = true;
                self.set(MCU_COM_REG0, 1);
            }
            _ => panic!("model: vendor request {request:#x} out ({value:#x}, {index:#x})"),
        }
    }

    pub fn vendor_in(&mut self, request: u8, value: u16, index: u16, data: &mut [u8]) {
        assert_eq!(data.len(), 4, "model: a read of {} bytes", data.len());
        let v = match request {
            REQ_MULTI_READ => {
                assert!(value == 0 && index & 3 == 0, "model: MULTI_READ of {value:#x}:{index:#x}");
                let v = self.reg(u32::from(index));
                // The statistics clear when read.
                if matches!(u32::from(index), RX_STAT_0 | RX_STAT_1 | RX_STAT_2 | TX_STA_0 | TX_STA_1 | TX_STA_2) {
                    self.set(u32::from(index), 0);
                }
                v
            }
            REQ_READ_CFG => self.cfg.get(&index).copied().unwrap_or(0),
            REQ_READ_EEPROM => {
                let at = usize::from(index);
                assert!(at + 4 <= EEPROM_SIZE, "model: EEPROM read at {at:#x}");
                u32::from_le_bytes(self.ee[at..at + 4].try_into().unwrap())
            }
            _ => panic!("model: vendor request {request:#x} in of {index:#x}"),
        };
        data.copy_from_slice(&v.to_le_bytes());
    }

    /// The ROM patch's commands to the MCU: apply the patch, reset the
    /// WMT, which then says it is applied.
    pub fn class_out(&mut self, request: u8, value: u16, _index: u16, data: &[u8]) {
        assert_eq!((request, value), (REQ_DEV_MODE, DEV_MODE_IVB), "model: class request {request:#x}");
        if data == PATCH_ENABLE {
            let at = ROM_PATCH_OFFSET as usize;
            assert_eq!(&self.mcu[at..at + self.patch.len()], &self.patch[..], "model: not the ROM patch");
            self.patch_enabled = true;
        } else if data == PATCH_RESET_WMT {
            assert!(self.patch_enabled, "model: WMT reset before the patch was enabled");
            self.patched = true;
            // Revision E3 and later say so in the MCU clock register.
            self.set(MCU_CLOCK_CTL, 1);
        } else {
            panic!("model: class request data {data:02x?}");
        }
    }

    /// Register `reg` was written.
    fn written(&mut self, reg: u32) {
        if reg == TX_CPU_FCE_DESC_IDX {
            assert_eq!(self.reg(reg), self.blocks, "model: descriptor index not moved once a block");
        }
    }

    // ---- Bulk endpoints --------------------------------------------------

    pub fn bulk_out(&mut self, ep: u8, data: &[u8]) {
        assert!(data.len() >= 8 && data.len() % 4 == 0, "model: transfer of {} bytes", data.len());
        assert!(data[data.len() - 4..].iter().all(|&b| b == 0), "model: no zeros after the transfer");
        let info = u32::from_le_bytes(data[..4].try_into().unwrap());
        if ep == EP_MCU {
            assert_eq!(info & 0x3800_0000, CPU_TX_PORT << MSG_PORT_SHIFT, "model: MCU message to port {}", (info >> 27) & 7);
            assert_ne!(info & MSG_TYPE_CMD, 0, "model: not a command");
            if self.running {
                self.command(info, &data[4..data.len() - 4]);
            } else {
                self.code_block(info, &data[4..data.len() - 4]);
            }
            return;
        }
        assert!(EP_DATA[..4].contains(&ep), "model: bulk OUT on {ep:#x}");
        self.frame(ep, info, &data[4..data.len() - 4]);
    }

    /// A block of the ROM patch or firmware, where the FCE's DMA address
    /// says, its length the FCE's DMA length.
    fn code_block(&mut self, info: u32, body: &[u8]) {
        let len = (info & 0xffff) as usize;
        assert_eq!(body.len(), (len + 3) & !3, "model: block of {len} in {} bytes", body.len());
        assert!(
            self.reg(FCE_PSE_CTRL) == 1
                && self.reg(TX_CPU_FCE_BASE) == 0x0040_0230
                && self.reg(TX_CPU_FCE_MAX_COUNT) == 1
                && self.reg(FCE_PDMA) == 0x44
                && self.reg(FCE_SKIP_FS) == 3
                && self.cfg(CFG_U3DMA) & DMA_TX_BULK_EN != 0,
            "model: code sent before the FCE was set up"
        );
        assert!(len <= FW_MAX_PAYLOAD - 8, "model: block of {len} bytes");
        assert_eq!(self.fce[&FCE_DMA_LEN] >> 16, body.len() as u32, "model: FCE DMA length not the block's");
        let at = self.fce[&FCE_DMA_ADDR] as usize;
        if !self.patched {
            assert!(len <= ROM_PATCH_MAX_PAYLOAD - 8, "model: ROM patch block of {len} bytes");
        }
        self.mcu[at..at + len].copy_from_slice(&body[..len]);
        self.blocks += 1;
    }

    /// A command to the running MCU: its effect, and its answer when its
    /// sequence number asks for one.
    fn command(&mut self, info: u32, body: &[u8]) {
        let len = (info & 0xffff) as usize;
        assert_eq!(body.len(), len, "model: command of {len} in {} bytes", body.len());
        let cmd = ((info >> MSG_CMD_SHIFT) & 0x7f) as u8;
        let seq = (info >> MSG_SEQ_SHIFT) & 0xf;
        let w = |at: usize| u32::from_le_bytes(body[at..at + 4].try_into().unwrap());
        match cmd {
            CMD_POWER_SAVING_OP => self.radio_on = w(0) == RADIO_ON,
            CMD_FUN_SET_OP => assert_eq!((w(0), seq), (Q_SELECT, 0), "model: FUN_SET {} answered", w(0)),
            CMD_SWITCH_CHANNEL_OP => {
                assert!(self.radio_on, "model: tuned with the radio off");
                assert_eq!(u16::from_le_bytes([body[4], body[5]]), 0x202, "model: not both chains");
                assert_eq!(body[2], 0, "model: not 20 MHz");
                self.tuned = Some((body[0], body[1] != 0));
            }
            CMD_CALIBRATION_OP | CMD_LOAD_CR | CMD_INIT_GAIN_OP => {}
            _ => panic!("model: MCU command {cmd}"),
        }
        self.commands.push((cmd, body.to_vec()));
        if seq != 0 {
            let rxfce = seq << RX_FCE_SEQ_SHIFT | EVT_CMD_DONE << RX_FCE_EVT_SHIFT;
            let mut a = rxfce.to_le_bytes().to_vec();
            a.extend_from_slice(&[0; 4]);
            self.answers.push_back(a);
        }
    }

    /// A frame after its TXWI, to the air.
    fn frame(&mut self, ep: u8, info: u32, body: &[u8]) {
        assert!(self.tx_on(), "model: frame sent with transmit off");
        let want = TXD_80211 | TXD_WIV | QSEL_EDCA << TXD_QSEL_SHIFT | WLAN_PORT << MSG_PORT_SHIFT;
        assert_eq!(info & !0xffff, want, "model: TXINFO {info:#x}");
        assert_eq!((info & 0xffff) as usize, body.len(), "model: TXINFO length of a {}-byte body", body.len());
        let txwi = &body[..TXWI_LEN];
        let rate = u16::from_le_bytes([txwi[2], txwi[3]]);
        let (ack_ctl, wcid, txstream) = (txwi[4], txwi[5], txwi[17]);
        let len = usize::from(u16::from_le_bytes([txwi[6], txwi[7]]));
        // The header, padded to 4 bytes, then the rest.
        let hdr = mtw::rx::hdrlen(body[TXWI_LEN], body[TXWI_LEN + 1]);
        let pad = if hdr % 4 != 0 { 2 } else { 0 };
        assert!(TXWI_LEN + len + pad <= body.len(), "model: TXWI length {len} in a {}-byte body", body.len());
        let mut frame = body[TXWI_LEN..TXWI_LEN + hdr].to_vec();
        assert!(body[TXWI_LEN + hdr..TXWI_LEN + hdr + pad].iter().all(|&b| b == 0), "model: header pad not zeros");
        frame.extend_from_slice(&body[TXWI_LEN + hdr + pad..TXWI_LEN + len + pad]);
        let mgmt = frame[0] & 0x0c == 0;
        if mgmt {
            assert_eq!(ep, EP_DATA[3], "model: a management frame on {ep:#x}");
        }
        let (channel, _) = self.tuned.expect("model: frame sent before the radio was tuned");
        let ra: [u8; 6] = frame[4..10].try_into().unwrap();
        if !mgmt && ra[0] & 1 == 0 {
            // Unicast data: to the entry of the access point it goes to.
            assert_eq!(self.bytes(wcid_addr(wcid), 6), &ra, "model: station entry {wcid} is not {ra:02x?}");
        }
        assert_eq!(ack_ctl & TXWI_ACK_REQ != 0, ra[0] & 1 == 0, "model: ACK wanted of a group frame, or not of another");
        assert_eq!(txstream, TXSTREAM_E4, "model: the TXWI's streams");
        let kbps = rate_kbps(rate, channel);
        // A frame faster than the access point's signal carries is lost:
        // the MAC retries it 7 times and counts a failure.
        let heard = self.air.rssi(channel, ra).is_some_and(|rssi| kbps <= max_kbps(rssi));
        let acked = ack_ctl & TXWI_ACK_REQ != 0 && heard;
        if ack_ctl & TXWI_ACK_REQ != 0 && self.air.has_ap(channel, ra) {
            let (sta0, sta1) = (self.reg(TX_STA_0), self.reg(TX_STA_1));
            if acked {
                self.set(TX_STA_1, sta1 + 1);
            } else {
                self.set(TX_STA_0, sta0 + 1);
                self.set(TX_STA_1, sta1 + (7 << 16));
            }
        }
        if heard || ra[0] & 1 != 0 {
            let answers = self.air.hear(channel, &frame);
            self.heard.extend(answers);
        }
        self.sent.push(Sent { channel, endpoint: ep, rate, ack_ctl, wcid, txstream, acked, frame });
    }

    /// The TX statistics as the MAC counts them: {failures, successes,
    /// retries}.
    pub fn tx_counts(&self) -> (u32, u32, u32) {
        let sta1 = self.reg(TX_STA_1);
        (self.reg(TX_STA_0) & 0xffff, sta1 & 0xffff, sta1 >> 16)
    }

    /// An answer of the MCU, if one waits.
    pub fn bulk_in(&mut self, ep: u8, buf: &mut [u8]) -> usize {
        assert_eq!(ep, EP_RESP, "model: answers read from {ep:#x}");
        let Some(a) = self.answers.pop_front() else {
            return 0;
        };
        buf[..a.len()].copy_from_slice(&a);
        a.len()
    }

    /// The bulk IN transfers on the frames endpoint: what was heard on the
    /// channel tuned to since (each access point's beacon, the answers),
    /// past the receive filter, a frame each.
    pub fn rx(&mut self, ep: u8) -> Vec<Vec<u8>> {
        assert_eq!(ep, EP_RX, "model: frames read from {ep:#x}");
        let mut out = Vec::new();
        if !self.rx_on() {
            return out;
        }
        assert_eq!(self.cfg(CFG_U3DMA) & DMA_RX_BULK_AGG_EN, 0, "model: receive aggregation on");
        let Some((channel, _)) = self.tuned else {
            return out;
        };
        let me = self.mac_addr();
        let filter = self.reg(RX_FILTR_CFG);
        let mut heard: Vec<Heard> = self.heard.drain(..).collect();
        heard.extend(self.air.beacons());
        for h in heard.into_iter().filter(|h| h.channel == channel) {
            let to: [u8; 6] = h.frame[4..10].try_into().unwrap();
            if filter & FILTR_PROMISC != 0 && to[0] & 1 == 0 && to != me {
                continue;
            }
            let ta: [u8; 6] = h.frame[10..16].try_into().unwrap();
            let wcid = (1..128u8).find(|&w| self.bytes(wcid_addr(w), 6) == ta).unwrap_or(0xff);
            let mut body = vec![0u8; RXWI_LEN];
            let ctl = u32::from(wcid) | (h.frame.len() as u32) << RXWI_CTL_MPDU_LEN_SHIFT;
            body[4..8].copy_from_slice(&ctl.to_le_bytes());
            // The chains' signal before the LNA gain is taken off, the
            // second 10 dB under the first.
            let raw = i32::from(h.rssi) + LNA;
            body[12] = raw as i8 as u8;
            body[13] = (raw - 10) as i8 as u8;
            body.extend_from_slice(&h.frame);
            body.resize((body.len() + 3) & !3, 0);
            let mut x = (body.len() as u32).to_le_bytes().to_vec();
            x.extend_from_slice(&body);
            x.extend_from_slice(&[0; FCE_INFO_LEN]);
            out.push(x);
        }
        out
    }
}

/// An EEPROM word.
fn word(ee: &mut [u8], at: usize, v: u16) {
    ee[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

/// The rate of TXWI rate word `rate` in kb/s (20 MHz, a long guard
/// interval); a mode or code the MT7612U has not, or CCK on 5 GHz, stops
/// the model.
fn rate_kbps(rate: u16, channel: u8) -> u32 {
    let (mode, idx) = (rate >> RATE_PHY_SHIFT, usize::from(rate & 0x3f));
    const MCS: [u32; 8] = [6500, 13_000, 19_500, 26_000, 39_000, 52_000, 58_500, 65_000];
    match mode {
        PHY_CCK => {
            assert!(channel <= 14, "model: CCK on channel {channel}");
            *[1000, 2000, 5500, 11_000].get(idx & !8).unwrap_or_else(|| panic!("model: CCK code {idx}"))
        }
        PHY_OFDM => *[6000, 9000, 12_000, 18_000, 24_000, 36_000, 48_000, 54_000]
            .get(idx)
            .unwrap_or_else(|| panic!("model: OFDM code {idx}")),
        PHY_HT => {
            assert!(idx < 16, "model: MCS {idx}");
            MCS[idx % 8] * (idx as u32 / 8 + 1)
        }
        _ => panic!("model: PHY mode {mode}"),
    }
}

/// The fastest rate a signal of `rssi` dBm carries to two antennas, roughly.
fn max_kbps(rssi: i8) -> u32 {
    match rssi {
        -50.. => 130_000,
        -60.. => 78_000,
        -70.. => 39_000,
        _ => 9000,
    }
}
